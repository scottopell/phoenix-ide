# Durable Workflows — Executive Summary

## Purpose

Durable Workflows is the shared execution protocol for accepted asynchronous
Phoenix obligations. The product reducer remains the semantic authority; the
engine supplies normalized execution truth, atomic plans, leased effects,
reconciliation, durable scheduling, cancellation compensation, and runtime
acceptance bookkeeping where a profile requires it.

Wake and conversation creation are the two normative profiles. Wake is the first
production adoption target; creation follows through non-authoritative shadow
comparison and a versioned acceptance cutover.

## Current Reality

The normative requirements and architectural decisions are specified. Existing
wake and creation implementations remain their respective execution authorities
until an engine-backed protocol is selected for new acceptances. No durable
workflow Allium model is part of this specification pass.

## Delivery Sequence

1. Implement the pure engine and deterministic simulator for both profiles.
2. Persist atomic normalized transitions, effects, claims, evidence, receipts,
   barriers, deadlines, and optional owed-runtime-acceptance records.
3. Run wake in non-authoritative shadow mode, then select engine authority for new
   wake registrations while legacy registrations drain.
4. Run creation in non-authoritative shadow mode, then select engine authority for
   new creation acceptances while legacy jobs drain.
5. Retire each legacy scheduler only after durable zero-authority proof.

## Requirement Coverage

| Requirement group | Status | Intended verification / code surface |
| --- | --- | --- |
| REQ-DWF-001–005 authority, ownership, atomic plans, DAGs, barriers | Specified | Pure reducer/engine model; transactional persistence |
| REQ-DWF-006–009 leased execution and ambiguity | Specified | Claim/renew/takeover simulation; typed profile adapters |
| REQ-DWF-010–012 deadlines, cancellation, manual resolution | Specified | Virtual-time schedules; cancellation transaction; operator flow |
| REQ-DWF-013–017 capabilities, versions, migration, acceptance | Specified | API/runtime projection tests; shadow/cutover/drain tests |
| REQ-DWF-018 deterministic verification | Specified | Property schedules with checked-in minimized regressions |
| REQ-DWF-WAKE-001–005 wake profile | Specified | Bash/tmux registration-to-resume end-to-end campaigns |
| REQ-DWF-CREATE-001–005 creation profile | Specified | Shell-first creation, Git/resource, cancel/delete campaigns |

## Related Decisions

- ADR-010 owns the normalized-core/profile boundary and wake-first adoption.
- ADR-011 separates workflow-version serialization from leased effect authority.
- ADR-012 separates observation, receipt, and runtime acceptance and permits the
  normalized owed-acceptance capability only for profiles that need it.
- ADR-007 remains historical authority for creation's fenced reconciliation.
- ADR-008 and ADR-009 remain historical authority for wake registration,
  observations, and durable resume acceptance.

## Implementation Gate

Engine-backed authority is not selected for new profile work until deterministic
fault schedules, real persistence contention/restart tests, profile parity, codec
compatibility, and reversible acceptance selection pass with no unresolved
semantic divergence. Accepted legacy and engine versions drain under their own
executors; no in-flight translation is permitted.
