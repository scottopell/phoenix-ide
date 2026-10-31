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

The runtime Coordinator registry exposes `read_goal`, `prepare_goal`, and `apply_goal`; ordinary target registries do not gain this authority. Preparation freezes the observation; application is reference-only with retained accepted/refused receipts. Discovery descriptions state that applying create/replacement requires explicit persisted autoContinue enablement and that the tools do not enable it. The registered native journey asserts this wording and the exact ordered migration ledger through 149. The cumulative integration retains the landed ordinary Coordinator and create/rename tool set through the shared production registry helper. Source review and formatting checks do not qualify this native/runtime journey; historical receipts above are not relabelled as cumulative-source execution.

## Delivery dependencies

PR997 remains unmerged pending qualification of the actual cumulative source. The create/rename dependency PR998 landed as `1da5ff1a5fd523affa0e2f672806e61ceb860ef1`, tree-identical to its qualified `2ab51ce0f7538deb6448c945e2bc96907278b1d1` packet, which the Goal owner explicitly accepted. That landed artifact already registers authentic Goal migration 145 before Tools migrations 146–149. The cumulative Goal integration consumes those unchanged migration and Tools implementations, preserves the current runtime execution machinery, and strengthens the native journey's exact ledger equality from 143–145 to 143–149. No migration identity is rewritten.

Cumulative-source compilation, native discovery/prepare/apply/read/replay, authority and OFF-state safety regressions, applicable schema/generated/UI checks, and current-head CI/review reconciliation remain outstanding. Historical Goal and accepted Tools qualification do not substitute for these integrated checks. Standing user approval permits guarded PR997 merge only after technical readiness; it does not authorize deployment. Native autoContinue setter/lifecycle remains Tools' separately owned follow-on, not a prerequisite for Goal admission readiness.
