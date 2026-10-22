# Execution-owned cancellation and terminal outcomes

## Commission and boundaries

Implement a correct-by-construction correction for helper timeout being recorded as parent cancellation, plus the user-approved general review guidance. Approval starts isolated development, not merge, deployment, activation, historical repair, or unrestricted shared-core edits. Keep mobile PR #964 independent.

Before source edits, establish an isolated worktree and coordinate the integration base and path ownership with Goals owner b477b7e7-0e64-4c4f-bba1-d0b101df98e9. There must be no parallel writer to its shared runtime/core/DB paths. Re-resolve current qualified/source heads rather than using historical review snapshots. Host 5d57fcc6 admits heavy validation. If ownership cannot be handed off or partitioned safely, stop at the contract/design deliverable.

## Evidence and invariant

Incident: product conversation 58cff20d-81c3-4655-b9d8-61fe615bda28, transcript 0ace32b0-dc4a-4d4d-ba3c-0271131841e9, deployed source 23badb8be4bc8d05a7e1c49c5367ceb14ebccd92. At 2026-10-21T00:03:00Z a helper timeout emitted UserCancel(Timeout); the parent drained helpers then continued, paused for a question, resumed, and reached Idle at 01:07:30.764829Z. Turn 7586 nevertheless settled Cancelled. Two distinct watch occurrences, direct_turn 7586 and interaction_response 55b3335b-f9d9-4c46-90e6-3aa7e4acf111, inherited cancellation. Trace 39886102ddbe8f84f07faa90d052993d independently confirms committed Idle/cancelled settlement. The child itself ended Failed(TimedOut).

The in-memory classifier marks any UserCancel as parent cancellation. A separate durable publication path can override terminal outcome using conversation-keyed execution_cancel_observations; its contribution to this incident is unproved.

User-approved invariant: a helper timeout can stop the helper and return control to its parent; it cannot by itself produce a cancelled parent. User-approved review principle: preserve distinctions required by downstream decisions.

## First checkpoint: contract before runtime implementation

Read current normative cancellation, subagent, durable-turn, Goals, watch, and compatibility contracts. Produce a concise forensic/design document in the isolated worktree, distinguishing verified evidence from inference and comparing deployed/current/integration source. Present the exact proposed contract and remaining policy decisions for review before runtime changes.

Define typed operations distinguishing initiator, cause, exact target, scope, and terminal result without permitting arbitrary invalid combinations. Separate recoverable helper draining from execution stopping. Specify the authoritative Stop/completion serialization point, execution identity across pause/resume and continuation, generation fencing, and restart ownership. Preserve Goals host decisions before Idle/watch completion and independent Stop fencing; publication cannot gate Stop acceptance. Do not claim rollback of external effects.

State Cancel, Close, and Stop semantics explicitly; resolve policy ambiguity before implementation. Define terminal-fact identity versus source-occurrence and event identity. Prefer one execution-level terminal notification with retained contributing sources, but obtain explicit agreement before changing coordinator-input/coalescing behavior.

Specify bounded schema/wire migration and legacy unresolved-observation treatment under the compatibility contract and a new ADR where required. Do not silently rewrite historical outcomes or add implicit downgrade/recovery guarantees.

## Approved AGENTS.md guidance

Add under the existing correct-by-construction principle:

### Preserve distinctions required by downstream decisions

Before reducing lifecycle history to a flag or status, ask:

> Can two different histories produce the same stored value but require different downstream behavior?

If yes, the representation has discarded necessary information. Preserve the distinction through typed states, variants, or causal references, and test the contrasting histories. Downstream code must not reconstruct it from unrelated state or conventions.

For example, stopping a timed-out helper and accepting a user Stop for its parent cannot both become an undifferentiated cancellation_seen: bool: the parent may complete normally in the first case but must stop in the second.

This is not a blanket ban on Booleans or a requirement to retain all history.

## Implementation after contract review and ownership coordination

Use existing runtime/turn/watch machinery, not another scheduler, response pipeline, or event framework. Replace ambiguous cancellation input and sticky terminal inference with exact execution-scoped causal authority. Persist accepted Stop as required by the approved lifecycle contract; prevent child/tool teardown from furnishing a parent-cancellation witness. Make watcher publication project the committed settlement rather than override it. Thread provenance through persistence/wire/UI without inventing reasons for historical rows.

Likely boundaries: sm_event/sm_state/effects and transitions; runtime executor timeout handling and terminal classification; authoritative direct-turn and interaction-response settlement; coordinator_watches; event wire types and owning UI components. Re-resolve actual symbols before editing. Do not absorb unrelated Goals failures or modify its approved domain behavior.

## Proof obligations and acceptance

- Exact incident journey: helper timeout, recovery, question pause/resume, normal parent completion; no parent Cancelled.
- Genuine Stop before provider dispatch, during execution, question wait, and helper drain remains authoritative.
- Deterministic Stop-versus-completion ordering; accepted stopping cannot resume normal dispatch.
- At most one terminal fact per execution; durable turn, source occurrences, watcher and UI agree.
- Nested/multiple helpers, late completion, duplicate fan-in, stale attempts and generations cannot contaminate parent outcome.
- Crash/replay cuts around Stop acceptance, continuation, request checkpoints, terminal settlement, outbox and acknowledgement preserve authority and idempotency.
- Actual error/exhaustion and explicit Complete/Blocked retain their own outcomes.
- Recoverable helper timeout cannot change otherwise successful parent settlement into cancellation (property/state-machine tests).
- Historical records remain intact; correction, if separately authorized, is explicit and linked.

Use focused tests and independent adversarial exact-HEAD review first, then host-admitted qualification. Report source review, compilation, test execution, and deployed behavior separately. Acceptance requires proof of the invariants, not a label change, flag reset, or hidden duplicate card.

## Delivery

Logical commits for guidance/contracts, lifecycle/durability, and publication/provenance as appropriate; each must preserve a coherent integration boundary. Provide exact-head review evidence, migration limitations and unresolved decisions. No merge/deploy approval is implied by this task.
