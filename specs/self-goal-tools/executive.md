# Own-conversation Goal tools — executive

## Implementation

Ordinary agents have two separate own-conversation tools: `create_goal { objective }` and `enable_auto_continuation {}`. Creation is create-only and never enables continuation. Enabling changes only the existing preference; it does not Start, dispatch, rearm, or reactivate a Goal. Coordinator and remote management boundaries are unchanged.

The runtime derives the ProductConversation, transcript, execution, generation, and canonical tool identity. Effects and refusals commit at the existing durable tool-round checkpoint. Migration 155 records immutable execution admission and the human-disable epoch; it works with production-default Goal execution/control policies disabled. Admission rows cascade with their owning resources.

| Requirement | Implementation | Regression coverage |
|---|---|---|
| REQ-SGT-001 own ordinary authority | Runtime discovery, strict parser, persisted source/execution validation | Runtime discovery/real producer assertions; `forged_pending_source_is_not_an_argument_error_or_a_finalized_success` |
| REQ-SGT-002 explicit enable | Enable-only preference write; create does not enable | `no_policy_installers_allow_canonical_enable_then_create`; OFF/enable/create runtime journey |
| REQ-SGT-003 create only | Existing Goal or OFF refuses; canonical first application classifies mixed enable/create rounds and refuses all own-management intents without mutation | `mixed_self_goal_rounds_refuse_every_intent_without_mutation`; `database_runtime_mixed_self_goal_round_refuses_both_intents_in_either_order`; separate-checkpoint same-turn success |
| REQ-SGT-004 control and disable fencing | Admission observes control presence/version and disable epoch | First control initialization, Stop/Release ABA, repeated false/disable ABA, stale generation/transcript, pause and Close tests |
| REQ-SGT-005 atomic checkpoint | Typed pending results flow through canonical, provider, fork, and terminal checkpoints | Reducer pending-round tests; `database_runtime_self_goal_tools_finalize_before_terminal_settlement_without_rebinding` covers prior pending Goal refusal while cancelling a following tool, durable/runtime Idle timestamp equality, and terminal authority release |
| REQ-SGT-006 immutable replay | Canonical receipts bind retained outcomes to source evidence | `real_lost_acknowledgements_reopen_and_replay_without_duplicate_effects`; retained-shape corruption and disable-after-success replay tests |
| REQ-SGT-007 no execution authority | No dispatch or budget/scope expansion; current-turn binding retained | Effect-count and Goal-binding assertions across DB/runtime journeys; independent source review |
| REQ-SGT-008 existing boundaries | Existing Goal/preference owners reused; Coordinator and remote APIs unchanged | Coordinator Goal suite and full hosted regressions; `deleting_ordinary_conversation_cascades_its_runtime_admission` |

DB regressions are in `crates/phoenix-db/src/conversation_goals/writer_tests/self_management.rs`; runtime integration is in `crates/phoenix-ide/src/runtime/executor/self_goal_tests.rs`. The runtime cancellation tests distinguish cancellation of the current tool (ordinary typed cancellation) from refusal of a prior pending own-Goal intent at the terminal checkpoint.

## Mixed-round correction

A later external review of merged [PR #1046](https://github.com/scottopell/phoenix-ide/pull/1046#discussion_r4501397065) exposed an uncovered REQ-SGT-003 violation: a single transaction applied enable before create, allowing both to succeed in one checkpoint. The corrective source classifies the complete pending own-management set only after exact retained-round replay. It verifies all pending sources before applying results and passes a typed separate/mixed admission to finalization. Mixed rounds return `mixed_self_goal_operations` unless an existing admission or human-control refusal takes precedence. No migration or scheduler is added.

The regression inventory includes OFF/ON × both orders at the DB and real runtime producer/reducer/checkpoint boundaries, unchanged preference/Goal/current-turn binding, immutable refusal replay after later preference changes, changed-source rejection, malformed later-source rollback, and before/after canonical-commit cuts with replay evidence. The successful enable-only then create-only journey remains covered. The runtime fixture starts from a constructed tool-execution state and uses an empty successful mocked provider follow-up; it does not establish live-provider or native/macOS acceptance.

[Corrective PR #1048](https://github.com/scottopell/phoenix-ide/pull/1048) carries the exact-head hosted-check and independent-review receipts; its checks and qualification comment are authoritative for execution status, rather than this coverage inventory. Original green CI and earlier clean source reviews do not resolve the later mixed-round finding. Publication remains gated on the qualified correction and an explicitly selected feature-plus-fix artifact through the authorized delivery owner.

## Verification and delivery

[PR #1046](https://github.com/scottopell/phoenix-ide/pull/1046) carries the hosted qualification and immutable source-review receipts. Its [checks](https://github.com/scottopell/phoenix-ide/pull/1046/checks) are the authority for the exact-head test result; the coverage inventory above does not substitute for passing checks. Qualification includes Rust build, Clippy, musl, four Rust test shards, codegen, UI/specs, E2E, assistant-finalization geometry, and task validation. No local tests, builds, dependency installation, or native/macOS qualification were performed under the host resource boundary.

Independent source review found no actionable defect in `c31efd70cdbcd8adf061686b15cef7ae116153ab..cece548c83e249e44cb048ecc063b248316a186f`. The subsequent runtime-fixture corrections and terminal-coverage extension through `98e1048026a0e4d41f6a4cce7677519c10de985d` received separate exact-delta reviews with no actionable findings. Source review is distinct from hosted execution and external GitHub approval. The original PR has since merged; its historical receipts do not establish corrective qualification or runtime delivery. Corrective integration and deployment remain with the authorized owners.

Decision: [ADR-242](../adrs/242_execution-bound-own-goal-creation-and-enable.md). Normative contracts: [requirements](requirements.md) and [checkpoint behavior](self-goal-tools.allium).
