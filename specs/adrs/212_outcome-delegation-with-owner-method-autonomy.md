# ADR-212: Outcome delegation with owner method autonomy

- **Status:** Proposed
- **Date:** 2026-11-01
- **Affects:** REQ-GR-011

## Context

The user reported that ordinary scroll-regression debugging had become a sequence of Coordinator approvals for command corrections, fixture fixes and reruns. They approved wording that delegates outcomes rather than attempts and requested multiple examples with an unambiguous speaker and recipient.

## Options considered

1. Keep generic follow-through guidance alone: shortest, but it did not clearly distinguish routine method repair from approval-gated actions in the reported case.
2. Add the outcome rule and three role-consistent examples: concrete boundaries and repair latitude, at the cost of a larger prompt. The user accepted the size-budget exception after reviewing the draft.
3. Compress existing guidance to fund the examples: retains the size limit but risks losing previously reviewed reporting, authority and recovery distinctions; no such rewrite is proposed here.

## Decision

Propose the approved delegation paragraph and three hypothetical bad/good pairs in both Coordinator system variants. All examples are Coordinator → owner. They contrast independent debugging within success criteria, waiting for CI and fixing failures while preserving a merge gate, and continuing local work while a shared-server restart remains restricted.

Keep established authority, Stop instructions and the agreed evidence standard binding. Reports do not create new pauses. This introduces no runtime enforcement, Goal admission changes or new permission to merge, deploy or restart services.

## Consequences

The proposed wording makes routine method repair independent of Coordinator approval while preserving evidence quality and real action gates. It adds no runtime authority or enforcement; model behavior remains unqualified.

### Size tradeoff

The literal paragraph and all three requested examples add 185 words / 1,224 Unicode characters plus a separator to each variant. Preserve the old base prompt and its historical ceilings; test the bounded addition separately. This explicitly proposes an increase rather than claiming the earlier same-size budget was met. This draft does not cut security guidance or the earlier silence examples to conceal growth; the user subsequently accepted the larger overall budget: “Prompt increase reasonable, my prior rule aside”.

## Verification

Static tests check all three examples, role consistency, success criteria and explicit boundaries. They do not establish model compliance. Behavioral acceptance should exercise an owner repairing a command and fixture independently, preserving the test oracle, waiting on checks, and stopping at a genuine approval boundary.

## References

- [Global recall requirements](../global-recall/requirements.md), REQ-GR-011.
- [ADR-150](150_concrete-coordinator-reporting-and-blocker-recovery.md): prior recovery and reporting refinement.
- `coordinator_prompt` and its instruction-presence and size tests.
