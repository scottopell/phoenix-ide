# Deletion-first durable result subscriptions

## Status

Brainstorming only. No production implementation is approved.

This task replaces its original design snapshot. The earlier proposal to delete durable direct-turn authority and replace it with a new conversation-input admission model is superseded by the P0 ProductConversation direction. Existing durable direct-turn authority remains part of the execution model.

GitHub Project coordination item: **Wake redesign — explicit durable-result subscriptions** in [Phoenix Product Delivery](https://github.com/users/scottopell/projects/1).

## Goal

Delete the legacy wake-specific lifecycle while preserving these product journeys:

- waits are explicit; ordinary Bash, tmux, and subagent use never enrolls implicitly;
- Bash, tmux, and subagent terminal results arrive without polling;
- one explicit subscription produces one correlated durable result;
- busy conversations remain safe;
- cancellation suppresses automatic continuation without killing the resource;
- Bash restart loss is typed;
- tmux may survive Phoenix restart and be re-probed;
- subagent result truth remains in the existing child/parent state machines;
- any retained automatic continuation enters only through existing durable direct-turn/runtime admission;
- ProductConversation Close can settle pending subscriptions through one typed operation.

Optimize for the least production code and the fewest overlapping authorities.

## Governing design rule

Use the fewest authorities that still represent genuinely different facts. No two authorities may answer the same semantic question.

## Accepted authority map

| Fact | Authority |
|---|---|
| ProductConversation identity and continuation membership | topology-derived root plus `continued_in_conv_id` |
| Open/History lifecycle and whether new delivery is allowed | ProductConversation lifecycle and root-scoped Close authority |
| Conversation execution meaning | existing conversation state machine and persisted `ConvState` |
| Accepted turn identity and runtime ownership | existing durable direct-turn authority |
| Resource ownership and retirement | WorkScope plus each resource owner's typed retirement operation |
| Bash terminal truth | `BashHandleRegistry` / handle tombstone |
| tmux terminal truth | tmux registry plus OS tmux |
| Subagent terminal truth and parent fan-in | existing child and parent state machines |
| Subscription observation, terminal result, and delivery disposition | reduced durable-result subscription mechanism designed by this task |
| Transcript content | canonical persisted messages |
| UI and SSE | post-commit projections only |

ProductConversation owns the complete Close journey. Each subsystem owns one typed operation and its evidence.

## ProductConversation integration contract

The ProductConversation owner accepted this contract for settlement PR 2:

1. Committing the active root-scoped Close obligation closes admission for every new conversation delivery to that ProductConversation.
2. Close admission and result delivery serialize at one durable transaction boundary. Commit order decides the race.
3. ProductConversation exposes a typed delivery-admission decision derived from existing lifecycle authority, conceptually:
   - `Open`
   - `ClosedByClose(close_attempt_id)`
   - `History`
4. Wake must consume that typed decision. It must not interpret individual Close phases.
5. A result suppressed during Close remains suppressed if Close is later cancelled.
6. ProductConversation PR 2 owns cross-domain orchestration and acceptance tests.
7. Wake owns one exact-ID, idempotent settlement operation and remains authoritative for its result and delivery evidence.
8. WorkScope retirement—not subscription cancellation—stops or removes resources.

### Race examples

```text
Result delivery commits before Close
  -> result is delivered into the transcript
  -> Close preserves it

Close commits before result delivery
  -> terminal resource result is recorded
  -> conversation delivery is suppressed permanently

Close is later cancelled
  -> suppressed result is not replayed into the reopened conversation
```

## Minimal subscription contract

The subscription mechanism must represent these facts structurally:

```text
subscription identity
resource identity and correlation
one typed terminal result
one delivery disposition
```

Conceptual delivery dispositions:

```text
Delivered { message_id }
SuppressedByLifecycle { close_attempt_id }
```

The result and its delivery are different facts:

```text
result:
  BashExited(status = 0)

delivery:
  SuppressedByLifecycle(close_attempt_id = 17)
```

Suppressing delivery must never erase the terminal result or terminate the resource.

## Resource adapters

### Bash

- Validate explicit handle ownership when subscribing.
- Observe terminal truth from the Bash registry/tombstone.
- Represent Phoenix restart loss as a typed result; do not attempt process recovery.
- Do not register subscriptions from ordinary background Bash execution.

### tmux

- Validate durable server/window identity when subscribing.
- Re-probe OS tmux after Phoenix restart.
- Do not register subscriptions from ordinary `tmux_run` execution.

### Subagent

- Observe existing child/parent result authority.
- Do not create a subagent registry, second result store, or second fan-in path.
- Integrate with ProductConversation Close through the existing typed subagent settlement boundary.

## Close-facing operation

Wake must expose one typed operation conceptually equivalent to:

```text
settle_pending_deliveries_for_close(
  product_conversation_root,
  close_attempt_id,
) -> WakeSettlementEvidence
```

Requirements:

- exact-ID based;
- idempotent;
- settles each pending delivery once;
- records typed lifecycle suppression evidence;
- triggers no automatic continuation;
- does not terminate the resource;
- is safe across retry and Phoenix restart;
- does not copy Close phase state into wake records.

ProductConversation records only exact-attempt orchestration evidence that wake settlement succeeded or failed. Wake remains authoritative for underlying result and delivery facts.

## Existing machinery to preserve

- Generic durable workflows own execution attempts, observations, receipts, deliveries, and delivery suppression.
- Existing direct-turn machinery owns accepted runtime turn identity, exact replay, materialization, and runtime ownership.
- Existing runtime admission owns busy serialization and any automatic continuation.
- Existing resource authorities own terminal truth.

The implementation may use a narrow typed durable-workflow profile or existing profile seam, but it must not create a new lifecycle repository merely to rename wake.

## Required deletions

The final cutover must remove or retire:

- implicit wake registration;
- `WakeRepository` as a parallel lifecycle authority;
- `WakeWorker` and wake-specific recovery/admission;
- wake-specific deadlines and expiry arbitration unless a current requirement proves they are needed;
- cancellation-versus-terminal ownership arbitration;
- delivery-owner transfer;
- wake-specific runtime admission;
- compatibility bridges and dual writes;
- legacy unresolved obligations, which may be dropped or manually cancelled;
- wake-specific UI lifecycle state when canonical result/delivery projections replace it.

Legacy wake deletion is owned by this workstream and must not disappear during ProductConversation development.

## Explicit non-goals

This task does not own:

- ProductConversation Open/History policy;
- Close phases or whole-conversation orchestration;
- WorkScope retirement;
- stopping resources during Close;
- steering cancellation policy;
- Closing/History UI design;
- direct-turn redesign or cleanup;
- parallel Work-subagent lifecycle architecture;
- a generic cancellation framework;
- compatibility for unresolved legacy wake rows.

## Smallest delivery sequence

Implementation should begin only after ProductConversation's settlement integration seam is stable enough to consume.

Proposed sequence:

1. Prove the typed ProductConversation delivery-admission check and transaction ordering required by PR 2.
2. Define the minimal typed subscription result and delivery disposition.
3. Define the exact-ID Close settlement operation.
4. Implement one Bash delivery-only slice with no automatic continuation.
5. Prove idle, busy, Close-wins, delivery-wins, retry, and restart behavior.
6. Delete the corresponding legacy wake path rather than dual-writing.
7. Add tmux using OS reprobe.
8. Integrate subagent through existing result/fan-in authority.
9. Add automatic continuation only if it remains small and enters exclusively through existing durable direct-turn/runtime admission.
10. Delete all remaining legacy wake machinery and projections.

Every implementation phase must remove or permanently bypass a legacy authority. No phase may leave two writable lifecycle representations.

## Required evidence

### Enrollment

- Ordinary Bash, tmux, and subagent execution creates no subscription.
- Explicit wait creates exactly one subscription for one owned resource identity.

### Delivery

- One terminal observation produces one typed result.
- Exact replay does not duplicate result or conversation message.
- A busy conversation does not start a competing turn.
- Any automatic continuation uses existing durable admission.

### Close races

- Delivery commit before Close produces one delivered transcript result.
- Close commit before delivery records the result and suppresses delivery.
- Suppression remains final after Close cancellation.
- Close retry settles the same exact subscription once.
- Suppression does not stop the resource.
- WorkScope retirement later stops/removes owned resources.

### Restart

- Bash restart loss is typed.
- A surviving tmux window is re-probed and can produce its terminal result.
- A subagent result remains owned by existing child/parent state machines.
- Restart never infers delivery from timing or polling.

### Projections

- SSE/UI publish only after commit.
- Reconnect converges from durable result and delivery facts.
- ProductConversation presentation owns whether suppressed evidence is visible during Closing or History.

## Stop gates

Stop and re-ground if implementation requires:

- another wake/subscription lifecycle aggregate and repository;
- a second direct-turn or conversation admission authority;
- wake code interpreting individual Close phases;
- subscription cancellation terminating resources;
- a subagent result registry or second fan-in path;
- dual writes or compatibility translation between legacy and replacement wake;
- timing, polling, or worker order as a correctness mechanism;
- a durable child ledger or unrelated scheduler;
- broad direct-turn cleanup before the P0 ProductConversation stack stabilizes.

## Dependencies and timing

P0 ProductConversation always wins scheduling conflicts.

This P1 work may proceed during ProductConversation PR 2 development when the typed delivery-admission and subsystem-settlement seams are stable. Otherwise it begins immediately after the P0 lifecycle stack is stable. ProductConversation must not depend on this redesign to land its foundation.

## Current source anchors

Legacy wake machinery:

- `crates/phoenix-db/src/workflow/wake.rs`
- `crates/phoenix-ide/src/runtime/wake.rs`
- `crates/phoenix-workflow/src/wake_profile.rs`

Existing durable workflow and direct-turn seams:

- `crates/phoenix-db/src/workflow.rs`
- `crates/phoenix-db/src/workflow/direct_turn.rs`
- `crates/phoenix-ide/src/runtime/direct_turn_worker.rs`
- `crates/phoenix-ide/src/runtime.rs`
- `crates/phoenix-ide/src/runtime/executor.rs`
- `crates/phoenix-workflow/src/types.rs`

Resource authorities:

- `crates/phoenix-tools/src/bash/registry.rs`
- `crates/phoenix-tools/src/tmux/registry.rs`
- existing subagent/conversation state-machine result effects

## Approval boundary

This document is a planning contract, not implementation approval. Before implementation, verify the landed ProductConversation settlement seam and convert this brainstorming task into an approved executable plan with exact symbols, migration/deletion sequence, and focused tests.
