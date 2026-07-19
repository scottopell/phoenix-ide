# Durable Workflows — Executive Summary

## Purpose

Durable Workflows is the shared execution protocol for accepted asynchronous
Phoenix obligations that cross a crash boundary. The product reducer remains the
semantic authority; the engine supplies normalized execution truth, atomic plans,
attempt fencing, optional reclaimable leasing, reconciliation, durable
scheduling, canonical delivery, and runtime-acceptance bookkeeping where a
profile requires it.

Wake and conversation creation remain the two normative profiles. Wake remains
the first intended implementation slice because it exercises observation,
deadline precedence, canonical delivery, continuation, and runtime acceptance.
Creation remains a second profile that exercises a richer dependency graph,
resource reconciliation, and compensation.

## Current Reality

The normative package has been rewritten to the one-scheduler, durable-
acknowledgement model captured by the grand-vision review. The requirements,
Allium package, and a superseding ADR now remove permanent selector,
shadow-authority, rollback, exact-drain, and universal-lease machinery from the
steady-state contract.

The normalized foundation and wake vertical slice implement the one-scheduler
SQLite contract. Workflow attempts, receipts, deliveries, schedules, wake
bindings, terminal evidence, and message links have normalized persisted
authorities. Attempt authority is universally fenced, leases are limited to
reclaimable observation, canonical delivery and runtime acceptance are atomic,
and incompatible persisted work is explicit.

Wake is implemented end to end for durable Bash and tmux obligations:
registration precedes acknowledgement, observation and deadline arbitration are
restart-safe, terminal results materialize once as linked conversation messages,
adoption atomically resolves exact delivery sets, and auto-resume is coalesced
through durable runtime acceptance. Direct chat and conversation creation remain
specified profiles without matching vertical-slice implementations.

## Normative Shape

The current normative package comprises:

- `requirements.md`
- `durable-workflows.allium`
- `direct-chat-profile.allium`
- `wake-profile.allium`
- `creation-profile.allium`
- ADR-013 through ADR-016 and ADR-019 in `specs/adrs/`

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
11. migration safety without permanent parallel authority.

## Implementation Status

| Requirement group | Status | Notes |
| --- | --- | --- |
| REQ-DWF-001–005 reducer authority, normalized ownership, atomic plans, DAGs, barriers | Implemented | The pure engine validates typed transition plans; SQLite persists normalized workflow, effect, dependency, barrier, receipt, delivery, and schedule state atomically. |
| REQ-DWF-006–012 attempt fencing, optional leases, recovery policies, cancellation, manual resolution | Implemented | Persisted attempt/process authority fences every execution; only reclaimable observations receive renewable leases; cancellation and manual outcomes use typed transitions. |
| REQ-DWF-013–018 capabilities, typed migration contract, deterministic verification | Implemented | Capability classes, supported codecs, profile/version compatibility, incompatible status, transactional failpoints, concurrency tests, and restart tests cover the implemented foundation. |
| REQ-DWF-029–042 acceptance, parity/adoption boundaries, one scheduler, durable acknowledgement, canonical delivery, submit-observe, capability classes, direct turns, no-loss migration, CoalesceLatest, independent consumers, migration safety | Foundation implemented; profile coverage varies | The shared one-scheduler repository, durable acknowledgement, canonical delivery, submit-observe, capability, scheduling, and migration-safety mechanisms are implemented. Direct-turn profile behavior remains specified only. |
| REQ-DWF-CHAT-001–011 direct-chat profile | Specified only | Target-bound direct-turn durable acceptance, immutable prepared payloads, typed committed/replay outcomes, target-local runtime arbitration, exact-ID reconciliation, capability-isolated target resolution, and independent per-target fan-out have no matching vertical-slice implementation. |
| REQ-DWF-WAKE-001–005 wake profile | Implemented | Durable Bash/tmux registration, observation, expiry and cancellation arbitration, continuation transfer, canonical terminal delivery, exact-set adoption, restart recovery, and coalesced auto-resume use the normalized foundation. |
| REQ-DWF-CREATE-001–005 creation profile | Specified only | Conversation creation has no matching vertical-slice implementation against the normalized foundation. |

## Relationship to Historical ADRs

ADR-013 through ADR-016 remain historical records of the earlier durable-
workflow direction. ADR-019 supersedes their permanent-selector,
shadow-authority, exact-drain, and universal-lease conclusions for current
normative work without rewriting those historical records.

## Verification Expectations

The implemented foundation and wake profile are covered by pure-engine,
repository, migration, concurrency, restart, failpoint, runtime-recovery, and
tool-registration tests. Full project validation checks Rust, TypeScript, E2E,
code generation, specification shape, and Allium consistency.

Remaining profile work is:

- direct-chat acceptance, replay, conflict, exact-ID reconciliation, and
  target-local runtime arbitration;
- conversation-creation execution and compensation against the normalized
  foundation.

## Related Decisions

- ADR-013 established the normalized-core/profile split.
- ADR-014 recorded the earlier universal-leased effect model.
- ADR-015 recorded the separation of evidence, receipts, and runtime
  acceptance.
- ADR-016 recorded the earlier client-acceptance and adoption-perimeter decision.
- ADR-019 supersedes ADR-013 through ADR-016 where they require permanent
  selector/shadow/drain machinery or universal leased authority.
- ADR-007, ADR-011, and ADR-012 remain historical profile-specific context for
  creation and wake.
