# ADR-046: ProductConversation owns aggregate presentation without duplicating transcript authority

- **Status:** Accepted
- **Date:** 2026-08-31
- **Affects:** REQ-BED-030A; `ProductConversation`, transcript members

## Context

First-class ProductConversation identity and lifecycle make aggregate navigation and
presentation durable product facts. Transcript rows still own the execution facts
that must remain generation- and session-scoped: continuation edges, messages,
SSE publication, runtime and provider sessions, and prompt projection.

There is a design choice between making ProductConversation a second transcript
authority, leaving aggregate presentation inferred from whichever transcript row
happens to be latest, or drawing an explicit aggregate/member boundary.
Repository authority is a separate concern. Phoenix uses the current
Project-backed repository model, and this lifecycle cutover does not provide a
named consumer that requires replacing it.

## Options considered

1. **Duplicate transcript authority on ProductConversation** — aggregate reads are
   self-contained, but messages, SSE, sessions, and projections gain parallel
   representations that can diverge.
2. **Infer all aggregate presentation from transcript members** — avoids new
   aggregate fields, but makes stable navigation, lifecycle, and presentation
   depend on mutable execution-row state.
3. **Partition aggregate and member authority** — ProductConversation owns stable
   identity, aggregate membership, topology-derived canonical navigation,
   ordinary lifecycle, and aggregate presentation; transcript members own
   continuation edges and execution-scoped facts.

## Decision

Choose the partitioned authority boundary. ProductConversation owns stable
aggregate identity, aggregate membership, topology-derived canonical navigation,
ordinary lifecycle, and aggregate presentation. Transcript members retain
continuation-edge topology, message persistence, SSE publication, runtime and
provider sessions, and generation-fenced prompt projection.

Phoenix does not add aggregate-native message persistence, SSE publication,
runtime/provider sessions, or prompt projection. Phoenix continues using the
current Project-backed repository model; replacement is deferred until a named
feature requires it.

## Consequences

- **Positive:** aggregate list and detail surfaces have one stable identity,
  lifecycle, navigation, and presentation authority.
- **Positive:** transcript execution facts retain their existing generation and
  session fences without a second representation.
- **Negative:** aggregate readers must join or traverse transcript members for
  messages and continuation-edge topology.
- **Neutral:** repository-authority replacement remains deferred and requires a
  separately named consumer and decision.

## References

- ADR-026: Product conversation lifecycle is separate from WorkScope resource ownership
- ADR-031: ProductConversation persistence uses staged single authority
- ADR-045: Provider prompts use persisted generation-fenced projections
- `specs/bedrock/requirements.md` — REQ-BED-030A
