# ADR-271: Raise the ordinary foreground window to eight

- **Status:** Accepted
- **Date:** 2026-11-18
- **Affects:** REQ-BED-004, REQ-SA-005

## Context

ADR-270 selected four retained ordinary foreground owners. The user selects eight while retaining eager refill after each individual owning commit and the full-round barrier before another model request.

## Options considered

1. Keep the four-owner bound from ADR-270.
2. Raise that fixed bound to eight while preserving the same sliding window.

## Decision

Choose eight retained ordinary preparation/execution owners per conversation. A committed sibling result releases capacity for the next unissued call even while an earlier sibling remains running. Change no permission, cancellation, receipt, source-ordering or mixed-special policy. This replaces only ADR-270's numeric bound; its remaining decisions stand.

## Consequences

Eight independent ordinary calls may overlap, increasing possible per-conversation resource use. This is a user-selected bound, not a benchmark result, throughput promise or global resource guarantee. The model still chooses independence; no configuration, scheduler, dependency detection or locking framework is added. Test/build worker budgets remain separate.

## References

- [ADR-270](270_run-ordinary-foreground-tool-batches-with-a-bounded-barrier.md)
- `specs/bedrock/requirements.md` and `tool-dispatch.allium`
- `specs/subagents/requirements.md`
- `ConversationRuntime::fill_ordinary_batch`
