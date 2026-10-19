# Durable Conversation Goals — Immutable Storage Foundation

## Status

M1 source is authored as a dormant core/DB increment under the approved immutable-objective amendment. The schema remains staged in `crates/phoenix-db/src/conversation_goals/schema.sql`, **not registered in migrations**; normal databases do not acquire these tables. No runtime caller, HTTP endpoint, goal tool, UI/SSE, automatic follow-through, Stop integration or watch change is installed.

The original mutable-objective source at `4a71cba3976a2c3f1f02c1e1ef6931e27887907c` is preserved historical evidence, not the selected contract. `f6604cd89afdf93071d482ab0d38593c78e114d6` records the amended approval brief, not the completed feature. Subsequent source implements the immutable model. No compatibility bridge/backfill for the stale DDL is needed: it was never registered or applied.

Validation is source-only: existing standalone rustfmt and Git whitespace checks. Earlier independent read-only source reviews identified useful safeguards retained by the amendment; no new review agents were commissioned. **No compiler, Cargo, SQLite test, migration, browser, VM, hosted CI or production execution has qualified this increment. Authored tests are not passing-test evidence.** Numbered migration and ADR integration remain parent-owned and gated.

## Selected model

- Ordinary stable ProductConversation owns each immutable goal identity and objective; no WorkScope is required. Objective is stored once on the immutable identity row, never repeated as writable history data.
- Goal-specific immutable status-history rows are the only current/history authority. Current identity/status is the indexed latest owner version. Creating a new identity atomically supersedes that current identity, preserving prior identities and factual outcomes rather than editing text, clearing first or fabricating success.
- There is one monotonic owner/status version across identities. Every actor uses observed-version CAS; exact action retries compare the original full payload/actors/binding and return the original history result even after terminal settlement or supersession.
- Underway and Blocked are nonterminal; explicit Blocked → Underway preserves identity and objective. Complete and Exhausted are distinct terminal outcomes: exhaustion is unfinished, not success. Neither terminal identity can be reopened or receive another status change. A deliberate new goal has a different identity.
- There is no objective-edit, objective-revision, clear API, lifetime/shared-budget schema or wait framework. Existing Bash same-handle waits are unchanged.
- Concrete requester and applier occupy normalized actor rows. Agent validation checks owner/kind/workflow/transcript membership and an actual typed assistant ToolUse. This is consistency, not authentication or proof that the tool belongs to that exact turn; exact invocation assignment must come from trusted runtime context.
- An immutable binding stores either Goal or NoGoal at an observed owner version, indexed by durable turn. Captured goal identity plus version fences stale reports after status changes or supersession, including identical replacement text.
- Immutable source locators may outlive deleted source rows. Goal data cascades with its ordinary owner; standalone history/actor/binding deletion is guarded while that owner exists. Full-schema supported hard-delete tests are authored for superseded/terminal goals and NoGoal-only bindings, not executed.

## Explicit runtime integration limits

Storage is **not** Stop protection or start permission. A replacement's Underway marker does not release user Stop/security. Dormant Suspended remains solely a stored fencing representation with no status-release operation, not a commissioned resume policy. M3 must identify and enforce the actual durable runtime Stop fence across replacement before activation.

The capture primitive is crate-private and dormant. Present eligibility checks **cannot establish co-creation in the admission transaction**. M3 must capture the immutable goal context with actual turn admission before model generation, and never look up the current goal when a queued report dispatches or retrofit a current goal onto an older unbound turn.

A NoGoal binding on blocked execution cannot be silently converted into a goal-bearing observation when a structured answer resumes it. Known answer recovery needs an explicitly matching host status mutation and valid report/admission integration, not an invented mid-turn rebind or an extra competing turn. Question dismissal is not automatic goal recovery. No such integration is installed here.

## Requirement and authored coverage map

All rows mean **authored; unqualified**, not shipped or verified behavior. There are 9 pure domain and 21 DB tests authored; zero have been run or qualified in this lane.

