# Goals simplification — sole-writer review packet

## Review state and scope

Candidate source only; **not compiled, executed, independently approved, qualified, shipped, or activated**. Integrator `b477b7e7-0e64-4c4f-bba1-d0b101df98e9` owns independent review and authorized qualification. No new CI, build/test worker, service, merge, deployment, or activation was started for this continuation.

- Sole writer/worktree: `a52c58d8-3dcb-42c5-82fe-585e6126dcb0`, branch `work/durable-goals-m1-storage`.
- Worktree base HEAD: `feea29c3f245d63ebab5df966c1ca02e026e9928`. The integrator's previously qualified `bb55312` remains separate evidence, not qualification of this diff.
- Authentic approval recovered from the original `user_api` row `@transcript:dcba54e9-665d-4db2-8891-a9b5a3586185#message-772be0f8-a7c4-46d1-b082-60bf59d0a01f`, approving the proposed simplification in message `84e7864f-9e97-443a-9365-f60f9db55c2a`. Parent's relayed approval is `@transcript:6f7b2368-a4a7-410e-9b50-c5c92783f904#message-106841c2-f3bf-49e1-b7ea-1319a4f04086`.
- Classification correction recovered from the actual parent message `4fc25fd5-4174-4c84-a5e9-0f122b824b35` and reaffirmed in the current transcript: real allowance exhaustion is typed `Exhausted`; stale/blocked admissions are not exhaustion; explicit Complete/Blocked is not overwritten.
- Issue #806 generated body was read on continuation. Normative Goals requirements and ADR-186 were consulted. No specification or compatibility policy is changed by this packet.
- All 11 pre-existing stashes are preserved, including `782a5…`. Snapshot: `/tmp/goals-simplification-stashes-2f5.txt` (server-local evidence only).
- Migrations 132–136 and generated wire artifacts are untouched. The `96670864951a` open seam is not integrated; `/tmp/goals-seam-966.diff` remains separate, SHA-256 `7b98893bfc2cbea7199ecf2de22f202a580edc0b9cc64c39d007df891fb1a2b9`.
- Public Start remains default OFF. The removed case is a Coordinator itself executing ordinary Goal Start, not Coordinator management of ordinary targets.

## Field authority map

| Semantic value | Authoritative owner | Distinct downstream use; no second live owner |
|---|---|---|
| Runtime LLM attempt generation and handle | Existing `ConversationRuntime` generation/attempt/handle | Common `LlmCompletion` transports its generation with the ordinary `LlmOutcome`; stale completion is rejected before retiring the current handle. |
| Current direct-turn identity/generation | Existing `active_direct_turn` and durable turn/workflow claim | No separate active Goal request ID/generation/attempt slot. The admitted receipt carries historical intent generation separately from the current claim generation. |
| Logical Goal request identity and immutable charge | Normalized request ledger; non-Clone `AdmittedGoalRequest` receipt | Optional receipt is moved through the existing completion channel and then into the in-progress canonical effect. A retry/reconstruction reuses a logical request unless the typed cause is a genuinely new `AfterCheckpoint`. |
| Provider response content and replay update | Ordinary `LlmOutcome` and existing pending replay owner | No separate Goal result envelope/channel or response payload. Both ordinary and Goal persistence call the same canonical transaction; optional accounting borrows those exact arguments rather than duplicating them. |
| Current runtime state | Existing `state` / committed state projection | Pending checkpoint `source` is the prior persisted slot being fenced, not a second proposed/current state. Final selected state is returned by the canonical transaction. |
| Goal definition, version, allowance, request count | Existing immutable Goal snapshot plus normalized request rows | UI/runtime observations are fenced snapshots, not a second counter. Host exhaustion is selected only after explicit-report application and only while the live Goal remains Underway. |
| Stop and queued-input authority | Existing control binding and normalized accepted-input rows | Existing dispatch preflight and first-poll fences remain responsible. A denial never fabricates Exhausted or refunds a charge. |
| Terminal outcome and projection | Existing direct-turn terminal obligation and settlement owner | No Goal terminal representation. Canonical response/ledger TX stages the existing obligation for a selected Idle decision; source projection stays unchanged until ordinary terminal settlement completes. |

### Immutable Goal fence review correction

The literal continuing `Underway` report premise was retracted by the integrator after source verification: declared `GoalReportOutcome` values are Blocked, Complete, and Exhausted; an explicit return to Underway is a separate settled State interaction from Blocked. No outcome, domain, history, or compatibility change is included here.

