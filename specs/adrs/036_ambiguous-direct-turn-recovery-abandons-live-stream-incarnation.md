# ADR-036: Ambiguous direct-turn recovery abandons the live stream incarnation

- **Status:** Accepted
- **Date:** 2026-08-15
- **Affects:** REQ-DWF-CHAT-012, REQ-DWF-CHAT-013, REQ-DWF-CHAT-014, REQ-DWF-CHAT-016; `DirectTurnReconciliationResult`, `SseStream`, `ReplayRing`

## Context

Direct-turn materialization spans two different kinds of truth. SQLite owns the
accepted turn, generation, claim, canonical user-message materialization, and
terminal settlement. A live runtime owns process-local execution, while its SSE
broadcaster owns an ephemeral replay ring, subscriber set, and cursor for one
stream incarnation.

When a materialization or post-materialization transaction has an ambiguous
outcome, continuing the same live stream requires proving that every reserved
cursor, broadcaster sender, runtime handle, and later recovery attempt still has
one coherent owner. Repeated handoff and cleanup exceptions make that proof less
reliable while treating process-local projection state as if it were durable
authority. The client already reconnects after server-side stream closure and
accepts a validated `Init` built from durable state.

## Options considered

1. **Preserve the broadcaster and repair the reserved cursor.** Existing
   subscribers remain connected, but correctness requires transfer across every
   cleanup and replacement failure path. A filler event would make an event true
   only to preserve an in-memory cursor.
2. **Persist the SSE event stream or add an outbox and retry service.** This could
   make cursor continuation durable, but creates another persistence model and a
   manager-wide lifecycle service for a conversation-local recovery boundary.
3. **Abandon the ambiguous runtime and stream incarnation.** Retain durable
   repository authority, drop all senders for the old broadcaster, let subscribers
   observe closure, and reconstruct a fresh incarnation from an authoritative
   SQLite-backed `Init` on reconnect or later durable discovery.

## Decision

Adopt option 3.

One exact repository reconciliation command for an accepted turn and generation
returns a closed typed result: committed materialization, confirmed non-commit,
ambiguous materialization, ambiguous post-materialization recovery, or stale
authority. Runtime errors, missing projected messages, projected conversation
state, runtime absence, and cleanup outcomes cannot independently classify that
result.

A confirmed non-commit releases only the exact unmaterialized claim identified by
the result. Committed materialization preserves the canonical message identity,
claim, generation fence, and at-most-once provider-dispatch authority. Ambiguous
results retain the claim or owed work; stale results do not mutate current
authority.

For either ambiguous result, Phoenix exits the affected runtime, identity-removes
its exact handle, and drops all manager/runtime sender ownership for the old
broadcaster. It does not deposit that broadcaster into replacement retention,
transfer its replay ring or reserved cursor, fabricate a repair event, or create a
replacement from the ambiguity handler. Only a subsequent client reconnect or
later durable-discovery pass may create a fresh broadcaster and stream
incarnation after old sender ownership is absent. The new stream emits `Init`
first from committed SQLite state plus replay state belonging only to the new
incarnation.

Recovery remains conversation-local: one candidate's ambiguity does not stop
independent candidates or prevent the worker startup-readiness pass from
completing.

## Consequences

- **Positive:** Durable repository truth remains the sole authority for claim,
  materialization, and dispatch decisions.
- **Positive:** Subscriber closure and replacement ordering are structural: old
  sender ownership reaches zero before a new incarnation exists.
- **Positive:** The design deletes same-stream reservation and broadcaster handoff
  obligations rather than relocating them.
- **Negative:** Affected clients see a brief reconnect and lose ephemeral in-flight
  replay entries from the abandoned incarnation.
- **Negative:** Work whose exact repository outcome remains ambiguous stays owed
  until a later reconciliation can classify it; it is not made retryable by
  inference.
- **Neutral:** Ordinary replay-ring reconnect behavior remains unchanged while an
  incarnation is coherent.

## References

- Extends ADR-024's one-authority-per-semantic-fact decision.
- Requirements: `specs/durable-workflows/requirements.md`
- Behavioral specs: `specs/durable-workflows/direct-chat-profile.allium`,
  `specs/sse_wire/sse_wire.allium`, `specs/connection_machine/connection_machine.allium`
- Existing boundaries: `ConversationManager::get_or_create`,
  `SseBroadcaster::snapshot_pending`, `sse_stream`
