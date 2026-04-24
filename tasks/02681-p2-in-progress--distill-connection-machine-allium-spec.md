---
created: 2026-04-23
priority: p2
status: in-progress
artifact: specs/connection_machine/connection_machine.allium
---

# Distill connection_machine.allium — SSE connection lifecycle

## Scope

Formalize the client-side SSE connection lifecycle as an Allium spec.
The state machine already exists explicitly in
`ui/src/hooks/connectionMachine.ts` (states, transitions, effects),
so this is largely a mechanical distillation.

## States

- `connecting` (initial / after disconnect)
- `live` (SSE open and delivering)
- `reconnecting` (retry timer scheduled after SSE_ERROR)
- `offline` (browser reports navigator.onLine = false)
- `reconnected-display` (grace period showing "reconnected" badge)

## Transitions worth formalizing

- `CONNECT` / `DISCONNECT` (conversationId mount/unmount)
- `SSE_OPEN` (EventSource's first event delivered → live)
- `SSE_ERROR` (EventSource error, which includes clean server close) →
  schedule retry
- `RETRY_TIMER_FIRED` → back to `connecting`
- `BROWSER_ONLINE` / `BROWSER_OFFLINE` (via window.online/offline
  events + visibilitychange)
- `RECONNECTED_DISPLAY_DONE` (grace period elapsed → live)

## Why

Task 02679 (streaming-finalize-disappears) and the broadcast-lag
close fix from `736b37d` both rely on the following chain of
invariants:

1. Server closes the stream on `BroadcastStreamRecvError::Lagged`.
2. Client's EventSource observes onerror.
3. ConnectionMachine fires `SSE_ERROR` → schedules retry.
4. After backoff, reconnects → OPEN_SSE effect fires new EventSource.
5. Server's new subscribe handler sends `init` first.
6. Client's reducer applies init → state consistent with server.

Each step is in code but nowhere written down as a contract. If any
step breaks (e.g. a new reconnecting state is added that doesn't
trigger OPEN_SSE), the lag-close fix silently degrades into exactly
the bug it was designed to prevent. This spec would catch that.

## Invariants worth writing

- After `SSE_ERROR` from a previously-live connection, the machine
  eventually reaches `live` again (assuming browser stays online) —
  i.e. no dead-end reconnecting state.
- An OPEN_SSE effect is emitted exactly once per entry into
  `connecting`.
- Retry delay backoff has a ceiling (don't reconnect-storm).
- Visibility-change on focus regain triggers a reconnect only if
  the machine was in offline/reconnecting.

## Cross-references

- **Task 02679** — this spec's "stream close → reconnect → OPEN_SSE"
  chain is load-bearing for the leading hypothesis in that task.
- **Task 02680** (`distill-sse-wire-allium-spec`) — the dual. Wire
  spec says "on Lagged, the stream closes"; this spec says "when the
  stream closes, the client reconnects." Together they form the
  resync contract.
- **`specs/conversation_atom/conversation_atom.allium`** (landed at
  `68b7336`) — the reducer-side spec. Connection state transitions
  into `connecting` dispatch `connection_state` actions to the atom;
  the reducer-side spec names that action, this spec names the
  producer.

## Acceptance

- `specs/connection_machine/connection_machine.allium` exists.
- `allium check` passes cleanly.
- Self-contained (no `use` imports).
- Every state in `connectionMachine.ts`'s state enum has a
  corresponding named state here.
- The "close → reconnect → open" chain is explicit and testable.
