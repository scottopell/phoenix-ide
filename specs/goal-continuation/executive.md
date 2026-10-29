# Goal continuation admission — executive status

## Source and ownership

Goal creation/replacement checks the same ProductConversation's automatic-continuation preference in the shared write transaction. It does not lock the setting ON, reactivate inactive Goal identities, alter historical evidence or dispatch work. ADR-192 records the admission-only decision. The native Tools owner owns the setting/lifecycle tools; this surface owns admission consistency.

## Verification

| Requirement | Source coverage | Qualification |
| --- | --- | --- |
| REQ-GOAL-AC-001 | `apply_goal_tx`, owner HTTP and registered native prepare/apply regressions | Exact-candidate host run pending |
| REQ-GOAL-AC-002 | Refusal rollback, accepted replay, unchanged preference/history/receipt assertions | Exact-candidate host run pending |
| REQ-GOAL-AC-003 | Disabled legacy reads, metadata, Complete/Suspend/Exhausted/Stop regressions | Exact-candidate host run pending |
| REQ-GOAL-AC-004 | Registered runtime tool executor, retained work-count assertions and runtime cancellation checkpoint regressions | Exact-candidate host run pending |

The recovered historical candidate `b3d49f6a6` passed 23 DB tests; native inventory compilation stopped at a private `api::goal_commands` module. The visibility correction is retained, but those historical DB results do not qualify the changed admission source. Source parsing/formatting and independent source review are not executed runtime proof. The sole host custodian runs the frozen exact candidate serially, with Tools' expensive execution priority retained. No merge or deployment approval is implied.
