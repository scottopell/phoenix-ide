# ADR-010: Wake-resume scheduling uses a durable acceptance outbox

- **Status:** Accepted
- **Date:** 2026-07-10
- **Supersedes:** ADR-009 for the guarantee that a persisted inbox observation alone makes resume scheduling idempotent
- **Affects:** REQ-WAKE-004, REQ-WAKE-005, REQ-WAKE-008, REQ-WAKE-012, `ResumeRequest`

## Context

ADR-009 separates durable terminal observations from their later LLM resume, but
its scheduling decision leaves a crash boundary between consuming an inbox
snapshot and the runtime accepting the corresponding turn. Retrying only the
inbox is insufficient after those rows are consumed; retrying only an in-memory
event is impossible after restart. Continuation adds another constraint: the
successor LLM can observe only messages in successor history, not a message that
remains owned by the predecessor.

The durable boundary must distinguish an observation that still needs runtime
acceptance from one whose accepting `LlmRequesting` state is already persisted.
It must also preserve exactly one semantic observation when ownership transfers.

## Options considered

1. **Treat the consumed inbox and persisted message as sufficient scheduling
   state.** Recovery would infer whether to send from conversation state and
   message history. This has no structural acknowledgement boundary and can
   resend an accepted turn or lose an unaccepted one.
2. **Use a durable pending/accepted resume outbox.** Materialize the bounded inbox
   snapshot, its meta-user message, and a pending outbox row atomically. Accept
   the outbox row in the same transaction that persists `LlmRequesting`.
3. **Keep inbox rows unconsumed until the LLM request completes.** This avoids a
   second table, but conflates observation batching with LLM execution and makes
   concurrent later observations and crash recovery ambiguous.

## Decision

Adopt option 2.

A bounded inbox snapshot commits one deterministic meta-user observation and one
pending resume outbox row in the same transaction that consumes the snapshot.
Dispatch may retry a pending row whenever its conversation is idle. Runtime
acceptance atomically persists `LlmRequesting` and marks that exact row accepted
before invoking the LLM. Busy runtimes, failed sends, and process restarts leave
the row pending. Duplicate or stale events do not start a second turn.

Continuation transfers a pending resume by copying the exact meta-user content
into successor history under a deterministic successor-safe message identity,
then updating the outbox reference in the same transaction. The predecessor
message remains historical. Idempotent transfer produces neither another
successor message nor another pending semantic observation.

## Consequences

- **Positive:** Every persisted observation is structurally either awaiting
  runtime acceptance or already paired with a persisted accepting state.
- **Positive:** Startup can retry pending scheduling without guessing from inbox
  consumption markers or transient runtime state.
- **Positive:** Successor LLM history contains every pending observation it owns.
- **Negative:** Wake delivery requires another normalized durable table and an
  atomic state/outbox persistence path.
- **Negative:** Continuation transfer must allocate successor message sequence and
  identity while preserving predecessor history.
- **Neutral:** Accepted outbox rows remain durable audit records and are excluded
  from pending dispatch queries.

## References

- Superseded scheduling guarantee: ADR-009
- Feature spec: `specs/wake-contracts/requirements.md`
- Behavioral model: `specs/wake-contracts/wake-contracts.allium`
- Code: `Database::persist_wake_inbox_snapshot_message`,
  `Database::accept_wake_resume_state`, `transfer_wake_contracts_tx`,
  `wake::dispatch_pending`
