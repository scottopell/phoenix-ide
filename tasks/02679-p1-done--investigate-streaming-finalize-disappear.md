---
created: 2026-04-23
priority: p1
status: done
artifact: src/api/sse.rs
---

# Streaming response finalizes then visually disappears

## Symptom

User sends a message. State correctly advances to the LLM turn (purple
chrome, stop button). Token-by-token streaming is visible in the UI
(the `streamingBuffer` path in `atom.ts` — user sees characters
accumulating). Then the response "finalizes" — streaming stops — and
**the entire assistant message disappears from the view**.

Manually refreshing the page resurrects it: the final assistant
message shows up as expected. So the content is persisted in SQLite;
the loss is strictly in the client's in-memory state between
finalization and refresh.

First observed on prod on 2026-04-23 after deploy `b820092` (full SSE
robustness stack: 02674 / 02675 / 02676 / 02677 / 02678). Still
reproducible after the broadcast-lag close fix in `736b37d`
(`fix(sse): close stream on broadcast lag + bump capacity 128 → 4096`).

## What's been ruled in / ruled out

The reducer in `ui/src/conversation/atom.ts` was formalized as an
Allium spec in `68b7336` (`specs/conversation_atom/conversation_atom.allium`).
The spec names the atomic triple for `sse_message` (append +
clear-buffer + advance-seq) and the phase-coupling invariant for
`streamingBuffer`. Distillation concluded:

- **Reducer is structurally sound** for the known cases. The atomic
  triple is implemented as a single return in one `applyIfNewer` call
  — React atomicity by construction.
- The reducer CANNOT recover from "sse_agent_done fires without a
  preceding sse_message" — it's outside the reducer's authority, only
  a reconnect + sse_init merge can fix it.

Three candidate failure shapes were named in the spec distillation:

1. `sse_message` dropped by `applyIfNewer` guard (seq ≤ lastSequenceId).
   Would require the server's sequence counter to have advanced past
   the message's own seq. Unusual; no obvious mechanism.
2. **`sse_message` lost during a reconnect gap** (the leading
   hypothesis — see below).
3. `sse_agent_done` server-emitted before `sse_message` (server-side
   ordering bug). Not investigated in detail.

## Leading hypothesis: emit-vs-persist race

The fix in `736b37d` closes the SSE stream on
`BroadcastStreamRecvError::Lagged` so the client reconnects and
resyncs via `sse_init`. The reconnect works, `sse_init` merges from
DB, phase settles.

**But**: tokio's `broadcast::channel` does NOT buffer events for
future subscribers. Any event broadcast while the client's receiver
is dropped is silently emitted-into-void for that client. The new
subscriber (after reconnect) starts receiving from its subscribe
point forward.

If the server's finalization path is ordered as

```
broadcast_tx.send_message(message);   // broadcast first
db.put_message(message).await;        // persist after
```

and the client reconnects between these two steps (or even just
after broadcast but before the DB commit is visible to a read-txn),
then:

- The broadcast of `sse_message` reached nobody (old subscriber is
  already dropped from the lag-close).
- The `sse_init` handler reads DB, doesn't yet see the message,
  returns without it.
- The client subscribes fresh, starts receiving from seq N+1 onward —
  but `sse_message` was at seq N. Gone forever from this client's SSE
  stream.
- Eventually the DB commit lands. Refresh picks it up via a fresh
  `sse_init`. User sees the message.

**This fits the symptom exactly.**

## Investigation plan

1. Read the executor's finalization path in `src/runtime/executor.rs`
   (and adjacent files) to confirm the actual order of
   `send_message` vs `db.put_message`. Specifically look for where
   `SseEvent::Message` is broadcast for assistant messages at turn end.

2. Check `~/.phoenix-ide/prod.log` for `"SSE broadcast lagged"`
   warnings during a reproduction. If the hypothesis is right, every
   occurrence of the symptom should line up with a lag warning (the
   lag is what triggers the close → reconnect → race).

3. Consider adding a "broadcast replay buffer" on the server: a
   per-conversation ring of the last N events keyed by sequence_id.
   On reconnect, the client sends its `lastSequenceId`, server
   replays ring entries > that seq, then subscribes to live. Closes
   the race by construction. Heavier fix.

4. Lighter alternative: **persist to DB before broadcasting**. Reverse
   the order in the executor. Broadcast becomes strictly "a
   notification that this persisted event exists." Reconnecting
   client's `sse_init` always sees the message.

## Likely fix shape

Option (4) is the smallest viable fix. One function: find the
executor's `send_message(msg)` calls and ensure they follow
`db.put_message(msg).await`. Verify with a regression test that
simulates a close-and-reconnect between broadcast and persist (or
just asserts ordering).

Option (3) is the correct-by-construction fix, but it's a real
architectural change — broadcast channels don't have ring-replay
natively, and building one requires per-conversation state that
integrates with `SseBroadcaster`.

## Out of scope

