# ADR-015: Observation, receipt, and runtime acceptance are distinct durable facts

- **Status:** Accepted
- **Date:** 2026-07-11
- **Affects:** REQ-DWF-001, REQ-DWF-002, REQ-DWF-007, REQ-DWF-012, REQ-DWF-017, REQ-DWF-WAKE-002, REQ-DWF-WAKE-003, REQ-DWF-WAKE-005, REQ-DWF-CREATE-003

## Context

External evidence, accepted execution outcome, and entry into a running product
state happen at different crash boundaries. A tmux exit marker is evidence; the
engine may reconcile it into a wake receipt; the conversation runtime may accept
the resulting observation only later when idle. Collapsing these facts either
lets infrastructure decide product success or loses the durable boundary between
queued delivery and an accepted runtime action.

Not every profile needs later runtime acceptance. Creation receipts can often
return directly through the reducer in the committing worker, while wake delivery
must survive busy runtimes, continuation, and restart.

## Options considered

1. **Treat observations as successful receipts.** This is compact but lets raw
   evidence acquire product meaning and cannot represent conflict or adoption.
2. **Treat a persisted receipt or message as runtime acceptance.** This avoids an
   outbox, but recovery cannot distinguish owed dispatch from an already accepted
   product action.
3. **Separate all three concepts and make owed acceptance an optional core
   capability.** Profiles always distinguish observations and receipts; profiles
   that cross a separately scheduled runtime use normalized owed-acceptance rows
   accepted atomically with product state.

## Decision

Adopt option 3. Observations are append-only typed evidence. One accepted terminal
receipt records the engine's reconciled execution truth. The same product reducer
interprets that receipt. When reducer output must later enter a separately
scheduled runtime, the engine core may persist a normalized owed-acceptance record;
acceptance is complete only in the transaction that persists the exact accepting
product state. Profiles without this boundary do not create synthetic acceptance
records.
Receipt acceptance atomically creates exactly one reducer inbox row. Reducer
inbox progression has its own pending/consumed delivery state and does not share a
status or timestamp with runtime acceptance. Reducer consumption either ends there
for a profile without the capability or atomically creates one typed owed-acceptance
row for a profile that declares it. Suppression uses a typed reason.

Wake uses the capability for coalesced resume requests and continuation transfer.
Creation uses observations and receipts but owes runtime acceptance only for a
step whose product contract actually crosses that boundary.

## Consequences

- **Positive:** Raw evidence cannot silently become product success, and product
  meaning stays reducer-owned.
- **Positive:** Restart can retry owed delivery without duplicating an accepted
  runtime action.
- **Positive:** Optional capability ownership avoids forcing outbox semantics onto
  profiles that do not need them.
- **Negative:** Wake requires normalized acceptance bookkeeping and atomic
  integration with conversation-state persistence.
- **Negative:** Implementers must preserve three related identities and codec
  contracts rather than one overloaded status.
- **Neutral:** Accepted acceptance rows may remain as audit history while pending
  queries exclude them.

## References

- Related ADRs: ADR-011, ADR-012, ADR-013, ADR-014
- Feature spec: `specs/durable-workflows/requirements.md`
- Wake profile: `specs/wake-contracts/requirements.md`