No supported same-execution case needs an independently refreshed request Goal version. `select_intent` derives every causal successor/summary from the original admitted Goal fence, rejects a retained intent with a different fence, and has no `current_goal` parameter or refresh projection. `require_live_request_tx` compares both the immutable execution observation and the exact current Goal fence before any live request effect. Context admission performs this check before appending even a new charge. Recovery and its exact own-commit classifier use that same check. A changed status/version or replacement is not permission to upgrade an old execution. Retained committed checkpoint/history classification remains independent of admission for a new live effect.

The authored real Blocked report → terminal settlement → explicit settled State Underway → new typed Start/materialization regression checks that the original execution cannot regain authority, its binding/history remains unchanged, and the newly accepted binding captures the new version without refilling the existing per-goal ledger. The unreported Underway yield test now explicitly checks the successor's unchanged admission fence. One earlier DB-only recovery subcase attempted a settled State Complete while the execution was live (which is Busy, not an admitted mutation); that unsupported fixture branch was removed explicitly, not replaced with fabricated data. Complete precedence and exact retained classification remain covered by the actual report journey and `conversation_goal_complete_report_checkpoint_cuts_replay_whole_bundle_after_bound_goal_changes`. No regression file was deleted.

## Source / MOVE ledger

| Source area | Change / destination | Retained behavior |
|---|---|---|
| `runtime/executor/goal_request_checkpoint.rs` dedicated completion receiver and active Goal slot | Removed in favor of common `LlmCompletion { generation, outcome, admitted: Option<_> }` in existing LLM receiver/driver | Generation checks, current-slot/ownership checks, affine receipt, retry handling. |
| Dedicated Goal no-tool/tool response wrappers and `GoalResponseMessage` | Removed. Optional accounting calls live in existing `AuthoritativeEffect::PersistMessage` and ordinary ToolRound effect; publication rejoins their existing common tails | Ordinary opening projection, assistant/tool publication, token forwarding, persisted message reservation, trusted results. |
| `conversation_goals/request_checkpoint.rs` independent response transaction | Moved into neutral `provider_replay::persist_canonical_state_and_replay_input`; request checkpoint remains an optional accounting/selection hook | Canonical assistant/tool rows, report application, replay, request checkpoint, and decision committed together; exact uncertain-commit classification. |
| `phoenix-db/lib.rs` ordinary replay/state writer | Calls that same canonical transaction with no accounting | Ordinary clear/write and replay behavior; no Goal admission or automatic scheduler added to ordinary requests. |
| Runtime no-tool yield | Existing transition/effects continue with RequestLlm after successful accounting; new causal slot timestamp is selected even if state remains LlmRequesting | Same active execution; no inferred progress, fake User message, Idle window, or extra terminal obligation. |
| Selected Complete/Blocked/Exhausted | Canonical TX calls existing `WorkflowRepository::persist_terminal_obligation_tx`; existing runtime `settle_pending_direct_turn` completes it before state publication | Terminal cause/receipt are not cleared before successful commitment/settlement. Failure requests database recreation; manager prioritizes ordinary retained terminal obligations. |
| `goal_start/recovery.rs` blanket refusal of a completed predecessor | Existing reconstruction/context-admission path admits a typed `AfterCheckpoint` successor only for the persisted requesting-attempt-one decision, with retained predecessor checkpoint and canonical assistant/results | Same execution, exactly one new immutable charge, workflow/turn generation CAS and history append. Commit classification probes the newly admitted intent and its predecessor, not the old charge. |
| Coordinator ordinary Start authority in management DB/service | Removed impossible combination and its Coordinator-executes-ordinary-Goal tests; replaced with explicit ordinary-Start rejection | Normal Coordinator-to-target management remains. Original regression files are retained, not silently deleted. |
| Existing control/management/provider tests | Adapted to the common completion envelope; independent behavior expectations preserved | Stop, first poll, accepted input, generation, replay, and normal management boundaries. |

## Executable journey obligations included in source

These are authored tests, **not execution evidence**. The integrator must obtain passing exact-packet results through the authorized existing qualification lane.