| Requirement | Source anchors | Authored regression anchors |
|---|---|---|
| REQ-GOAL-001 immutable ordinary ownership/current identity | `conversation_goal_ids`, `conversation_goal_owner_is_ordinary`, `Database::conversation_goal` | `wrong_owner_role_and_fabricated_user_channel_roll_back_every_row`, `immutable_objective_is_not_overwritten_by_create_reuse_or_raw_updates`, `superseded_goal_identity_cannot_be_reused_or_bypass_owner_version` |
| REQ-GOAL-002 all-writer owner CAS | `GoalMutation::advance`, `Database::mutate_conversation_goal`, owner/version primary key | `all_actor_writes_compete_on_observed_version`, `objective_cannot_change_under_current_identity_and_supersession_needs_observed_version` |
| REQ-GOAL-003 atomic history/retry | action uniqueness; transaction-scoped identity and actor inserts | `normalized_history_roundtrips_all_actors_and_exact_retries`, `failed_history_insert_rolls_back_both_actor_rows`, `agent_locator_must_name_a_real_assistant_tool_call_for_both_actor_roles` |
| REQ-GOAL-004 actor distinction | `GoalActor`, `insert_actor`, `conversation_goal_actor_source` | all-actor round trip, wrong-role/channel, invented-call and user-message-as-agent cases |
| REQ-GOAL-005 execution/report fence | `GoalExecutionBinding`, `bind_goal_at_admission_tx`, `conversation_goal_report_fence` | `admitted_binding_is_immutable_and_late_report_cannot_rebind_to_replacement`, `raw_report_row_cannot_reference_another_owner_or_wrong_version`, `no_goal_admission_cannot_acquire_a_later_goal`, `replacement_changes_identity_and_fences_admitted_report_even_with_same_text` |
| REQ-GOAL-006 immutable identity/status terminality | `GoalChange`, `GoalDisposition`, `GoalMutation::advance`, `conversation_goal_history_transition` | `complete_and_exhausted_are_distinct_terminal_outcomes_and_cannot_reopen`, `blocked_underway_roundtrip_keeps_objective_and_fences_previous_execution`, `terminal_success_and_exhaustion_roundtrip_cannot_reopen_but_allow_new_identity`, `suspended_representation_rejects_fresh_report_or_status_release_without_claiming_stop_enforcement` |
| REQ-GOAL-007 scoped retention/deletion | owner cascades and owner-existence no-delete guards; evidence locators | `real_migrations_user_goal_supersession_and_supported_hard_delete_do_not_pin_aggregate`, `real_migrations_terminal_and_superseded_goals_do_not_pin_supported_hard_delete`, `real_migrations_no_goal_binding_does_not_pin_supported_hard_delete`, `history_and_binding_rows_cannot_be_rewritten_or_deleted` |
| REQ-GOAL-008 relational/time/upgrade | staged `schema.sql`, checked `_us` timestamp | `timestamp_constraints_reject_text_and_negative_integers`, `staged_schema_applies_after_real_migrations_without_rewriting_existing_data` |

Core tests live in `crates/phoenix-core/src/domain/conversation_goal/tests.rs`; DB tests live in `crates/phoenix-db/src/conversation_goals/tests.rs`. The isolated fixture tests storage constraints; separately authored tests open the full migration substrate and invoke supported delete. The restart case exercises persisted binding/history retry only, not startup runtime recovery or context-continuation qualification. Earlier edit/clear regression cases were rewritten as direct supersession, immutable-objective, terminality and owner-version cases rather than silently discarded.

## Handoff and remaining gates

Task 98021 carries the approved amendment and status. Parent-owned gates: unique migration/ADR reservation and registration; actual trusted adapters, Stop enforcement and atomic admission/recovery capture; review of final source pin; hosted compile/test/format/check qualification. No further implementation writer, generic authorization/event/wake framework, shared-main mutation, merge, deployment, release or production migration belongs to this increment.

ADR rationale handed to parent: immutable objectives remove the need for objective-revision fencing and edit/reopen policy. New identity plus the existing monotonic owner version fences replacement races; objective-once storage makes mutation structurally unavailable. Goal-specific history preserves terminal outcomes and exact actor retry evidence. No numeric ADR reservation is claimed by this lane.
