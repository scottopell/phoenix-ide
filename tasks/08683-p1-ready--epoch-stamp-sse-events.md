---
created: 2026-05-05
priority: p1
status: ready
artifact: ui/src/hooks/useConnection.ts
---

# epoch-stamp-sse-events

## Summary

`useConnection` carries a latent cross-conversation contamination bug that
task 02703 made structurally important. SSE events are trusted by
`sequenceId` alone; a fresh atom has `lastSequenceId: 0` and accepts any
event with `sequenceId > 0`. Combined with the `setTimeout(0)` indirection
in the effect executor (`useConnection.ts:200-205`), there is a one-task
window during slug change where:

- `dispatchRef.current` already points at the new slug's atom (set by an
  earlier-firing effect)
- The old `EventSource` is still open (`CLOSE_SSE` is also wrapped in
  `setTimeout(0)`)
- An incoming `sse_message` or `sse_state_change` from the *old* conversation
  reaches the *new* atom and is accepted (no phase guard on those event types)

The structural fix is not to remove the `setTimeout` — it is to give events
a notion of *which connection generation produced them*. Events without that
notion are trusted by sequence number, and sequence numbers reset to 0 on
the new atom. This is the "wrong state should not be representable" rule
from the project's correctness guide, applied to SSE event identity.

## Context

Relevant files:
- `ui/src/hooks/useConnection.ts` — primary surface
- `ui/src/conversation/ConversationStore.ts` — atom dispatch path; needs to
  reject mismatched-epoch events
- `src/api/sse.rs` — server-side; verify whether the wire format already
  carries something epoch-like (likely not; this is a client-side concept)

## Plan

### Phase 1: Add `connectionEpoch` to the connection state machine

- Each `OPEN_SSE` mints a monotonic `epoch: number`, e.g.
  `state.epochCounter + 1`. Every connection in this tab gets a unique
  epoch. Counter is per-machine, not global — collisions across machines
  don't matter because epochs are only compared within their own machine.
- Machine context carries `currentEpoch: number`.

### Phase 2: Stamp incoming events with their connection's epoch

In the EventSource `onmessage` handler, capture the epoch in scope at
subscription time:

```ts
const epochAtSubscribe = currentEpoch;
es.onmessage = (msg) => {
  dispatchMachine({ type: 'SSE_EVENT', event: msg, epoch: epochAtSubscribe });
};
```

The `epochAtSubscribe` closure is stable for the lifetime of this
EventSource. When `OPEN_SSE` runs again it produces a new closure with a
new epoch — old EventSource handlers retain their old epoch.

### Phase 3: Reject mismatched-epoch events at dispatch

- When the state-machine effect dispatches an SSE event into the atom,
  include the epoch.
- Atom dispatch checks `event.epoch === atom.connectionEpoch`. Mismatch →
  drop the event with a `tracing::debug!`-equivalent log per the project's
  "capability gaps are logged, not silenced" convention.
- Atom is updated with the new epoch on `OPEN_SSE` success. (When does
  `connectionEpoch` get bumped on the atom side? Cleanest: the
  state-machine effect that fires `OPEN_SSE` also dispatches a synthetic
  `CONNECTION_OPENED` action into the atom carrying the new epoch.)

### Phase 4: Clean up the executor indirection

With epoch protection in place, the
`setTimeout(() => executeEffectsRef.current(...), 0)` indirection inside
`setMachineState`'s functional updater (`useConnection.ts:200-205`) is no
longer load-bearing for ordering safety — it was structurally preventing
the cross-atom write, but the epoch check now does that explicitly.

Drop the side-effect-in-functional-updater anti-pattern: the executor runs
synchronously after `setMachineState` returns (or via a clean `useEffect`
that fires on state change). Functional updaters become pure again.

This also fixes the StrictMode double-invocation duplicate-timer bug
(panel Concurrency finding #2) — duplicate timer scheduling was a symptom
of running effects inside the updater.

### Phase 5: Tests

- **Unit test** for the state machine: events with mismatched epochs are
  rejected by the atom dispatch path.
- **Integration test**: open conv A's SSE, navigate to conv B before the
  next tick, fire a synthetic A-event into the buffer; verify it does NOT
  land in B's atom. (Likely needs an EventSource shim in tests — there is
  precedent in the codebase; mirror it.)
- Existing `useConnection` tests pass without modification.
- **StrictMode regression**: render the hook in `<StrictMode>`, verify
  no duplicate `SCHEDULE_RETRY` timers fire on reconnect.

## Acceptance Criteria

- Every dispatched SSE event carries the epoch of the connection that
  produced it.
- The atom rejects events with `epoch !== atom.connectionEpoch`, logged
  at debug.
- The `setTimeout(0)` inside the functional updater is gone; the executor
  runs in a clean effect or synchronously after `setMachineState` returns.
- New cross-conversation contamination test passes (verifies an A-event
  arriving during navigation does not land in B's atom).
- StrictMode does not produce duplicate retry timers.
- `./dev.py check` passes.

## Independence

This task can run in parallel with task A (ChainPage migration). It
touches `useConnection` and `ConversationStore`'s atom dispatch path;
ChainPage changes do not intersect.
