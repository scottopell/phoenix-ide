# Durable Workflows — Executive Summary

## Purpose

Durable Workflows is the shared execution protocol for accepted asynchronous
Phoenix obligations that cross a crash boundary. The product reducer remains the
semantic authority; the engine supplies normalized execution truth, atomic plans,
attempt fencing, optional reclaimable leasing, reconciliation, durable
scheduling, canonical delivery, and runtime-acceptance bookkeeping where a
profile requires it.

Direct-chat and conversation creation are the retained normative profiles.
Creation exercises a richer dependency graph, resource reconciliation, and
compensation. Wake is retired under `../wake-contracts/requirements.md` and
ADR-092, without a replacement profile or redesign commitment.

## Current Reality

The normalized engine foundation is implemented. Workflow attempts, receipts,
deliveries, and schedules have normalized persisted authorities. Wake removal is
a candidate implementation with migration and runtime verification ongoing; this
summary claims neither final validation nor deployment. Historical wake bindings,
terminal evidence, statuses, payloads, messages, and links are preserved as inert
history by additive retirement metadata, not rewritten as settled execution.
Already-admitted DirectTurns remain normal work on their existing authorities.

Direct-chat foundations have landed partially. The aggregate/repository layer contains accepted-turn identity, immutable prepared payload, runtime acceptance, replay, and deterministic transaction-cut foundations. Production direct-turn materialization now returns a closed established/unclassified authority result, keeps proposed conversation state private until SQLite commits the accepted turn, reducer projection, and canonical message, and routes unclassified authority through one process fail-stop boundary that closes runtime admission and semantic publication before draining live runtimes. Production chat submission/reconciliation has not fully cut over to the durable profile, and equivalent authority-loss enforcement for other authoritative SQLite commands remains incomplete. Conversation creation is in a similar state: shell-first and protocol/model work exist, while production worker/orchestration cutover remains incomplete.

## Normative Shape

The current normative package comprises:

- `requirements.md`
- `durable-workflows.allium`
- `direct-chat-profile.allium`
- `creation-profile.allium`
- ADR-013 through ADR-016, ADR-019, ADR-024, ADR-036, and ADR-092 in `specs/adrs/`

These artifacts now state:

1. one scheduler authority per SQLite database;
2. durable acknowledgement as the workflow-adoption boundary;
3. stable externally retryable acceptance and direct-turn acceptance keyed by
   resolved target-bound durable identities;
4. a direct-chat profile with immutable prepared payloads, typed outcome and
   replay variants, target-local runtime arbitration, exact-ID reconciliation,
   and independent per-target fan-out;
5. universal attempt fencing plus optional reclaimable leases only where the
   phase is reclaimable;
6. structural execution capability classes and recovery policies;
7. one canonical durable delivery lifecycle;
8. submit-then-observe for long-running remote work;
9. typed profile versioning and migration with explicit incompatible-work
   handling;
10. `CoalesceLatest` as the first explicit schedule policy;
11. migration safety without permanent parallel authority;
12. durable-fact classification and process fail-stop when local SQLite authority cannot be established.

## Implementation Status