- Reducer changes. The reducer is correct; the spec proves it.
- Client-side recovery beyond what `sse_init` already does.
- Renegotiating the broadcast channel capacity (already bumped to
  4096 in `736b37d`).

## Related work

- `68b7336` — `specs/conversation_atom/conversation_atom.allium`
  distills the reducer contract and names the failure shapes above.
- `736b37d` — broadcast-lag close + capacity bump. Fixed the *other*
  symptom (no responses ever arrived) but exposed this finalization-
  disappears shape more visibly because the close-reconnect dance
  happens more often now.
- `02675`, `02676`, `02677`, `02678` — the SSE robustness stack that
  set up the total-order discipline this task assumes.

## Spec follow-ups that would close this bug's class

- **Task 02680** (`distill-sse-wire-allium-spec`, p2) — the most
  directly relevant. Names the persist-before-broadcast invariant
  that is this bug's proposed fix. With that spec in place, the
  emit-vs-persist race is a spec violation, not an open mystery.
- **Task 02681** (`distill-connection-machine-allium-spec`, p2) —
  formalizes the "close → reconnect → OPEN_SSE → init" chain that
  the lag-close fix relies on. Guards against future regressions in
  the reconnect path that would silently turn the lag-close into
  the very bug it was designed to prevent.
- **Task 02682** (`distill-user-message-queue-allium-spec`, p3) —
  unrelated to this bug but completes the set of client-side SSE
  specs recommended from the conversation that produced this task.

## Resolution (2026-04-24)

Fixed in commit `e1175ce` on main: **fix(sse): pre-allocate message seq
from broadcaster before DB write (task 02679)**.

### The actual bug (distinct from "option 4" in the plan above)

The investigation plan proposed "persist to DB before broadcasting" as
the lighter-weight fix. Reading the executor, the code *already* did
that: every `send_message` was preceded by `storage.add_message(...)`.
A literal happens-before read of PersistBeforeBroadcast was satisfied.

The real bug is at one level deeper: **sequence-ID allocation**, not
write ordering.

- `db.add_message` allocated `sequence_id` via `SELECT MAX(seq)+1 FROM
  messages` — a per-conversation message counter rooted in the DB.
- `SseBroadcaster` has its own atomic counter that ephemeral events
  (state_change, init, tokens, errors) increment via `next_seq()`.
- These counters diverge: after a streaming LLM turn with many tokens,
  `SseBroadcaster`'s counter sits at N ≫ DB message count. The
  finalizing assistant message persists with DB seq ≈ `DB_count` ≪ N,
  then `send_message` broadcasts it with that stale seq.
- Client's `applyIfNewer` guard (`lastSequenceId = N`) drops the
  message. Page refresh resurrects it because the fresh `init` reads
  from DB, where it is persisted.

Observed on prod via an `EventSource`-intercepting SSE log:
`init seq=3 last_seq=3, token seq=4, token seq=5, message seq=2
(dropped), state_change seq=6, agent_done seq=7` — the AI response
vanishes visually despite being persisted.

### Fix

Pre-allocate `sequence_id` from `SseBroadcaster::next_seq()` *before*
the DB write; persist via a new `Database::add_message_with_seq(seq,
...)`. The message's own seq is strictly greater than every ephemeral
event emitted earlier. `applyIfNewer` accepts it.

- 7 call sites in `src/runtime/executor.rs` + 1 in
  `src/api/lifecycle_handlers.rs` converted.
- `MessageStore` trait extended with `add_message_with_seq` (plus
  `Arc<T>`, `DatabaseStorage`, `InMemoryStorage` impls).
- Non-broadcasting call sites (sub-agent bootstrap message in
  `spawn_sub_agent`, crash-recovery restart marker) keep the plain
  `add_message` — their seq-allocation race is benign because no
  client ever observes their seq out of order.

### Regression tests

- `db::tests::test_add_message_with_seq_writes_caller_seq`
- `runtime::broadcaster_tests::next_seq_after_ephemeral_events_exceeds_prior_events`
- `runtime::broadcaster_tests::observe_seq_is_idempotent_when_counter_already_past`
- `runtime::broadcaster_tests::observe_seq_catches_up_when_db_seq_leapfrogs`

647 tests pass (+4 new). `./dev.py check` 12/12 pass.

### Spec implications

The `PersistBeforeBroadcast` invariant in
`specs/sse_wire/sse_wire.allium` (on the `task-24694-distill-sse-wire-
allium-spec` branch, not yet merged to main) correctly captures this
bug as a spec violation: the join entity `StreamMessage` existing
implies a `PersistedMessage` exists. Pre-fix, that held at a moment
in time (the DB write did complete before the broadcast), but the
*client's effective ordering* is by `sequence_id`, and a stale seq
made the persisted message invisible. The fix restores alignment
between the structural invariant and the observable client state.

A follow-up task should update the allium temporal-ordering feedback
(also on that branch) to note this lesson: "happens-before" intuitions
that don't explicitly model the sequence-ID watermark miss
sequence-allocation races like this one. The feedback item's existing
workaround (join entity) survived contact with the real bug.
