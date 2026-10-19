# ADR-133: Federated subscriptions carry bounded durable terminal facts

- **Status:** Accepted
- **Date:** 2026-10-12
- **Affects:** REQ-FED-015; extends ADR-070 across authenticated peers

## Context

ADR-070 establishes local Coordinator watches as delivery of future committed facts through ordinary durable admission. Federation needs the same user value across explicitly enrolled instances without turning Phoenix into an event bus, transcript stream, broadcast service, or distributed scheduler.

Remote connectivity can disappear longer than an HTTP request. A subscription therefore needs destination-owned event identity, bounded replay, restart recovery, and an explicit answer when retained history no longer covers the caller's cursor. Peer revocation and unsubscribe must stop future delivery without rewriting facts the caller already accepted.

## Options considered

1. **Stream every conversation event or model token** — rejected because it widens the contract into an unbounded event platform and leaks implementation detail rather than terminal facts.
2. **Infer task completion from transcript text or silence** — rejected because execution settlement and task completion are different semantic facts.
3. **Keep an unbounded event log** — rejected because offline peers would create unlimited retained state.
4. **Reuse durable watch admission with bounded authenticated replay** — selected because it preserves existing ownership while making disconnects and retention gaps explicit.

## Decision

A federated subscription binds one authenticated source instance to one destination stable ProductConversation. It follows continuation and carries only destination-committed terminal completion, failure, or cancellation facts.

Each event has a destination-owned durable ID. Destination admission and caller acceptance use durable cursors and deduplicate by event ID. Restart and reconnect resume after the last accepted cursor. The destination retains at most 1,024 undelivered events per subscription and expires undelivered events after 30 days.

When a caller's cursor predates retained history, the destination reports an explicit gap plus the conversation's authoritative current terminal or nonterminal state. It does not fabricate intermediate events. Unsubscribe or peer revocation blocks future delivery but does not retract already accepted facts.

Discovery and admission reuse ADR-070's owner-driven durable boundaries. Federation does not add indefinite streaming, catch-all predicates, broadcast, remote Bash, a generic event platform, or a distributed scheduler.

## Consequences

- **Positive:** Disconnect and restart behavior is deterministic and testable.
- **Positive:** Stable conversation identity survives continuation without exposing transcript streaming.
- **Negative:** A sufficiently long offline interval can produce an explicit gap requiring current-state reconciliation.
- **Neutral:** Live production subscription acceptance requires a dedicated authorized target; isolated two-instance tests cover ordinary development.

## References

- ADR-070, ADR-083, ADR-034
- `specs/federation/requirements.md` REQ-FED-015
- `specs/federation/federation.allium`
