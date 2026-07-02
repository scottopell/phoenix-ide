# ADR-006: Wake contracts are persisted conversation-scoped terminal waits

- **Status:** Accepted
- **Date:** 2026-07-01
- **Affects:** REQ-WAKE-001, REQ-WAKE-002, REQ-WAKE-003, REQ-WAKE-004, REQ-WAKE-005, REQ-WAKE-006, REQ-WAKE-009, REQ-WAKE-010, REQ-WAKE-012, REQ-WAKE-013, REQ-WAKE-016, REQ-WAKE-017, REQ-WAKE-018

## Context

Phoenix has several long-running work substrates: bash handles, tmux windows, and
sub-agent conversations. An LLM can synchronously poll those substrates, but a
poll loop consumes repeated LLM turns even when the agent has nothing useful to
say until the work reaches a terminal state. Sub-agent fan-in avoids polling only
by baking a blocking wait into the parent state machine.

The design problem has three independent axes:

- conversation state determines whether user input is expected;
- WorkScope determines resource lifetime for bash/tmux resources across
  continuation and cleanup boundaries;
- wake delivery determines which conversation receives a terminal handle result.

Conflating these axes creates invalid states. A conversation waiting on a runtime
event should remain user-interruptible, so representing the wait as an
`Awaiting*` state would imply the wrong user contract. Likewise, a WorkScope-owned
resource can transfer to a continuation, while a sub-agent terminal result remains
keyed by the child conversation / agent id.

## Options considered

1. **In-memory wake handles.** Keep pending waits only in a router task. This is
   simple, but Phoenix restart would silently lose the commitment to wake the
   parent conversation.
2. **A new `AwaitingWake` conversation state.** Model runtime waits like task
   approval or continuation waits. This gives a visible state-machine node, but
   it makes a runtime wait look like a user wait and creates a parallel
   representation alongside the pending contract row.
3. **Persisted conversation-scoped terminal contracts.** Persist wake contracts as
   rows, keep the conversation otherwise `Idle`, and have a wake router deliver a
   synthetic terminal tool result when the watched handle resolves.

## Decision

Adopt option 3. A wake contract is a persisted Phoenix commitment to resume a
specific conversation when a concrete handle reaches a terminal, expired,
cancelled, or forgotten outcome. The conversation stays `Idle` while the contract
is pending. Busy/cleanup decisions derive from the contract table rather than from
a new conversation state.

Wake contracts v1 supports terminal waits only for bash handles, tmux `window_id`
handles, and sub-agent handles. Bash and tmux handles are WorkScope-keyed and can
transfer with a same-WorkScope continuation; sub-agent handles are keyed by the
child conversation / agent id and do not transfer by WorkScope inheritance.

Wake delivery uses synthetic tool results instead of a new conversation event
kind. Where a synchronous wait surface already exists, the wake result mirrors its
payload; sub-agent wakes use a tagged terminal payload. Multiple contracts on one
conversation resolve independently; a later compound `wait_any` or `wait_all`
contract can encode first-wins semantics if that behavior is needed.

## Consequences

- **Positive:** Waiting on long-running work consumes no LLM turns until the
  watched terminal condition resolves.
- **Positive:** User messages remain valid while the parent conversation is idle
  with pending wake contracts.
- **Positive:** Restart handling is explicit: the contract survives, and startup
  either re-registers durable handles, delivers persisted child terminal state, or
  emits a forgotten result for handles that cannot still resolve.
- **Negative:** `is_busy()` and lifecycle cleanup need a contract-table read in
  addition to conversation state.
- **Negative:** The wake router polls handle state; push-based notification from
  each substrate would be more efficient, but would couple bash/tmux/sub-agent
  implementations to the wake router.
- **Neutral:** V1 intentionally excludes actor-style parent/child messaging,
  parent-to-child continuation, `NeedMoreBudget`, regex/file/webhook conditions,
  and compound conditions. Those require separate product contracts.

## References

- Related ADRs: ADR-000, ADR-001, ADR-002, ADR-003
- Feature spec: `specs/wake-contracts/requirements.md`
- Behavioral model: `specs/wake-contracts/wake-contracts.allium`
- Status summary: `specs/wake-contracts/executive.md`
- Related specs: `specs/bash/requirements.md`, `specs/tmux-integration/requirements.md`, `specs/subagents/requirements.md`
