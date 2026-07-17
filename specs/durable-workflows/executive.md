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

Implementation code has not yet been rewritten to match this contract. Existing
merged and in-flight durable-workflow code still reflects the earlier universal-
lease and permanent migration-lifecycle design. This executive therefore records
normative direction, not implementation completion.

## Normative Shape

The current normative package comprises:

- `requirements.md`
- `durable-workflows.allium`
- `wake-profile.allium`
- `creation-profile.allium`
- ADR-013 through ADR-016 and ADR-019 in `specs/adrs/`

These artifacts now state:

1. one scheduler authority per SQLite database;
2. durable acknowledgement as the workflow-adoption boundary;
3. stable direct-turn acceptance keyed by client message identity;
4. universal attempt fencing plus optional reclaimable leases only where the
   phase is reclaimable;
5. structural execution capability classes and recovery policies;
6. one canonical durable delivery lifecycle;
7. submit-then-observe for long-running remote work;
8. typed profile versioning and migration with explicit incompatible-work
   handling;
9. `CoalesceLatest` as the first explicit schedule policy.

## Implementation Status

| Requirement group | Status | Notes |
| --- | --- | --- |
| REQ-DWF-001–005 reducer authority, normalized ownership, atomic plans, DAGs, barriers | Specified, not implemented to new shape | Existing code still includes superseded permanent migration machinery and older entity shapes. |
| REQ-DWF-006–012 attempt fencing, optional leases, recovery policies, cancellation, manual resolution | Specified, not implemented to new shape | Current implementation still assumes universal claimed-step leasing. |
| REQ-DWF-013–019 capabilities, typed migration, durable acknowledgement, canonical delivery | Specified, not implemented to new shape | Canonical single delivery lifecycle is normative; implementation still has overlapping wake and generic delivery state. |
| REQ-DWF-020–029 remote submit-observe, runtime acceptance, direct turns, CoalesceLatest, independent consumers, adoption boundary | Specified only | These are normative requirements without matching implementation on this branch. |
| REQ-DWF-WAKE-001–005 wake profile | Partially implemented under superseded architecture | Wake code exists, but not yet under the narrowed canonical-delivery and optional-lease contract. |
| REQ-DWF-CREATE-001–005 creation profile | Specified only under new architecture | Earlier shadow and cutover machinery is superseded normatively; implementation has not been rewritten. |

## Relationship to Historical ADRs

ADR-013 through ADR-016 remain historical records of the earlier durable-
workflow direction. ADR-019 supersedes their permanent-selector,
shadow-authority, exact-drain, and universal-lease conclusions for current
normative work without rewriting those historical records.

## Verification Expectations

This normative rewrite is complete only at the specification layer. Matching
implementation work still owes:

- pure-engine model updates for attempt authority plus optional reclaimable lease
  authority;
- repository reshaping to one canonical delivery lifecycle;
- typed migration and incompatible-work handling in place of permanent selector
  and drain machinery;
- wake vertical-slice repairs under the new contract;
- creation-profile implementation against the new contract;
- Allium, spec-shape, and timeless-language validation on every further spec
  edit.

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
