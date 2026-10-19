# Durable Conversation Goals — Storage Foundation

## Status

M1 source is authored as a dormant core/DB increment. The schema is staged in `crates/phoenix-db/src/conversation_goals/schema.sql`, **not registered in migrations**; normal databases do not acquire these tables. No runtime caller, HTTP endpoint, goal tool, UI/SSE, follow-through, Stop integration or watch change is installed.

Validation is source-only: existing standalone rustfmt and Git whitespace checks, plus independent read-only reviews. **No compiler, Cargo, SQLite test, migration, browser, VM, hosted CI or production execution has qualified this increment. Authored tests are not passing-test evidence.** Numbered migration and ADR integration await the parent integrator's coordinated reservation.

## Storage model

- Ordinary stable ProductConversation owns each goal; no WorkScope is required.
- Goal-specific immutable version snapshots are the only current/history authority. Current state is the indexed latest owner version, not a separately writable current record or generic event stream.
- Concrete requester and applier occupy normalized actor rows. Agent locator validation checks owner/kind/workflow/transcript membership and an actual typed assistant ToolUse; this is consistency, not authentication or proof that the tool belongs to that exact turn. Exact invocation assignment must come from trusted runtime context.
- An immutable binding stores either Goal or NoGoal at an observed owner version, indexed by durable turn. The primitive is crate-private and dormant. Its current eligibility checks **cannot establish co-creation in the admission transaction**. Actual admission integration must insert the binding atomically with turn creation and must never retrofit a current goal onto an older unbound turn.
- Immutable source locators may outlive deleted source rows. Goal data cascades only with its ordinary owner; standalone history/actor/binding deletion is guarded while that owner exists. Full-schema supported hard-delete tests are authored, not executed.
- There is no release/resume action or safety-budget schema. Objective edits preserve disposition. No policy for collective bounds/renewal, non-user release, natural-language resume or healthy slow waits is chosen by M1.

## Requirement and authored coverage map

All rows below mean **authored; unqualified**, not shipped or verified behavior.

| Requirement | Source anchors | Authored regression anchors |
|---|---|---|
| REQ-GOAL-001 ownership/absence | `conversation_goal_owner_is_ordinary`, `Database::conversation_goal` | `wrong_owner_role_and_fabricated_user_channel_roll_back_every_row`, `no_goal_admission_cannot_acquire_a_later_goal` |
| REQ-GOAL-002 all-writer CAS | `GoalMutation::advance`, `Database::mutate_conversation_goal`, owner/version primary key | `all_actor_writes_compete_on_observed_version`, `cleared_goal_does_not_allow_id_or_version_aba` |
| REQ-GOAL-003 atomic history/retry | goal history action uniqueness; transaction-scoped actor inserts | `normalized_history_roundtrips_all_actors_and_exact_retries`, `failed_history_insert_rolls_back_both_actor_rows`, `agent_locator_must_name_a_real_assistant_tool_call_for_both_actor_roles` |
| REQ-GOAL-004 actor distinction | `GoalActor`, `insert_actor`, `conversation_goal_actor_source` | all-actor round trip, wrong-role/channel tests, invented-call and user-message-as-agent tests |
| REQ-GOAL-005 binding/report fence | `GoalExecutionBinding`, `bind_goal_at_admission_tx`, `conversation_goal_report_fence` | `admitted_binding_is_immutable_and_late_report_cannot_rebind_to_edit`, `raw_report_row_cannot_reference_another_owner_or_wrong_version`, `binding_rejects_cross_owner_and_already_materialized_work`, `fabricated_fresh_report_cannot_release_suspended_or_completed_state` |
| REQ-GOAL-006 edits do not activate | `GoalChange`, `GoalMutation::advance` | `completion_cannot_be_reopened_by_an_ordinary_edit`, `later_suspend_wins_even_with_unchanged_objective` |
| REQ-GOAL-007 owner-scoped retention/deletion | owner cascades and owner-existence no-delete guards; evidence locators | `real_migrations_user_goal_clear_and_supported_hard_delete_do_not_pin_aggregate`, `real_migrations_no_goal_binding_does_not_pin_supported_hard_delete`, `history_and_binding_rows_cannot_be_rewritten_or_deleted` |
| REQ-GOAL-008 relational/time/upgrade | staged `schema.sql`, checked `_us` timestamp | `timestamp_constraints_reject_text_and_negative_integers`, `staged_schema_applies_after_real_migrations_without_rewriting_existing_data` |

Core tests live in `crates/phoenix-core/src/domain/conversation_goal/tests.rs`; DB tests live in `crates/phoenix-db/src/conversation_goals/tests.rs`. The isolated DB fixture tests storage constraints, while separately authored tests open the full migration substrate and invoke supported delete. The authored restart test exercises only persisted binding/history retry after reopening its disposable file; it is not context-continuation or startup-runtime qualification.

## Review disposition and remaining gates

Source reviewers found and the writer addressed: a fabricated fresh inactive report accepted by the pure transition; insufficient SQL owner/report-fence correspondence; agent locators accepting user messages/invented calls; and goal foreign keys/immutable-delete triggers pinning supported deletion. Later-stage rollback and full-migration deletion tests were added. These fixes remain unexecuted.

Parent-owned integration gates: unique migration/ADR reservation; actual trusted adapters and atomic admission capture; review of final source pin; hosted compile/test/format/check qualification. No further implementation writer, generic authorization/event/wake framework, shared-main mutation, merge, deployment, release or production migration is part of this increment.
