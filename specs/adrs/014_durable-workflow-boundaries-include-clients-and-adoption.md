# ADR-014: Durable-workflow boundaries include client acceptance and profile adoption

- **Status:** Accepted
- **Date:** 2026-07-12
- **Affects:** REQ-DWF-013, REQ-DWF-019, REQ-DWF-022, REQ-DWF-029, REQ-DWF-030, REQ-DWF-031, REQ-DWF-032, REQ-DWF-CREATE-001, REQ-DWF-WAKE-003, REQ-DWF-WAKE-004

## Context

A durable workflow begins at acceptance, but acceptance may occur across a
client-server boundary whose request is retried after lost responses, offline
queue replay, or reconnect. Shell-first persistence prevents server-side loss,
but without a stable client key the client cannot distinguish a lost receipt
from a request that was never accepted. Repeating the request can then create a
second workflow.

Capabilities and user-visible state also cross this boundary. If each client
reimplements server policy, the reducer is no longer the singular source of
product meaning. Durable inbox observations introduce a related future boundary:
notification consumers may need at-least-once delivery, but sharing reducer
consumption state would make independent consumers race over one semantic fact.

The engine also needs an explicit perimeter. A high profile-admission cost is
intentional, but it can encourage crash-spanning features to build a smaller
bespoke scheduler unless adoption criteria are stated.

## Options considered

1. **Leave client acceptance and presentation outside the workflow contract.**
   This keeps the engine narrow but permits duplicate acceptance and semantic
   drift between clients.
2. **Require one universal idempotency key and receipt shape for every
   workflow.** This is uniform but fabricates an external boundary for internal
   workflows and couples unrelated profile receipts.
3. **Make external acceptance a typed profile capability, derive all client
   projections from product state, give additional inbox consumers independent
   dispositions, and define an adoption perimeter.** This keeps optional
   boundaries structural while preserving singular authority.

## Decision

Adopt option 3.

A profile whose acceptance can be retried across a client-server boundary
declares externally retryable acceptance. The request carries a client-supplied
stable key scoped to the profile and acceptance authority. Acceptance atomically
binds that key to one workflow and returns a typed profile receipt containing the
same key and durable handle. Replays return the bound receipt; conflicting intent
under the same key is rejected. Internal profiles omit the capability.

Product reducers remain the source of capability and presentation projections
for every supported client. Pending wake obligations are presentation detail and
lifecycle capability inputs, not a synthetic runtime-busy state.

Reducer inbox delivery remains singular. Any notification, indexing, or other
additional consumer uses its own durable cursor or disposition referencing the
observation; it does not mutate reducer delivery or runtime-acceptance state.

Crash-spanning accepted intent belongs in a durable-workflow profile when it
requires external-effect ambiguity handling, leased retry or takeover,
cancellation or compensation arbitration, durable deadlines, owed delivery, or
protocol migration. Work completed by one synchronous local transaction stays
outside. Profile-admission effort is not a reason to create parallel durable
scheduling authority.

## Consequences

- **Positive:** Offline and reconnecting clients can retry acceptance without
  creating duplicate workflows.
- **Positive:** Web, mobile, API, and runtime actions consume one semantic
  capability projection.
- **Positive:** Future notifications can be at-least-once without stealing or
  duplicating reducer delivery.
- **Positive:** New asynchronous features have a reviewable adoption boundary.
- **Negative:** Externally retryable profiles require normalized acceptance-key
  uniqueness and retained typed receipts.
- **Negative:** Client wire surfaces must evolve when product capability or
  presentation projections evolve.
- **Neutral:** Notification transport remains out of scope until a consumer is
  introduced.

## References

- Related ADRs: ADR-010, ADR-011, ADR-012
- Feature spec: `specs/durable-workflows/requirements.md`
- Client queue spec: `specs/user_message_queue/requirements.md`
