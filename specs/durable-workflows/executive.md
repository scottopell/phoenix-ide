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

The normalized engine foundation and wake vertical slice are implemented. Workflow attempts, receipts, deliveries, schedules, wake bindings, terminal evidence, and message links have normalized persisted authorities; wake is restart-safe for Bash/tmux obligations and remains the only profile with full end-to-end production coverage.

The direct-chat vertical slice durably accepts direct turns and uses a production worker to claim, replay, authoritatively materialize, and settle them by exact accepted-turn generation. The runtime bridge acknowledges authoritative materialization before publishing working state, preserves SSE cursor continuity across replay/claim-loss/error paths, and distinguishes pre-materialization release from post-materialization failure recovery. Provider-response durability across a later local persistence failure remains incomplete. Conversation creation is in a similar state: shell-first and protocol/model work exist, while production worker/orchestration cutover remains incomplete.

## Normative Shape

The current normative package comprises:

- `requirements.md`
- `durable-workflows.allium`
- `direct-chat-profile.allium`
- `wake-profile.allium`
- `creation-profile.allium`
- ADR-013 through ADR-016, ADR-019, and ADR-024 in `specs/adrs/`

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
| REQ-DWF-029–042 acceptance, parity/adoption boundaries, one scheduler, durable acknowledgement, canonical delivery, submit-observe, capability classes, direct turns, no-loss migration, CoalesceLatest, independent consumers, migration safety | Foundation implemented; profile coverage varies | The shared one-scheduler repository, durable acknowledgement, canonical delivery, submit-observe, capability, scheduling, and migration-safety mechanisms are implemented. The production direct-turn slice uses these boundaries through authoritative user-message materialization and exact-generation settlement. |
| REQ-DWF-CHAT-001–011 direct-chat profile | Partially implemented | The production direct-chat slice implements target-bound durable acceptance, immutable prepared payloads, typed committed/replay outcomes, target-local runtime claiming, and authoritative materialization. Provider-response durability and the complete profile verification matrix remain incomplete. |
| REQ-DWF-CHAT-012–015 direct-turn authority, refinement, verification, and exact-generation settlement | Partially implemented | The aggregate, repository, production worker, and runtime bridge implement the authoritative user-message materialization boundary: replay/claim-loss outcomes do not publish provisional working state or leave SSE cursor holes, pre-materialization failures release claims, and final settlement targets the exact accepted turn and generation. Durable provider-response child effects after provider dispatch remain incomplete. |
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

- direct-chat provider-response durability and the remaining profile verification matrix;
- conversation-creation execution and compensation against the normalized foundation.

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
- ADR-007, ADR-011, and ADR-012 remain historical profile-specific context for
  creation and wake.
