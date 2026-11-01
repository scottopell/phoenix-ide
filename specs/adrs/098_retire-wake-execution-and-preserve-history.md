# ADR-098: Retire wake execution and preserve history

- **Status:** Accepted
- **Date:** 2026-10-12
- **Affects:** REQ-WAKE-RETIRE-001 through REQ-WAKE-RETIRE-005; retired REQ-WAKE-001 through REQ-WAKE-018, REQ-DWF-WAKE-001 through REQ-DWF-WAKE-005, and REQ-SA-009; wake-specific portions of ADR-006, ADR-011, ADR-012, ADR-013, ADR-015, ADR-016, and ADR-020

## Context

The user explicitly approved removal of the durable wake feature, retirement during
migration, and preservation of its historical data. This is removal, not a pause
or a commission to redesign wake. The candidate implementation and its verification
are ongoing; this decision records scope, not a completed test or deployment claim.

Wake execution was coupled to shared workflow scheduling, observations, runtime
admission, continuation, tool/resource lifecycle, and Close. Removing live wake
behavior must not delete its historical evidence or interfere with a DirectTurn
already durably admitted. The shared workflow foundation, direct-turn authority,
creation contract, ordinary Bash/tmux operations, sub-agent fan-in, and existing
settlement machinery retain independent authority.

## Options considered

1. **Retain a dormant wake framework or defer replacement design.** This keeps
   speculative registration, router, scheduling, and lifecycle obligations that
   contradict the approved removal and creates an unrequested redesign promise.
2. **Rewrite or delete wake workflows as terminal/cancelled work.** This removes
   execution authority by altering history, conflating feature retirement with a
   real execution outcome and risking loss of messages, receipts, and links.
3. **Remove operational wake code and mark historical workflows retired with
   additive metadata.** Preserve history unchanged, deny all new wake admission
   and execution, and keep already-admitted DirectTurns on their ordinary path.

## Decision

Adopt option 3. Remove wake registration, observation/router workers, scheduling,
reconciliation, delivery/runtime activation, continuation transfer, and lifecycle
blocking/settlement obligations. Retire the normative wake feature requirements and
the wake-contract and wake-profile Allium models. Replace the feature contract with
explicit retirement and history requirements rather than a hypothetical framework.

Apply a one-time additive migration that records feature-retirement metadata for
all historical wake workflows, regardless of status. Preserve their payloads,
statuses, generations, attempts, schedules, bindings, evidence, receipts, canonical
deliveries, acceptance records, messages, and links. Do not manufacture cancellation
or completion, and do not convert undelivered wake history into newly admitted work.
Historical wake pointers remain evidence, never execution authority.

Already-admitted DirectTurns retain their exact identity, generation, payload,
ownership, and delivery/message links and follow existing execution, recovery,
cancellation, and terminal settlement. Wake removal does not replace settlement
machinery or grant special authority over ordinary direct turns, tools, children,
shared WorkScopes, or Close.

The forward migration policy of ADR-034 and `specs/compatibility/requirements.md`
continues to apply. This decision grants no downgrade, old-binary opening,
wake-restoration, or replacement-design guarantee.

ADR-006, ADR-011, and ADR-012 are superseded for wake feature execution. Wake-specific
obligations in ADR-013, ADR-015, ADR-016, and ADR-020 are superseded; their shared
foundation, creation, and direct-turn decisions remain applicable. These historical
records are preserved rather than rewritten.

## Consequences

- **Positive:** Removal is structural and does not leave an operational wake
  framework or lifecycle guard behind.
- **Positive:** Historical evidence stays intact without false terminal outcomes.
- **Positive:** Already-admitted work keeps the same normal settlement authority.
- **Negative:** Historical wake waits and unaccepted deliveries no longer produce
  autonomous observations or model turns, even when their recorded status is pending.
- **Verification obligation:** Qualify migration preservation, inert wake rows across
  startup/recovery/continuation/Close, absence of new wake admission/worker/runtime
  activation, and normal already-admitted DirectTurn execution and settlement.

## References

- ADR-006, ADR-011, ADR-012: historical wake contracts and wake-resume decisions.
- ADR-013, ADR-015, ADR-016, ADR-020: shared foundation and former wake-profile boundaries.
- ADR-024, ADR-074: retained direct-turn authority and restart baton.
- ADR-034: explicit compatibility policy; ADR-088: ordinary Close policy.
- `specs/wake-contracts/requirements.md`: retirement and history contract.
- `specs/durable-workflows/requirements.md`: retained foundation/direct-turn/creation contracts.
- `specs/compatibility/requirements.md`: forward migration and rollback boundaries.
