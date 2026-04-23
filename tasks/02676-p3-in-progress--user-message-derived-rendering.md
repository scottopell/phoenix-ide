---
created: 2026-04-23
priority: p3
status: in-progress
artifact: ui/src/pages/ConversationPage.tsx
---

# User-message rendering: derive, don't patch

## Problem

User messages live in two stores that both drive rendering:

- `queuedMessages` (localStorage-persisted, client-authored, via `useMessageQueue`).
- `atom.messages` (SSE-driven, server-authored, via the conversation reducer).

`MessageList.tsx:115-117` interleaves them. Boundaries between the two stores
are hand-wired via imperative calls: `enqueue` on send, `markSent` when the
message shows up server-side, `markFailed` on error.

This is the structural form of the bug fixed in task 02673. The `markSent`
boundary was being crossed too eagerly (on POST-200 instead of on SSE echo),
and the optimistic `sse_message` dispatch that was supposed to paper over
the gap was silently dropped by the monotonic guard. The fix in 02673
(`b7f053e`) added a reconciliation effect that calls `markSent` when the
echoed message lands in `atom.messages`. It works, but it preserves the
imperative shape — next time someone touches this code, the same class of
bug is one step away.

## Design

Remove the imperative `markSent` API for the online-echo path. Treat the
rendered user-message list as a pure derivation:

```ts
const pendingMessages = useMemo(() => {
  const serverIds = new Set(atom.messages.map((m) => m.message_id));
  return queuedMessages.filter((q) => !serverIds.has(q.localId));
}, [atom.messages, queuedMessages]);
```

`queuedMessages` becomes purely "what the client has attempted to send
that the server hasn't echoed yet." `sent` state is *derived*, not
*stored*. A queued message remains in storage until it appears in
`atom.messages` (by `message_id` — which is the client's `localId`,
see `api.ts:418` + `types.rs:47`), at which point it's filtered out of
the rendered list automatically.

### What this removes

- `markSent` API from `useMessageQueue` (or narrow to the explicit
  dismiss-failed case only).
- The reconciliation useEffect added in task 02673
  (`ConversationPage.tsx`).
- The timing-dependent transition between "visible in queue" and
  "visible in atom" — they overlap briefly during the SSE echo, but
  the filter removes the queue entry automatically.

### What stays

- `markFailed` — failure is not derivable from the server; the client
  observes POST rejection and must record it locally.
- `dismiss` — the user's explicit action to drop a failed entry.
- `retry` — the user's explicit action to re-attempt.

### `MessageStatus` narrowing

Current type is `'sending' | 'failed'`. The `'sending'` state becomes
implicit (presence in the queue == still sending). Narrow the type to a
single `failed: boolean` or remove the field entirely.

## Concerns

### Offline / queueOperation interaction

The offline path at `ConversationPage.tsx:352-362` uses a separate
`queueOperation` system (for operations to replay when connectivity
returns) and calls `markSentRef.current(localId)` after enqueueing.
This removes the message from `useMessageQueue` while the operation
sits in the offline queue.

Needs review: should offline messages stay in `useMessageQueue` too,
so the user keeps seeing them? Or is the offline path's UX handled
separately? Either way, the new design should make the two queues
consistent or explicitly document the split.

### localStorage persistence across refresh

`useMessageQueue` persists to localStorage. On refresh, queued messages
rehydrate. With the new derivation:

- Server echoes the message (it persisted) → `atom.messages` has it
  after SSE init → filter removes it from pending → no duplicate render.
- Server doesn't have the message (POST never reached) → pending stays
  visible → user can retry.

Both cases work correctly because the derivation uses `message_id`/
`localId` identity.

## Acceptance Criteria

- [ ] `markSent` removed or narrowed to explicit user actions only.
- [ ] Rendered user-message list computed as a pure function of
      `atom.messages` and `queuedMessages`.
- [ ] Reconciliation useEffect from task 02673 deleted (the derivation
      replaces it).
- [ ] The offline `markSent` call site in `ConversationPage.tsx` is
      updated — either the offline path also stays in the queue until
      echoed, or the split is documented and intentional.
- [ ] Test: send a message, receive the SSE echo → rendered exactly
      once (not twice during the overlap window).
- [ ] Test: send a message, POST fails → renders as failed, retryable.
- [ ] Test: reload mid-send (message in queue, server has it) →
      rendered once after rehydration.
- [ ] Test: reload mid-send (message in queue, server doesn't have
      it) → renders as pending, resends on connection restored.
- [ ] No regression in the user flow fixed by 02673 (message stays
      visible continuously from send through SSE echo).

## Rationale

Dissolves an entire class of bugs (timing gaps between client and
server stores) by making the rendered state a derivation instead of a
patching target. Every future bug in this area becomes "the derivation
is wrong," which is bisectable and unit-testable, instead of "the
imperative side-effect ran at the wrong time," which is order-dependent
and localStorage-flaky.

## Dependencies

Can be done independently of the other two SSE tasks, but benefits
from `sse-unify-sequence-id-dedup` (02675) being done first — a
unified ordering makes the server-echo detection robust even across
reconnect.

## Out of Scope

- Server-side message deduplication (task 02673 + the idempotent
  `message_id`-as-POST-body already handle it).
- Rewriting the offline operations queue.
- `streamingBuffer` rendering (assistant-side streaming, not user
  messages).
