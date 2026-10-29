# Goal continuation admission — executive status

## Source and ownership

Goal creation/replacement checks the same ProductConversation's automatic-continuation preference in the shared write transaction. It does not lock the setting ON, reactivate inactive Goal identities, alter historical evidence or dispatch work. ADR-192 records the admission-only decision. The native Tools owner owns the setting/lifecycle tools; this surface owns admission consistency.

## Verification

| Requirement | Source coverage | Qualification |
| --- | --- | --- |
| REQ-GOAL-AC-001 | `apply_goal_tx`, owner HTTP and registered native prepare/apply regressions | DB26 at87c391115; native34 and hosted qualification at85136eb96/d3c11a30f, scoped below |
| REQ-GOAL-AC-002 | Refusal rollback, accepted replay, unchanged preference/history/receipt assertions | DB26 at87c391115; native34 and hosted qualification at85136eb96/d3c11a30f, scoped below |
| REQ-GOAL-AC-003 | Disabled legacy reads, metadata, Complete/Suspend/Exhausted/Stop regressions | DB26 at87c391115; native34 and hosted qualification at85136eb96/d3c11a30f, scoped below |
| REQ-GOAL-AC-004 | Registered runtime tool executor, retained work-count assertions and runtime cancellation checkpoint regressions | DB26 at87c391115; native34 and hosted qualification at85136eb96/d3c11a30f, scoped below |

Qualification attribution: DB26 passed at `87c391115`, carried to `85136eb96` on byte-identical DB source. Native34 (12 registry/executor,20 HTTP/service,2 cancellation checkpoint tests), strict DB/IDE Clippy and all-target compilation passed at8513. Corrected `d3c11a30f` passed sole-host ledger3 (identity/order, historical142→145 reentry, idempotence) and hosted changed-crate/reverse-dependency Rust compile/tests/codegen/stale/fmt plus all other executed CI contexts. Conditional skips remain skips; this is not full-workspace or live-race qualification. Original source review plus the existing independent reviewer's mapped07d→d3 changed Goal/admission review found no actionable findings. Historical private-module/compile/lint/test failures remain preserved in PR997 evidence, not erased by later passes.

## Discovery and runtime

The runtime Coordinator registry exposes `read_goal`, `prepare_goal`, and `apply_goal`; ordinary target registries do not gain this authority. Preparation freezes the observation; application is reference-only with retained accepted/refused receipts. Discovery descriptions state that applying create/replacement requires explicit persisted autoContinue enablement and that the tools do not enable it. The existing registered native journey asserts this wording. This discovery-only successor is pending its own validation; d3 receipts are not relabelled as successor execution. Admission, schema, runtime dispatch and migrations are unchanged by the wording correction.

## Delivery dependencies

PR997 is ready for review, not merged. Discovery documentation PR996 is separately ready/unmerged. Goal delivery remains open despite qualification closure. The create/rename dependency PR998 has post86841 corrections3706d39/7cb7bd7/bd5dc4c plus newly accepted authorized-status-snapshot and rename-receipt-retention findings under repair. Source integration waits for the Tools owner's clean reviewed/qualified successor handoff, preserving migrations143/144/Goal145 and additive Tools146; current d3 evidence does not certify that future integrated pin. No migration history rewrite is required. Native autoContinue setter/lifecycle remains Tools' separate follow-on after create/rename land, not a prerequisite for Goal admission readiness. Ordinary source integration needs no bespoke sandbox or renewed human approval; merge/deploy authority remains separate.
