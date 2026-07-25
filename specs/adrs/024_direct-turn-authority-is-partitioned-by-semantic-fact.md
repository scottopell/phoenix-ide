# ADR-024: Direct-turn authority is partitioned by semantic fact

- **Status:** Accepted
- **Date:** 2026-07-25
- **Affects:** REQ-DWF-CHAT-001 through REQ-DWF-CHAT-014

## Context

Direct turns accumulated several durable and runtime representations that could
independently answer the same lifecycle questions: acceptance outcomes and live
slots, workflow status and generation, snapshot status and generation, runtime
state and ownership markers, delivery claims, materialization identifiers,
steering queues, terminal timestamps, and provider response tool blocks.

Those representations were locally reasonable, but repeated failures showed that
keeping them synchronized across crashes and interleavings depended on call-site
discipline. The governing requirement is one writable authority per semantic
fact, not one physical row for every concern. Semantic conversation state,
direct-turn acceptance and ownership, and independently claimable effect and
attempt lifecycle are distinct facts and need distinct typed authorities.

## Options considered

1. **Continue coordinating the existing representations.** This preserves the
   current schema but leaves invalid combinations representable and asks tests and
   reviewers to discover missing coordination branches.
2. **Create one scalar god-row and phase for all turn and effect state.** This
   centralizes writes but cannot truthfully represent multiple independently
   claimable tool effects and conflates product meaning with execution truth.
3. **Partition authority by semantic fact and migrate by refinement.** The
   conversation reducer owns semantic conversation state; a direct-turn aggregate
   owns scoped acceptance, immutable prepared semantics, live conversation
   ownership, generation, terminal outcome, and canonical materialization; typed
   child effects and attempts own multiplicative execution lifecycle.

## Decision

Adopt option 3.

Every writable semantic fact has one authority:

- the conversation reducer owns semantic conversation state;
- the direct-turn aggregate owns target-scoped acceptance identity, immutable
  prepared semantics, conversation ownership, turn generation, terminal outcome,
  and canonical transcript materialization;
- normalized child effect and attempt rows own owed, claimed, accepted, released,
  and interrupted execution lifecycle;
- durable tool intents, not provider response blocks, authorize tool execution;
- persisted steering membership, not a mutable runtime queue, owns steering order;
- committed persistence, not an independently advancing broadcaster counter, owns
  externally visible sequence consumption.

Runtime state, workflow snapshots, dispositions, metrics, transcript views, SSE,
and UI state are one-way projections. A persisted projection is permitted only
when the authority-changing transaction writes it and a refinement test compares
its normalized value to the authority.

Migration uses a strangler sequence. The pure transition model is introduced
first, repository commands then prove transactional refinement, and consumers cut
over one slice at a time. Temporary shadow comparison is read-only and
non-authoritative. Its removal gate is: every command in the slice is served by
the new repository, deterministic old/new normalization agrees for the retained
corpus, crash/interleaving tests pass, and no production reader consults the old
representation for a decision. The old writer and representation are deleted in
the same cutover slice.

Correctness never depends on timestamp inference, mutable runtime steering truth,
provider-response-derived tool authority, manual sequence rewind calls, sleeps,
or polling cadence.

## Consequences

- Invalid cross-record combinations are rejected by typed repository commands and
  transaction predicates rather than repaired by later reconciliation.
- Multiplicative tool work remains normalized as independently claimable child
  effects; it is not compressed into a scalar turn phase.
- Existing focused regressions remain acceptance tests and are classified in the
  deterministic crash/interleaving matrix.
- Repository work is larger because each cutover must remove its superseded writer
  rather than leave permanent dual authority.
- The broad direct-turn checkpoint is reference evidence, not the branch on which
  the replacement architecture is stacked.

## References

- Extends ADR-013 and ADR-020.
- Feature spec: `specs/durable-workflows/requirements.md`
- Behavioral specs: `specs/durable-workflows/direct-chat-profile.allium`,
  `specs/durable-workflows/llm-profile.allium`
- Pure model: `phoenix_workflow::direct_turn`
