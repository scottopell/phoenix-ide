# ADR-181: Federated subscriptions end with their source conversation

- **Status:** Accepted
- **Date:** 2026-10-19
- **Affects:** REQ-FED-015; extends ADR-070 and ADR-180

## Context

ADR-180 extends durable local watch admission across authenticated peers, but does not decide whether a subscription outlives the destination ProductConversation. Local watches target open ordinary conversations and end when their source closes. Continuing a federated subscription after Close or hard deletion would require a retained source tombstone and a new deleted-state reconciliation contract.

## Options considered

1. **Continue after Close or hard deletion** — rejected because the source can no longer provide the ordinary current-state authority required for honest gap reconciliation without retaining a new tombstone contract.
2. **End when the source closes** — selected because it preserves the local watch lifecycle, suppresses unaccepted delivery, and keeps accepted facts durable without inventing post-deletion state.

## Decision

A federated subscription may target only an open ordinary ProductConversation. It follows that stable conversation across transcript continuation while the ProductConversation remains open.

Closing the source ends its active subscriptions and suppresses facts the caller has not durably admitted. Facts already durably admitted by the caller remain accepted history and are not retracted.

Hard deletion after Close does not require a retained subscription tombstone or current-state reconstruction guarantee.

## Consequences

- **Positive:** Local and federated watch lifecycles share one source-ownership boundary.
- **Positive:** Gap reconciliation never fabricates a current state for a deleted source.
- **Negative:** A caller that still needs observation after Close must establish a new source under a separately authorized lifecycle rather than revive the ended subscription.
- **Neutral:** Already admitted facts remain durable independently of later source deletion.

## References

- ADR-070, ADR-180
- `specs/federation/requirements.md` REQ-FED-015
- `specs/federation/federation.allium`
