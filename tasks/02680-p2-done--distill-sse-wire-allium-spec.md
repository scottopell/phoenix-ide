---
created: 2026-04-23
priority: p2
status: done
artifact: specs/sse_wire/sse_wire.allium
---

# Distill sse_wire.allium — SSE protocol contract between server and client

## Scope

Formalize the SSE wire protocol as an Allium spec. Every event type
(`init`, `message`, `message_updated`, `state_change`, `agent_done`,
`token`, `conversation_update`, `conversation_became_terminal`,
`error`) with payload shape (cross-referencing the generated TS types
from task 02677) and — critically — the **ordering constraints**
between them.

## Why

Task 02679 (streaming-finalize-disappears open mystery) is the
leading example of a class that lives HERE, not in
`conversation_atom.allium`. The client reducer is structurally
correct; the bug is almost certainly the server's emit-vs-persist
ordering (broadcast before DB commit) racing with the broadcast-lag
close-and-reconnect flow introduced in `736b37d`. Naming the ordering
invariants formally — "DB write happens-before broadcast", "init's
snapshot is consistent with the next broadcast frame's seq", etc. —
makes the emit-vs-persist bug a spec violation, not an open mystery.

## Invariants worth writing

- `init` always first on any SSE stream; carries the full message
  list up to `last_sequence_id`.
- Merge-by-id semantics on reconnect init: no duplicates, no loss.
- `sse_agent_done` for turn T implies no more `sse_token` events for
  turn T on this conversation.
- `sse_message` for message M: no `sse_message_updated` for M before
  M's `sse_message` (except as an init replay).
- **Persist-before-broadcast**: if an event references a persisted
  entity (a message, a conversation metadata change), the persistence
  is committed before the broadcast fires. Any reconnecting client
  that reads the DB after the broadcast will see the entity. Without
  this invariant, task 02679's emit-vs-persist race is representable.
- **Lag semantics**: if the server emits `BroadcastStreamRecvError::Lagged`,
  the stream closes and the client reconnects; the resync `init` is
  equivalent to fresh load — no silent gap.

## Cross-references

- **Task 02679** — this spec would directly frame the hypothesis and
  test for that open-mystery bug. The "persist-before-broadcast"
  invariant is exactly what 02679 proposes as the minimal fix.
- **Task TBD** (`distill-connection-machine-allium-spec`) — adjacent
  spec. Where this one says "on Lagged the stream closes," the
  connection-machine spec says "when the stream closes, the client
  reconnects." Together they form the resync contract.
- **`specs/conversation_atom/conversation_atom.allium`** (landed at
  `68b7336`) — the reducer-side spec. This wire spec is the dual:
  reducer says "what the client does with a delivered event," wire
  spec says "what the server is obliged to deliver."
- **Task 02677** — ts-rs codegen. The wire spec should reference the
  generated types as the payload contract.

## Acceptance

- `specs/sse_wire/sse_wire.allium` exists.
- `allium check specs/sse_wire/sse_wire.allium` passes cleanly
  (0 errors, 0 warnings).
- Self-contained (no `use` imports), following the pattern
  established in `conversation_atom.allium` — avoids the existing
  specs' compliance issues entering the dep graph.
- Every SSE event type named above has a rule / invariant.
- The persist-before-broadcast invariant is explicit and named.