- `conversation_goal_actual_no_tool_yield_automatically_reaches_explicit_terminal_report_in_same_execution`: ordinary yield dispatches the same-execution successor; explicit Complete and Blocked at the allowance boundary win. No fake User message, extra request, or yield terminal obligation; state watcher and actual terminal owner are asserted. Test triggers reject Idle projection before terminalization or terminalization without the existing obligation.
- `conversation_goal_actual_blocked_resume_requires_new_start_not_version_refresh`: actual Blocked tool report settles the original execution; trusted host applies the existing State interaction at the settled boundary; old execution context/recovery refuses without a charge or send; only new typed Start/actual claim/materialization establishes the new version binding, retaining the original binding and finite per-goal accounting.
- `conversation_goal_actual_automatic_yields_exhaust_once_without_inferred_progress_or_refill`: two actual ordinary responses consume the fixed allowance and produce one typed Exhausted transition; real typed Start re-admission is refused; Stop/release does not refill; no further provider call or charge.
- `conversation_goal_actual_allowance_terminal_failure_reopens_through_existing_manager_before_idle`: inject terminal-owner failure after canonical commit, retain source projection/receipt/cause/obligation, reopen the database, and use the real existing RuntimeManager constructor to settle the obligation before exposing Idle, with zero recovery provider calls/charges.
- `conversation_goal_reconstruct_after_committed_yield_admits_distinct_charged_successor_once`: crash seam after yield checkpoint and before next admission; normal race, pre-commit cut, and uncertain committed cut. Checks same execution, typed distinct AfterCheckpoint ID, one charge, unchanged historical intent/generation, and repeated reconstruction refusal.
- `conversation_goal_checkpointed_slot_without_canonical_predecessor_or_fresh_attempt_cannot_reconstruct`: missing canonical response, retry-attempt state, terminal obligation, Stop, and accepted input refuse reconstruction without a new charge. Stop/accepted-input refusal leaves the Goal Underway rather than Exhausted.
- Existing actual token-forwarder-before-canonical-completion test is retained.
- Existing Stop-after-admission/before-first-poll, in-flight provider cancellation, in-flight tool cancellation, Stop/release-between-polls, and queued-input fencing tests remain in `goal_round_tests/control_dispatch.rs` and the Start/checkpoint test modules.
- Existing report mutation/replay/idempotence, retained-request current-generation, ordinary policy-binding, and default-off constructor-recovery tests remain proof obligations.

## Checks performed and remaining gates

Performed only lightweight source checks: rustfmt parse/format check over changed Rust files, `git diff --check`, retired-symbol searches, stash inventory, migration/generated-file exclusion check, source-level transaction/driver/recovery inspection. No Cargo check, tests, browser journey, CI, release build, service launch, or new resource allocation was performed.

Remaining gates: independent review of the entire frozen source packet and this authority/MOVE ledger; compile and focused owning-crate tests; actual automatic journey/recovery/Stop/queue/first-poll execution with observable ordering; ordinary no-accounting regression coverage; then integrator-authorized wider qualification. Any results for an older prefix remain older-prefix evidence. Do not turn this source packet into a PASS or activate Start from source inspection alone.

## Qualification failure correction — run 39700190279

The integrator's exact-`ae74b01dd` hosted run failed compilation before any focused journey ran. Retained artifact `12392063206` reports two undeclared production helper calls and a test receiver referring to the `fixture` function rather than a value. This is owned failure evidence, not a runtime PASS; the previously qualified prefix remains separate.

The bounded correction uses the actual declared contracts, without aliases or new mutation machinery:

- `current_tx(&mut GoalTx, &ProductConversationId) -> Result<Option<GoalSnapshot>>` replaces the nonexistent `snapshot_tx`; `round::db_error` retains the existing GoalDbError-to-DbError mapping.
- `apply_goal_tx(&mut GoalTx, &GoalMutation, Option<&str>, GoalTimestampUs, GoalWritePolicies) -> Result<GoalHistoryEntry>` replaces the nonexistent `apply_tx` with the required five arguments. Its `Some(transcript)` branch admits external Create/State only at a settled boundary, so it must not be used for host allowance settlement in a still-active execution. This internal host call uses `None` only after the canonical transaction's live-owner/generation/exact-source-slot checks and an immediate `require_live_request_tx` recheck of exact immutable/current Goal fence, Underway state, target lifecycle, control binding and accepted-input priority. Both remain in the same BEGIN IMMEDIATE transaction. GeneratedHost action identity, exact expected version, policies, timestamp, and existing error mapping are preserved. Explicit non-Underway reports return before this host write; retained classification with no write policies never performs the mutation or the live recheck.
- The test obtains the actual `WorkflowRepository` from `db.workflow_repository()` and calls its declared `load_terminal_obligation(TurnAuthorityId) -> DbResult<Option<DirectTurnTerminalObligationInput>>`. Existing projection/generation/replay assertions are unchanged.

No owner, field, code block, assertion, or migration is moved by this correction. No new feature, helper alias, domain outcome, compatibility behavior, or local/CI qualification is introduced. Formatting/diff checks alone do not establish that the corrected packet compiles or passes its journeys; the integrator owns the next exact-source review and authorized qualification.
