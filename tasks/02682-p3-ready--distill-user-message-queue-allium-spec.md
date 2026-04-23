---
created: 2026-04-23
priority: p3
status: ready
artifact: specs/user_message_queue/user_message_queue.allium
---

# Distill user_message_queue.allium — queued-to-rendered derivation

## Scope

Formalize the user-message queue pattern from `useMessageQueue.ts` and
the `derivePendingMessages` helper. Small, clean state machine:

- `enqueue` → message in queue with implicit `pending` status.
- Server echoes via `atom.messages` → queue entry filtered out of the
  rendered view (derivation, not a side-effect).
- POST fails → `markFailed` keeps the entry visible as failed.
- User `retry` → back to pending.
- User `dismiss` → removed from queue.

## The load-bearing invariant

```
rendered_user_messages =
    atom.messages ∪ { q ∈ queue | q.localId ∉ atom.messages.map(m.message_id) }
```

That single line is what tasks 02673 and 02676 landed in production.
It's a pure derivation — no timing-dependent `markSent` /
`markFailed` coordination — and having it stated as a formal invariant
would mean the next "message briefly disappears between POST and SSE
echo" bug can't ship without tripping the spec.

## Why less urgent than the other two

Task 02679 (the current mystery) is *not* a user-message-queue bug.
Task 02673 patched the visible pain for this path, and 02676 made the
fix structural. The class is now well-understood and well-tested.
This spec is the "name the invariant that's already correct" task —
lowest urgency of the four distillations I recommended.

## Cross-references

- **Task 02679** — unrelated bug, but the meta-pattern (formal
  invariant catches future regressions) is shared.
- **Task 02680** / **02681** — sibling distillations. All three would
  be referenced collectively as "the client-side SSE specs."
- **`specs/conversation_atom/conversation_atom.allium`** (landed at
  `68b7336`) — the reducer side. This spec is the dual at the
  rendering layer: reducer owns `atom.messages`, this spec owns the
  derivation over it plus the local queue.
- **Task 02673** (`b7f053e`) and **02676** (`4efaba1`) — the
  implementations this spec formalizes.

## Acceptance

- `specs/user_message_queue/user_message_queue.allium` exists.
- `allium check` passes cleanly.
- Self-contained.
- The derivation invariant above is explicit and named.
- Covers the four state transitions: enqueue, server-echo (implicit
  removal via derivation), mark-failed, retry, dismiss.
