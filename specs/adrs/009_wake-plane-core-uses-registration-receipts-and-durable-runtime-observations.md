# ADR-009: Wake-plane core uses registration receipts and durable runtime observations

- **Status:** Accepted
- **Date:** 2026-07-03
- **Supersedes:** ADR-006 for delayed-result delivery, `is_busy()` derivation, and the assumption that sub-agent wakes ship in the same core implementation slice
- **Affects:** REQ-WAKE-001, REQ-WAKE-002, REQ-WAKE-003, REQ-WAKE-004, REQ-WAKE-006, REQ-WAKE-008, REQ-WAKE-009, REQ-WAKE-012, REQ-WAKE-013, REQ-WAKE-016, REQ-WAKE-017, REQ-WAKE-018

## Context

ADR-006 established the core product idea for wake contracts: a persisted,
conversation-scoped obligation to eventually deliver one accountable terminal
outcome instead of burning repeated LLM turns on polling. Task 47002 approved a
more precise core protocol before runtime implementation.

Three parts of ADR-006 no longer match the approved implementation contract:

1. it described wake delivery as a delayed synthetic tool result for the original
   `wait_until` registration call;
2. it proposed mutating `ConvState::is_busy()` based on pending contracts;
3. it grouped bash, tmux, and sub-agent wake delivery into one v1 implementation
   slice rather than treating bash + tmux as the core and sub-agent integration as
   follow-up work.

The provider-valid history constraint is the forcing function. A delayed tool
result tied to an old tool-use id becomes fragile once the conversation accepts
user messages, continues into a successor, or accumulates multiple pending waits.
Those events are product requirements, not edge cases. Phoenix needs a protocol
where the registration call finishes normally, later runtime evidence is durable,
and resumption is coalesced and idempotent.

## Options considered

1. **Keep ADR-006 literally and emit delayed tool results.**
   This preserves the closest resemblance to synchronous polling, but it creates a
   second temporal phase for one tool call and makes provider-valid history depend
   on cross-turn matching of stale tool-use ids. It also keeps pressure to thread
   wake waiting into `is_busy()` and leaves sub-agent delivery coupled to the core
   substrate rollout.
2. **Immediate receipt plus durable runtime observation.**
   Let `wait_until` complete as an ordinary tool round with a registration receipt.
   Persist later terminal evidence as a distinct runtime observation correlated by
   `contract_id`, then resume the conversation once it is safe to accept another
   LLM request. Keep lifecycle blocking as a database-aware aggregate guard rather
   than mutating conversation state, and narrow the core implementation slice to
   bash and tmux.
3. **Introduce a first-class `AwaitingWake` conversation state.**
   This would make wake waiting visible in the state machine, but it would wrongly
   model a runtime wait as a user wait, duplicate information already present in the
   wake tables, and block the existing idle/user-interruptible contract.

## Decision

Adopt option 2.

`wait_until` registration completes immediately with a structured receipt such as
`{ registered: true, contract_id, expires_at }`. That receipt is persisted as part
of the ordinary serial tool round. If one or more registrations succeed, Phoenix
finishes the tool round, checkpoints history, and leaves the conversation Idle
instead of immediately invoking the LLM again.

The eventual terminal outcome is not a delayed tool result for the registration
call. Phoenix persists it as a durable runtime observation correlated by
`contract_id`. When the conversation can safely accept another request, Phoenix
schedules one idempotent wake resume that includes all unconsumed observations in
committed order at the request snapshot boundary.

Pending wake contracts do not redefine `ConvState::is_busy()`. Chat acceptance
continues to use conversation state only. Destructive lifecycle actions use an
aggregate guard equivalent to `state.is_busy() || has_pending_wake_contracts(conv)`
and re-check that predicate transactionally.

The approved core implementation slice is bash and Phoenix-managed tmux terminal
waits. Sub-agent wake semantics remain part of the feature contract, but their
runtime integration lands in follow-up work after the bash/tmux wake plane is
proven. Continuation still transfers pending wake delivery obligations and any
unconsumed wake observations to the successor conversation, independent of the
underlying resource's ownership model.

## Consequences

- **Positive:** Provider-valid history is simpler: the registration tool call ends
  in the same round it starts.
- **Positive:** Wake delivery survives user messages, continuation, and restart as
  durable runtime observations rather than delayed tool-call bookkeeping.
- **Positive:** Multiple wake outcomes can coalesce into one resumed LLM request
  rather than causing one-turn-per-event storms.
- **Positive:** Idle conversations with pending wakes remain user-interruptible
  without pretending they are runtime-busy.
- **Negative:** The wake plane now needs durable observation/inbox state in
  addition to contract terminalization.
- **Negative:** The wake protocol is less of a byte-for-byte imitation of the
  synchronous tool transcript; correlation is by `contract_id` rather than by an
  eventual tool result message.
- **Neutral:** ADR-006 remains the historical source for the persisted,
  conversation-scoped obligation model, while this ADR narrows and corrects the
  delivery protocol and implementation slice.

## References

- Superseded decision: ADR-006
- Authoritative implementation tracker: `tasks/47002-p1-in-progress--implement-wake-plane-core-bash-tmux.md`
- Feature spec: `specs/wake-contracts/requirements.md`
- Behavioral model: `specs/wake-contracts/wake-contracts.allium`
- Status summary: `specs/wake-contracts/executive.md`
- Related specs: `specs/bash/requirements.md`, `specs/tmux-integration/requirements.md`, `specs/subagents/requirements.md`
