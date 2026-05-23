# Specify and fix pending user-message/SSE delivery reconciliation

## Problem

A user message can remain visually stuck as `pending`/`sending` even while the LLM has clearly received it and is responding. The current strongest hypothesis is a cross-boundary reconciliation gap between:

- the local optimistic message queue (`useMessageQueue`, `derivePendingMessages`),
- POST `/api/conversations/:id/chat` idempotency (`message_id = localId`),
- server SSE stream-open init/replay semantics (`sse_wire.allium`), and
- the client `ConversationAtom` reducer (`conversation_atom.allium`).

The likely failure mode is that a persisted user message lands between the server's DB message snapshot and live SSE subscription/replay snapshot. Init can then advance the client's sequence floor past that user-message sequence without delivering the corresponding message row in either `messages` or `pending_events`. Later LLM state/tokens/tool events arrive normally, so the agent visibly works, but the local pending bubble never reconciles because `atom.messages` never contains `message_id === localId`.

## Why this matters

The user-visible requirement is not “flip to sent when `{queued: true}` returns”. The real requirement is: once Phoenix accepts a user message, the UI must give durable delivery confidence. A message must not remain indefinitely visually pending while later system behavior proves the message was accepted and acted on.

This task should reframe the spec around that user benefit, then use Allium + tests to squeeze out the bug and any nearby spec/code drift.

## Scope

### 1. Rework REQ-CONV-004 around delivery confidence

Update `specs/conversation-ui/requirements.md` so REQ-CONV-004 is less implementation-prescriptive and states the underlying user requirement:

- Sent messages are immediately visible optimistically.
- The UI distinguishes “not yet accepted”, “accepted/durable or server-reflected”, and “failed/retryable” states in a way users can trust.
- Once server behavior proves a message was accepted (e.g. authoritative history contains it, or subsequent turn activity is causally tied to it), the UI must not leave that message indefinitely marked pending.
- Network/offline behavior must preserve the user's message and provide retry/recovery affordances.

Remove or revise stale wording that says POST `{queued: true}` alone means “sent” if the intended implementation remains SSE/history-authoritative.

### 2. Add/complete `user_message_queue.allium`

Create `specs/user_message_queue/user_message_queue.allium`, absorbing/superseding the narrow existing task `02682-p3-ready--distill-user-message-queue-allium-spec.md`.

The spec should cover:

- local enqueue with `localId`, text, images, status,
- POST sends `message_id = localId`,
- pending derivation: render queued messages only when no authoritative server message with matching `message_id` exists,
- failed/retry/dismiss transitions,
- `steering_queued` behavior and eventual removal when the drained user message appears,
- localStorage rehydration and conversation-id scoping,
- invariant: rendered user-visible messages are the union of authoritative server user messages plus unreflected local queue entries, without duplicates,
- liveness/recovery obligation: accepted local messages must either reconcile to authoritative history or become an explicit recoverable inconsistency, never silent pending forever.

### 3. Strengthen `sse_wire.allium` around stream-open gap freedom

Audit `specs/sse_wire/sse_wire.allium` and add an explicit named invariant/rule if needed, such as `InitSnapshotNoDurableGap`:

- For every persisted message with `sequence_id <= init.pending_anchor_sequence_id`, the init DB `messages` snapshot must include that message.
- For every event with `pending_anchor_sequence_id < sequence_id <= last_sequence_id`, the init snapshot must either include the durable state that subsumes it or include it in `pending_events`.
- Init must not advance the client's sequence floor past a message/event that was neither delivered in the DB snapshot nor replayed.

The current `StreamOpened` guidance appears to intend this; make it testable and unmistakable.

### 4. Add regression tests that reproduce the suspected gap

Add tests at the appropriate layer(s), likely including:

- server-side SSE stream-open test: simulate/pin the ordering where a message is persisted after `get_messages()` but before `snapshot_pending()` / init construction; assert the init outcome cannot omit the message while advancing past its sequence,
- client reducer test: init with `last_sequence_id` beyond a missing local user message should not strand the local queue silently if subsequent events prove the turn is active,
- user message queue tests: accepted/reflected messages disappear from pending; rehydrated stale entries do not double-render; unreconciled accepted entries get an explicit recovery path.

Prefer deterministic unit tests over timing sleeps. If the production code needs refactoring to expose a seam for deterministic stream-open ordering, include that refactor.

### 5. Hunt and fix spec/code mismatches found during the process

Use the new/updated specs to audit implementation behavior. In particular inspect:

- `crates/phoenix-ide/src/api/handlers.rs::stream_conversation`,
- `crates/phoenix-ide/src/runtime.rs::SseBroadcaster` / ReplayRing,
- `ui/src/conversation/atom.ts`,
- `ui/src/hooks/useMessageQueue.ts`,
- `ui/src/pages/ConversationPage.tsx` send/retry/reconnect flow,
- `ui/src/hooks/useConnection.ts` init/message dispatch sequence ids.

Fix any confirmed divergence rather than only documenting it. If a spec is wrong, revise the spec and note why.

## Acceptance criteria

- REQ-CONV-004 expresses user-visible delivery confidence rather than hardcoding stale implementation details.
- `specs/user_message_queue/user_message_queue.allium` exists and validates with `allium check`.
- `sse_wire.allium` explicitly forbids init snapshots that advance past undelivered durable messages/events.
- Regression tests cover the pending-forever scenario or the closest deterministic reduction of it.
- Confirmed code/spec divergence in stream-open snapshot ordering is fixed or explicitly disproven with tests.
- Existing SSE/reducer/message-queue tests still pass.
- `./dev.py check` passes.

## Notes

Existing related task `02682-p3-ready--distill-user-message-queue-allium-spec.md` is narrower and should be closed, merged into this task, or referenced as superseded once this lands.
