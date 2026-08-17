# ADR-037: Legacy direct-turn terminal ambiguity is retired as failure

- **Status:** Accepted
- **Date:** 2026-08-17
- **Affects:** REQ-DWF-CHAT-013, REQ-DWF-CHAT-014, REQ-COMP-004

## Context

Older databases can contain a materialized runtime direct turn that still owns its conversation even though an agent response was persisted after the canonical input. Those rows predate durable terminal-obligation records. Persisted message content does not retain the provider's `end_turn` signal or a terminal kind, reason, and target reducer projection. The exact intended terminal result therefore cannot be reconstructed from those rows.

Inferring successful completion from the last transcript message would fabricate authority. Reissuing the provider request can duplicate completed work. Leaving ownership active indefinitely strands the conversation.

## Options considered

1. **Infer completion from transcript shape** — can unstick successful incidents, but cannot distinguish nonterminal responses, failed terminal results, or exact target projections.
2. **Resume the provider request** — preserves the previous generic recovery behavior, but can repeat a request whose response was already committed.
3. **Retire ambiguous legacy owners as failed** — preserves the transcript and exact turn identity, releases ownership through the normal atomic terminal transaction, and exposes that exact recovery was unavailable without fabricating success.
4. **Require manual row-specific repair** — avoids a general policy, but leaves supported forward migration unable to converge existing rows automatically.

## Decision

Choose option 3.

When the terminal-obligation table is introduced, each pre-existing materialized runtime turn that remains nonterminal and owns its conversation receives a `Failed` terminal obligation at its current turn identity and generation. Its target projection is a visible server error explaining that Phoenix restarted before an exact terminal result was recorded. The migration does not inspect transcript tails and does not claim the turn completed successfully.

Runtime reconstruction consumes the obligation through the same exact-identity atomic terminal settlement used for newly established obligations. The transcript remains unchanged. A user may retry from the resulting error state as a new turn.

New provider responses use a different contract: response message and exact terminal obligation commit atomically. Before that commit succeeds, the owning runtime retains the exact response transition in process and does not fall back to database reconstruction. After establishment, bounded settlement retry may rematerialize from the durable obligation.

## Consequences

- **Positive:** Supported forward migration converges stranded legacy owners, including responses that may already be present, without duplicate provider dispatch.
- **Positive:** No terminal kind, failure reason, or success projection is fabricated from message content.
- **Positive:** New rows have exact durable recovery evidence tied to turn identity and generation.
- **Negative:** A legacy turn that could have been classified as successful with unavailable historical provider metadata is surfaced as failed and requires an explicit user retry.
- **Negative:** The policy applies to all ambiguous materialized runtime owners at the migration boundary because storage cannot safely distinguish subsets.

## References

- `specs/compatibility/requirements.md`
- `specs/durable-workflows/requirements.md`
- ADR-024: Direct-turn authority is partitioned by semantic fact.
- ADR-034: Compatibility guarantees are explicit and data-aware.
