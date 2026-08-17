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

Direct-chat foundations have landed only partially. The pure aggregate/repository layer now contains accepted-turn identity, immutable prepared payload, runtime-acceptance, replay, and deterministic test foundations (see `crates/phoenix-db/src/workflow/direct_turn.rs` and the status row for REQ-DWF-CHAT-012–014), but Phoenix has not cut production chat submission/reconciliation over to that durable profile yet. The durability and local SQLite fail-stop doctrine is adopted, while the closed authoritative-result boundary, commit-before-publication enforcement, and process fail-stop infrastructure remain unimplemented. Conversation creation is in a similar state: shell-first and protocol/model work exist, while production worker/orchestration cutover remains incomplete.

## Normative Shape

The current normative package comprises:

- `requirements.md`
- `durable-workflows.allium`
- `direct-chat-profile.allium`
- `wake-profile.allium`
- `creation-profile.allium`
- ADR-013 through ADR-016, ADR-019, ADR-024, and ADR-036 in `specs/adrs/`

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
| REQ-DWF-043 durable facts and local SQLite classification | Doctrine adopted; enforcement not implemented | Requirements, direct-chat Allium, and ADR-036 define disposable process projections, a closed established/unclassified result, one exact authoritative-row query, and fail-stop selection. Production boundaries do not yet enforce the complete contract. |
| REQ-DWF-CHAT-001–011 direct-chat profile | Specified only | Target-bound direct-turn durable acceptance, immutable prepared payloads, typed committed/replay outcomes, target-local runtime arbitration, exact-ID reconciliation, capability-isolated target resolution, and independent per-target fan-out have no matching vertical-slice implementation. |
| REQ-DWF-CHAT-012–014 direct-turn authority, refinement, and verification | Partially implemented | The pure aggregate and authoritative repository implement scoped replay, immutable prepared semantics, runtime ownership, canonical materialization identity, terminal generation, atomic response-plus-terminal-obligation establishment, bounded settlement retry after durable establishment, active restart rematerialization, legacy ambiguous-turn retirement, and deterministic transaction-cut tests. Exact-turn Stop fencing, process fail-stop integration, exact terminal-row post-commit classification, and the complete interleaving matrix remain incomplete. |
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
- ADR-024 partitions direct-turn semantic authority across the reducer, aggregate,
  and normalized child effects with one writable authority per fact.
- ADR-036 selects process fail-stop when local SQLite authority cannot be established and makes process-local runtime and observer continuity disposable.
- ADR-007, ADR-011, and ADR-012 remain historical profile-specific context for
  creation and wake.
