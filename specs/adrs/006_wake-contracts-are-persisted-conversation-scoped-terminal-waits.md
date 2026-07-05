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

The wake plane is an accountability layer, not a handle-durability layer. Bash
handles are process-local; a Phoenix restart can make the underlying process
answer unknowable. The requirement is still stronger than "best effort": once
Phoenix accepts a wake registration, the parent conversation must eventually
receive exactly one accountable result — a fired payload, expiry, cancellation, or
forgotten reason. Persisting the wake contract preserves that obligation even
when the watched substrate cannot survive.

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

1. **Ephemeral router registrations.** Keep pending waits only in a router task.
   This is the KISS option: no schema, no restart reconciliation, and no stored
   deadlines. Its cost is loss accounting. After Phoenix restart, the system
   cannot distinguish "no wait was registered" from "a wait was registered, but
   the watched handle is no longer knowable." That erases the parent
   conversation's recovery path.
2. **Persisted registrations without mandatory deadlines.** Persist the wait
   intent, but allow unbounded waits. This preserves restart recovery, but it
   creates durable commitments with no upper bound on when Phoenix will spend the
   next LLM turn. A missing evaluator, stuck handle, or forgotten cancellation can
   become a zombie contract.
3. **A new `AwaitingWake` conversation state.** Model runtime waits like task
   approval or continuation waits. This gives a visible state-machine node, but
   it makes a runtime wait look like a user wait and creates a parallel
   representation alongside the pending contract row.
4. **Persisted conversation-scoped terminal contracts with mandatory delivery
   deadlines.** Persist wake contracts as rows, keep the conversation otherwise
   `Idle`, and have a wake router deliver exactly one synthetic terminal tool
   result when the watched handle resolves, is cancelled, is forgotten, or reaches
   its deadline.

## Decision

Adopt option 4. A wake contract is a persisted Phoenix commitment to resume a
specific conversation with exactly one accountable outcome: fired, expired,
cancelled, or forgotten. The contract makes the wait intent durable; it does not
make every watched handle durable. When a handle cannot still produce a terminal
answer, Phoenix delivers `Forgotten` rather than silently dropping the wait.

Every contract carries an `expires_at` delivery deadline. The deadline is an upper
bound on how long a running Phoenix instance may keep the parent conversation
parked without an answer. If Phoenix is down when the deadline passes, startup
resync resolves the contract before normal serving resumes. Expiry reports that
the condition did not hold before the deadline; forgotten reports that the handle
became unknowable before Phoenix could evaluate the condition. The conversation
stays `Idle` while the contract is pending. Busy/cleanup decisions derive from the
contract table rather than from a new conversation state.

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
- **Positive:** Restart handling is explicit: the wait obligation survives, and
  startup either re-registers durable handles, delivers persisted child terminal
  state, expires overdue contracts, or emits a forgotten result for handles that
  cannot still resolve.
- **Negative:** Every accepted contract creates a bounded future delivery of a
  synthetic result and possible LLM turn, even when the final answer is only
  `Expired` or `Forgotten`.
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
