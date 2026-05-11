# simplify-scoped-sse-connection-ownership

## Plan

## Summary

Remove redundant conversation-scope layers before adding guard code.

Main move: make `useConnection` one scoped owner for active conversation connection state, and delete unused atom-level connection mirror plumbing where possible.

## Context

Frontend root problem is not “missing helper.” Problem is too many partial owners:

- route slug owns page identity
- atom key owns conversation state
- `useConnection` owns live socket + retry machine
- atom also mirrors `connectionState`
- queue drain trusts `useConnection` state
- page has extra `conversationIdForSSE` debounce state
- cache effects trust whatever atom currently shows

Task 08683 fixed message/atom contamination with SSE epoch stamping. Good. But stale EventSource callbacks can still drive `useConnection` local machine (`SSE_OPEN`, `SSE_ERROR`, retry timers), which controls StateBar and queue drain.

Also found redundant layer: `atom.connectionState` is written by `connection_state` actions, but visible UI uses `connectionInfo.state` from `useConnection`. `useConversationSelectors` exposes `atom.connectionState`, but no consumer uses that selector. This mirror likely adds stale-scope surface without value.

## What to do

1. **Delete or shrink atom connection mirror**
   - Verify no real consumer depends on `atom.connectionState` / `connection_state`.
   - Remove `connectionState` from `ConversationAtom` if dead.
   - Remove `connection_state` action and reducer branch if dead.
   - Remove `epochStampedDispatch` uses for synthetic connection-state actions.
   - Keep epoch only where needed for real SSE-originated atom mutations.

2. **Remove `conversationIdForSSE` debounce if not justified**
   - Re-check why 100ms delay exists.
   - Prefer direct `useConnection({ conversationId })` if tests/perf allow.
   - If delay must stay, make it internal to `useConnection`, not page-level identity state.

3. **Make `useConnection` reset by conversation identity**
   - On `conversationId` change, force machine back to disconnected/connecting cleanly.
   - Old connection handlers must not drive new local machine state.
   - Prefer captured `convId` equality check over new generic lease abstraction.
   - Guard only local machine effects: stale `init`, native `error`, retry timer, reconnected display timer.

4. **Queue drain trust active connection only**
   - Ensure queue drain cannot run from stale A connection state after navigation to B.
   - Ideally falls out from fixed `useConnection` identity reset/guard.

5. **Keep existing atom epoch guard for real SSE events**
   - Do not replace working 08683 protection unless deletion proves safe.
   - After removing atom `connection_state`, epoch guard only protects real wire mutations.

6. **Update task 01002 checklist**
   - Mark fixed/already-safe/needs-follow-up with file+test evidence.
   - Capture any non-SSE leftovers separately.

## Tests

Add or update tests for:

- stale A `init` after switch to B does not set B-visible connection state connected
- stale A native `error` after switch to B does not set B-visible reconnecting/offline
- stale A retry timer after switch to B opens no extra/current wrong connection
- B queue drains only from B current connection state
- existing stale A message cannot land in B atom still passes
- if atom `connectionState` removed, deleted tests replaced with `useConnection` behavior tests

## Acceptance criteria

- Less duplicated ownership than before: no unused atom connection-state mirror remains.
- No new broad abstraction unless deletion path fails.
- `useConnection` local machine state is scoped to current `conversationId`.
- Queue drain cannot be triggered by stale connection state.
- Task 01002 checklist completed with evidence.
- `./dev.py check` passes.
- Rust change not expected; if any Rust touched, run `./dev.py restart` and report URL.

## Progress

- Implemented in PR #39 ("Simplify scoped SSE connection ownership"). The `conversationIdForSSE`
  page-level debounce was removed; `useConnection` resets by `conversationId`; queue drain is
  scoped to the active connection. The `atom.connectionState` mirror was deliberately kept — it
  has a live consumer via `connectionInfo.state` → `StateBar` — which the task allowed ("if
  dead"). Status had been left `in-progress` — flipped to `done`.