| Requirement group | Status | Notes |
| --- | --- | --- |
| REQ-DWF-001–005 reducer authority, normalized ownership, atomic plans, DAGs, barriers | Implemented | The pure engine validates typed transition plans; SQLite persists normalized workflow, effect, dependency, barrier, receipt, delivery, and schedule state atomically. |
| REQ-DWF-006–012 attempt fencing, optional leases, recovery policies, cancellation, manual resolution | Implemented | Persisted attempt/process authority fences every execution; only reclaimable observations receive renewable leases; cancellation and manual outcomes use typed transitions. |
| REQ-DWF-013–018 capabilities, typed migration contract, deterministic verification | Implemented | Capability classes, supported codecs, profile/version compatibility, incompatible status, transactional failpoints, concurrency tests, and restart tests cover the implemented foundation. |
| REQ-DWF-029–042 acceptance, parity/adoption boundaries, one scheduler, durable acknowledgement, canonical delivery, submit-observe, capability classes, direct turns, no-loss migration, CoalesceLatest, independent consumers, migration safety | Foundation implemented; profile coverage varies | The shared one-scheduler repository, durable acknowledgement, canonical delivery, submit-observe, capability, scheduling, and migration-safety mechanisms are implemented. Direct-turn profile behavior remains specified only. |
| REQ-DWF-043 durable facts and local SQLite classification | Partial: direct-turn authoritative materialization enforced | `WorkflowRepository::materialize_authoritative_turn` returns the reusable closed `LocalAuthorityResult`, owns one exact classifier, and drives process fail-stop when unclassified. `ConversationRuntime::apply_transition_result` keeps proposed direct-turn state private until the accepted turn, reducer projection, and canonical message commit. Other authoritative commands remain outside this slice; pending-delivery materialization is tracked separately. |
| REQ-DWF-CHAT-001–011 direct-chat profile | Specified only | Target-bound direct-turn durable acceptance, immutable prepared payloads, typed committed/replay outcomes, target-local runtime arbitration, exact-ID reconciliation, capability-isolated target resolution, and independent per-target fan-out have no matching vertical-slice implementation. |
| REQ-DWF-CHAT-012–014, 016 direct-turn authority, refinement, verification, and restart baton | Partially implemented | The pure aggregate and authoritative repository implement scoped replay, immutable prepared semantics, runtime ownership, canonical materialization identity, terminal generation, atomic response-plus-terminal-obligation establishment, bounded settlement retry after durable establishment, active restart rematerialization, proactive post-tool startup dispatch under exact-turn ownership, bounded restart-loop failure, legacy ambiguous-turn retirement, deterministic transaction-cut tests, and commit-before-publication for direct-turn adoption. Exact-turn Stop fencing, exact terminal-row post-commit classification, and the complete interleaving matrix remain incomplete. |
| REQ-DWF-WAKE-001–005 wake profile | Retired normative contract; candidate removal / verification ongoing | No wake execution authority. REQ-WAKE-RETIRE-001–005 govern inert historical data, additive retirement metadata, and preservation of already-admitted DirectTurns. |
| REQ-DWF-CREATE-001–005 creation profile | Specified only | Conversation creation has no matching vertical-slice implementation against the normalized foundation; request-bound job identity, immutable starting-pin selection, atomic publication, and ambiguity-safe cleanup remain normative only. |

## Relationship to Historical ADRs

ADR-013 through ADR-016 remain historical records of the earlier durable-
workflow direction. ADR-019 supersedes their permanent-selector,
shadow-authority, exact-drain, and universal-lease conclusions for current
normative work without rewriting those historical records. ADR-092 supersedes
wake-specific execution obligations while retaining shared foundation, direct-turn,
and creation authority.

## Verification Expectations

The foundation has pure-engine, repository, migration, concurrency, restart,
failpoint, and runtime-recovery coverage. Historical wake-profile test results do
not establish acceptance of retirement. Candidate validation must establish
history-preserving migration, no wake admission/worker/runtime activation or
lifecycle blocking, and normal execution and settlement of already-admitted
DirectTurns. That implementation validation remains ongoing with the owning
implementation slice. Full project validation checks Rust, TypeScript, E2E, code
generation, specification shape, and Allium consistency.

Remaining profile work is:

- direct-chat acceptance, replay, conflict, exact-ID reconciliation, and
  target-local runtime arbitration;
- conversation-creation execution and compensation against the normalized
  foundation, including request-bound job identity, immutable starting-pin
  selection, atomic publication, and ambiguity-safe cleanup.

## Related Decisions

- ADR-013 established the normalized-core/profile split.
- ADR-014 recorded the earlier universal-leased effect model.
- ADR-015 recorded the separation of evidence, receipts, and runtime
  acceptance.
- ADR-016 recorded the earlier client-acceptance and adoption-perimeter decision.
- ADR-019 supersedes ADR-013 through ADR-016 where they require permanent
  selector/shadow/drain machinery or universal leased authority.
- ADR-024 partitions direct-turn semantic authority across the reducer, aggregate,
  and normalized child effects with one writable authority per fact.
- ADR-036 selects process fail-stop when local SQLite authority cannot be established and makes process-local runtime and observer continuity disposable.
- ADR-074 selects the active materialized direct turn as the sole post-tool restart baton and rejects transcript-only execution authority.
- ADR-007 remains creation context; ADR-006, ADR-011, and ADR-012 remain
  historical wake context superseded for execution by ADR-092.
- ADR-092 removes operational wake authority, preserves history via additive
  retirement metadata, and retains ordinary already-admitted DirectTurn settlement.
