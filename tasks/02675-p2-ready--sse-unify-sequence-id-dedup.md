---
created: 2026-04-23
priority: p2
status: ready
artifact: ui/src/conversation/atom.ts
---

# Unify sequence-id dedup across SSE event types

## Problem

Each SSE event type in `ui/src/conversation/atom.ts` has its own dedup rule,
and there is no unifying contract. The current matrix:

| Event | Dedup key | Reducer behavior |
|---|---|---|
| `sse_init` | — (merges by id) | merges existing + appends new |
| `sse_message` | strict `sequence_id` | **append** (no id dedup) |
| `sse_message_updated` | **none** | in-place mutate by `message_id` |
| `sse_state_change` | `sequence_id` if present (**server doesn't populate it** — see `useConnection.ts:223`) | replace phase + breadcrumb cascade |
| `sse_agent_done` | `sequence_id` if present | set phase=idle, clear streamingBuffer |
| `sse_token` | per-connection closure counter that resets on reconnect | accumulate |
| `sse_conversation_update` | none | merge fields |

### Latent bugs this causes

1. **`sse_message` can duplicate on replay.** `atom.ts:229` is
   `[...atom.messages, action.message]` — no id dedup. Safe only because
   the server contract "never re-emits a message with a fresh sequence_id"
   is assumed. If reconnect logic ever replays messages, duplicates result.

2. **Server-side sequence jumps strand messages.** If `init` arrives with
   `lastSequenceId=100` but `messages` ends at seq 95, `atom.lastSequenceId`
   leapfrogs to 100 and subsequent `sse_message` events for 96–100 are all
   rejected by the guard. Unrecoverable without a full refresh.

3. **`sse_message_updated` has no replay protection.** If an update is ever
   not idempotent (counter field, order-sensitive content), two deliveries
   corrupt state.

4. **`sse_state_change` replays unconditionally.** The server doesn't send
   `sequence_id` on state_change (comment at `useConnection.ts:223`), so
   the optional guard is always skipped. A replayed state_change on
   reconnect reapplies breadcrumb cascades and can corrupt the breadcrumb
   list.

5. **`sse_token` reconnect stall.** `useConnection.ts:253` resets
   `tokenSequence = 0` per SSE connection, but `atom.streamingBuffer.lastSequence`
   persists in the atom across reconnects. If SSE drops mid-stream and the
   server continues streaming through the new connection, post-reconnect
   tokens 1, 2, 3, … are silently dropped by `atom.ts:338` until
   `tokenSequence` crosses the pre-reconnect high-water mark. Whether
   this fires today depends on whether the server keeps streaming across
   reconnect (server contract question) — but the guard is written in a
   way that presumes the answer, and there's no comment stating it.

## Design

Every SSE event carries the conversation's global `sequence_id`. Every
client handler uses the same guard.

### Server changes

- Add `sequence_id: u64` to the `state_change`, `message_updated`, `token`,
  `conversation_update`, `agent_done`, and `conversation_became_terminal`
  event payloads.
- Sequence ids are monotonic per conversation, allocated in the order events
  are emitted. Share the counter with `message` events so the ordering is
  a single total order.

### Client changes (`atom.ts`)

- Extract a helper:

```ts
function applyIfNewer(
  atom: ConversationAtom,
  sequenceId: number,
  apply: (a: ConversationAtom) => ConversationAtom,
): ConversationAtom {
  if (atom.lastSequenceId >= sequenceId) {
    if (import.meta.env.DEV) {
      console.warn(`[sse] dropping event seq=${sequenceId} (have ${atom.lastSequenceId})`);
    }
    return atom;
  }
  return { ...apply(atom), lastSequenceId: sequenceId };
}
```

- Every case in `conversationReducer` goes through `applyIfNewer`
  (or its init equivalent that merges by id).

### `sse_message` also needs id dedup

Change the reducer to skip if `message_id` already exists, regardless of
sequence_id. Defense-in-depth so the contract "never re-emit with a fresh
sequence_id" stops being load-bearing.

### `sse_token` reconnect stall fix

Token sequences share the global conversation sequence space. The
per-connection counter in `useConnection.ts:253` is removed. Server emits
real sequence_ids on token events.

## Acceptance Criteria

- [ ] Server emits `sequence_id` on every SSE event, monotonic within the
      conversation's total order.
- [ ] Client reducer uses a single `applyIfNewer` helper for every event
      type.
- [ ] `sse_message` also dedups by `message_id` — a second delivery with
      a different sequence_id does not duplicate.
- [ ] Per-connection `tokenSequence` closure in `useConnection.ts` is
      removed; tokens use server-provided sequence_ids.
- [ ] Dev mode: dropped dispatches log a structured warning (event type +
      both sequence ids) so silent drops become observable.
- [ ] Test: replay the same init event twice → atom converges to the
      same state (idempotent).
- [ ] Test: simulated reconnect mid-stream with server continuing to emit
      tokens → new tokens accumulate without stall.
- [ ] Test: duplicate `message_updated` events → state reflects exactly
      one application.
- [ ] Test: the `lastSequenceId` jump scenario (init lastSeq=100, messages
      only to 95, subsequent individual events for 96–100) — all five
      messages land in `atom.messages`.

## Rationale

Collapses the N-event × M-guard fragility matrix into one rule: *events
are ordered facts; ordering is the server's single global sequence_id;
the client applies each fact exactly once in order.* Makes reasoning
about reconnect and replay trivial. Makes the token reconnect stall go
away as a side effect.

## Dependencies

Best done after `sse-wire-schema-validation` (02674) — that task makes
silent contract drift observable, which you want in place before
touching how events are deduplicated.

## Out of Scope

- Changing the transport (SSE stays; no WebSocket migration).
- Deriving rendered state (task: `user-message-derived-rendering`).
