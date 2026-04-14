---
created: 2026-04-14
priority: p1
status: ready
artifact: src/runtime/executor.rs, ui/src/conversation/atom.ts
---

# Streaming message appears to "repeatedly deliver itself" (real-provider)

## User-reported symptom

> "When new messages stream in, the same message gets stuck repeatedly
> delivering itself, it seems like an impossible infinite scrolling
> container sometimes."

- Occurs with a **real** provider (not mock)
- **Reloading the page clears it** — authoritative server state is fine,
  the dup lives only in the client's in-memory atom
- Bugged surface: the message list during / just after streaming

## Investigation

I was unable to reproduce this against the `mock` provider (tokens and
message events flow through the same in-memory broadcast channel with
effectively zero delay). The symptom only manifesting on real providers
is a strong tell that **timing** is the root cause. Walking the stack
turned up two plausible causes, one of which is an outright race that
already exists in the code.

### Primary hypothesis — backend token-forwarder race

The LLM streaming path in `src/runtime/executor.rs` forwards tokens to
the SSE broadcast channel from a **separate spawned task** (the "token
forwarder") that reads from the provider's `TokenChunk` channel. The
main task that broadcasts the final `SseEvent::Message` does not wait
for the forwarder to drain.

Concretely:

```rust
// src/runtime/executor.rs:870-890  (token forwarder, simplified)
tokio::spawn(async move {
    let mut rx = chunk_rx;
    loop {
        match rx.recv().await {
            Ok(TokenChunk::Text(text)) => {
                let _ = broadcast_tx_for_tokens.send(
                    SseEvent::Token { text, request_id: request_id_for_fwd.clone() },
                );
            }
            Err(broadcast::error::RecvError::Closed) => break,
            ...
        }
    }
});

// src/runtime/executor.rs:892-975  (main LLM task, simplified)
let handle = tokio::spawn(async move {
    ...
    let llm_outcome = llm_client.complete_streaming(&request, &chunk_tx).await;
    // chunk_tx dropped here — closes the forwarder's rx
    let _ = llm_tx.send(llm_outcome);
});

// executor.rs:979-983  (oneshot → unified outcome channel)
tokio::spawn(async move {
    if let Ok(llm_outcome) = llm_rx.await {
        let _ = outcome_tx.send(EffectOutcome::Llm(llm_outcome)).await;
    }
});

// ...then the main executor loop pulls from outcome_rx, persists the
// assistant message, and broadcasts SseEvent::Message to
// self.broadcast_tx — on an entirely different task.
```

Three independent tokio tasks are sharing `self.broadcast_tx`:

1. **Forwarder task** — sends `SseEvent::Token` for each chunk
2. **Main LLM task** — does `llm_tx.send(outcome)` then exits (drops
   `chunk_tx`, closing the forwarder's receiver)
3. **Main executor loop** — receives the outcome, builds and persists
   the assistant message, broadcasts `SseEvent::Message`

There is **no synchronization** between task (1) draining and task
(3) broadcasting the Message event. With a real provider, network
jitter and larger chunks make it easy for the last Token chunk to
arrive at the forwarder *after* the main loop has already sent
`SseEvent::Message`. Order on the broadcast channel is FIFO, so SSE
consumers see:

```
Token, Token, Token, Message, Token  ← this is the bug
```

The client reducer (`ui/src/conversation/atom.ts:300-315`) handles
`sse_token` unconditionally:

```ts
case 'sse_token': {
  if (atom.streamingBuffer && atom.streamingBuffer.lastSequence >= action.sequence) {
    return atom;
  }
  return {
    ...atom,
    streamingBuffer: {
      text: (atom.streamingBuffer?.text ?? '') + action.delta,
      lastSequence: action.sequence,
      startedAt: atom.streamingBuffer?.startedAt ?? Date.now(),
    },
  };
}
```

Because `sse_message` (`atom.ts:205-251`) clears `streamingBuffer` to
`null`, a late-arriving Token creates a **fresh** streaming buffer
with `text = '' + action.delta`. The message list now renders:

- The just-persisted assistant message (from `sse_message`)
- A "ghost" streaming message below it, containing the trailing
  chunk(s) from the same response

The user's eye reads this as "the message is repeating itself."
Because subsequent real Token events keep arriving if the model is
still streaming another message (or if re-use happens across turns
without a clean reset), the ghost can grow — explaining the "impossible
infinite scrolling container" description.

**Reload clears it** because the streaming buffer is client-only,
non-persisted state; on refresh the atom is rebuilt from the DB which
has exactly one copy of the message.

### Secondary hypothesis — defensive gap in `sse_init` merge

On reconnect (`?after=N`), the reducer blindly concatenates:

```ts
// ui/src/conversation/atom.ts:164-166
const mergedMessages =
  atom.lastSequenceId > 0 ? [...atom.messages, ...p.messages] : p.messages;
```

No `message_id` dedup, no `sequence_id > atom.lastSequenceId` filter.
If `p.messages` ever overlaps `atom.messages` — for any reason: backend
off-by-one, race on `get_last_sequence_id` vs `get_messages_after`, a
future regression — users see **every overlapping message twice** with
no recovery except reload.

Compare to the `sse_message` path (`atom.ts:205-251`), which is
bulletproof:

```ts
if (atom.lastSequenceId >= action.sequenceId) return atom;
const existingIdx = atom.messages.findIndex(
  (m) => m.message_id === action.message.message_id,
);
```

This is a latent bug even if no current code path triggers it. It
belongs to the "every component should dedup defensively, not rely on
upstream correctness" discipline.

## Proposed fix

### Primary: close the backend race

Option A (minimal, recommended): **await the forwarder task** before
broadcasting `Message`. The forwarder's `JoinHandle` is currently
dropped; instead, store it and `await` it right after the LLM task
completes, before the main loop is allowed to broadcast Message. This
guarantees all `Token` events are flushed first.

```rust
let forwarder_handle = tokio::spawn(async move { /* ... */ });
// ... later, after the LLM task sends llm_outcome ...
let _ = forwarder_handle.await;  // wait for drain before Message can ship
```

Option B: send a sentinel chunk (`TokenChunk::EndOfStream`) on close
and have the forwarder reply via a oneshot when it's seen.

Option C (correct-by-construction per AGENTS.md): collapse the forwarder
into the main task, so Token and Message events are broadcast from the
same task and their ordering is a local property of one code path, not
a race between tasks. This eliminates the class of bug.

Option C is the best long-term — it removes the ability to represent
the buggy interleaving at all. Options A/B are safer as targeted
backports if C is too invasive.

### Secondary: client-side guards

Both are cheap and belong independent of the backend fix.

1. **`sse_token` phase guard.** Only create a `streamingBuffer` when the
   current phase is `llm_requesting`. Late tokens that arrive while
   phase is `idle` / `tool_executing` / etc. are dropped.

   ```ts
   case 'sse_token': {
     if (atom.phase.type !== 'llm_requesting') return atom;  // drop late tokens
     // ... rest unchanged
   }
   ```

2. **`sse_init` dedup.** Filter the merged messages by sequence_id and
   message_id:

   ```ts
   const existingIds = new Set(atom.messages.map((m) => m.message_id));
   const delta = p.messages.filter(
     (m) => m.sequence_id > atom.lastSequenceId && !existingIds.has(m.message_id),
   );
   const mergedMessages =
     atom.lastSequenceId > 0 ? [...atom.messages, ...delta] : p.messages;
   ```

## Repro attempts

- Tried: mock provider, 20ms-per-chunk streaming. Too fast/tight for
  the race to trigger in a single-machine single-process setup. No
  visible duplication.
- Not tried (would fire reliably): real Anthropic streaming with a
  500-token response. Agent SDK clients on the SEND side of the
  broadcast channel do not simulate network latency so the mock's
  reliability isn't comparable.
- Also not tried: forcibly pausing the token forwarder task via a
  sleep injection + running the mock. This would make the task deterministic but requires touching executor.rs.

## Related

- Task `08321-p1-done` — earlier investigation of message-list
  duplication. Fixed a different root cause (`MessageList` vs
  `VirtualizedMessageList` drift); shared `MessageComponents.tsx`
  module. This bug is a new failure mode in the streaming pipeline,
  not the same bug recurring.
- Task `0592-p2-ready` (qaplan) — streaming markdown rendering QA plan.
  Tests 1 and 7 would currently miss this bug because the QA uses
  local mock/one-shot providers, not a real network-latency stream.
- AGENTS.md "Correct-by-construction is the governing principle" —
  Option C for the backend fix is the instance of this rule.

## Done when

- [ ] A provider + executor test deterministically reproduces the
      `Token → Message → Token` ordering on `broadcast_tx` with a
      synthetic chunk-forwarding delay
- [ ] The reproduction fails after the backend fix
- [ ] `sse_token` reducer drops tokens when phase is not
      `llm_requesting` (add unit test)
- [ ] `sse_init` reducer dedups by sequence_id and message_id (add
      unit test)
- [ ] Manual smoke: send a 500-token message to a real Claude / GPT
      provider, watch the message list, confirm no ghost/dup
