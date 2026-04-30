---
created: 2026-04-22
priority: p2
status: done
artifact: src/api/sse.rs
---

Split `SseEvent::Message` into `Message` (new) + `MessageUpdated` (mutate) AND close the reconnect-window gap where update-in-place broadcasts are lost when the client is disconnected at broadcast time. Both fixes must land together — the type split alone doesn't replay missed updates.

## Two bugs, one task

### Bug 1 — overloaded `SseEvent::Message` (structural)

`SseEvent::Message { message: db::Message }` fires for two semantically distinct events:
1. A new message was inserted (advances the conversation sequence cursor).
2. An existing message mutated (display_data refresh, content rewrite) — the broadcast re-sends the full row with its ORIGINAL sequence_id.

The UI reducer's monotonic cursor guard silently dropped case 2 until the surgical fix in `ui/src/conversation/atom.ts` (shipped 2026-04-22) detected updates via runtime `message_id` lookup. That works but relies on introspection of an overloaded event — a future update path (patch enrichment, long-running tool progress, etc.) will have to remember to go through the runtime-detect branch. Correct-by-construction: make the two cases structurally distinct.

### Bug 2 — reconnect-window gap (operational)

Even with the surgical fix, a client that disconnects between the broadcast and the next re-render gets stuck showing stale state forever (full page reload is the only recovery). Path:

- `src/runtime/executor.rs:persist_sub_agent_results` broadcasts `SseEvent::Message` with the updated row — but into a tokio broadcast channel whose receiver was dropped when SSE disconnected. Event is lost.
- On reconnect, `useConnection.ts:149-151` appends `?after=lastSequenceId` to the stream URL.
- `src/db.rs:895-911` `get_messages_after` filters strictly by `sequence_id > after`. The updated message's row still has its original (low) `sequence_id`, so it's excluded from the delta.
- Atom's `lastSequenceId` persists across EventSource reconnects (lives in router-level context), so even short network blips trigger this.
- `src/api/sse.rs:23-26` `BroadcastStream` discards lagged events; no replay mechanism.

Symptom: spawn widget frozen on "Spawning N sub-agent(s)" forever, no summary, even though subsequent LLM turns reference the results correctly (DB has the update, `build_llm_messages_static` reads from DB).

**Severity:** no data integrity risk, but high user-disorientation cost. Hits anyone who backgrounds a tab during multi-minute sub-agent runs — a common pattern.

## Proposal (both bugs)

### Part A — split the event type

```rust
// src/runtime.rs (or wherever SseEvent lives)
pub enum SseEvent {
    Message { message: db::Message },          // strictly NEW
    MessageUpdated {
        message_id: String,
        // Only the mutable subset — message_id/sequence_id/created_at are immutable.
        display_data: Option<serde_json::Value>,
        content: Option<MessageContent>,
    },
    // ... existing variants unchanged
}
```

Backend (`src/runtime/executor.rs:persist_sub_agent_results`): stop calling `get_message_by_id` + `SseEvent::Message`. Send `SseEvent::MessageUpdated { message_id, display_data: Some(json), content: Some(content) }` instead.

Frontend `ui/src/conversation/atom.ts` reducer:

```ts
case 'sse_message_updated': {
  const idx = atom.messages.findIndex(m => m.message_id === action.messageId);
  if (idx < 0) return atom; // unknown id: ignore rather than create a ghost
  const merged = {
    ...atom.messages[idx]!,
    ...(action.displayData !== undefined && { display_data: action.displayData }),
    ...(action.content !== undefined && { content: action.content }),
  };
  const newMessages = [...atom.messages];
  newMessages[idx] = merged;
  return { ...atom, messages: newMessages };
  // Does not touch lastSequenceId or streamingBuffer.
}
```

Wire through `ui/src/hooks/useConnection.ts` as a new `message_updated` SSE event type.

Revert the runtime-detect branch in the `sse_message` case — it's replaced by the dedicated action. The `sse_message` case returns to strict monotonic semantics (new messages only).

### Part B — close the reconnect gap

Use the init payload to deliver a current snapshot of any mutable state that can't be reconstructed from the sequence cursor. Two viable shapes — pick one:

**Option 1 (recommended): full message resync on reconnect.** Change `stream_conversation` in `src/api/handlers.rs:1177-1183` to always return the full message list (drop the `?after` branch on this endpoint). The init event already carries messages; `sse_init` reducer in `atom.ts:159-219` already dedups and merges. Bandwidth cost: minor — messages are small, turns are finite, and Phoenix conversations don't typically grow past a few hundred messages. This is the simplest correct fix and eliminates the bug class entirely.

**Option 2: preserve delta but add an "updates-since" replay.** Keep `get_messages_after` for the new-message delta. Add a separate query that returns any message whose current `display_data` or `content` differs from what the client could have derived at `lastSequenceId`. Requires tracking an `updated_at` or version column (migration); more complex, more surface area. Reject unless Option 1 proves too expensive on measurement.

Implement Option 1 unless Scott explicitly requests Option 2. The `?after` query parameter can stay in the API for backward compat but should be ignored (or deprecated outright).

## Why this is correct-by-construction

- After the split, `SseEvent::Message` unambiguously means "new" and `SseEvent::MessageUpdated` unambiguously means "mutate". The compiler enforces the distinction on both sides of the wire.
- After the resync change, reconnects can never silently miss mutable state — the init payload always reflects current DB truth.
- Together, the two fixes eliminate the class of bugs where SSE delivery failure (dropped broadcast) corrupts client state. Either the event arrived (live update), or the init will overwrite (reconnect resync) — no path where stale state persists.

## Acceptance

Backend:
- `SseEvent::Message` definition no longer carries updates (only new rows).
- New `SseEvent::MessageUpdated { message_id, display_data, content }` variant, serialized as SSE event type `message_updated`.
- `persist_sub_agent_results` uses the new variant; no more `get_message_by_id` + `SseEvent::Message` pattern for updates.
- `stream_conversation` returns full messages in init (Option 1). `?after` query param may remain parseable but must not affect the init payload.

Frontend:
- `useConnection.ts` listens for `message_updated` events and dispatches `sse_message_updated`.
- `atom.ts` `sse_message` case: revert to strict monotonic guard; remove the runtime `message_id` lookup (it's moving to the new case).
- `atom.ts` new `sse_message_updated` case as sketched above.
- `sse_init` merge path (`atom.ts:173-182`) updated so that when the payload is a full resync (not delta), existing messages with the same `message_id` are **replaced in-place**, not skipped as duplicates. Current code skips on message_id match — that must flip.

Tests:
- Existing regression test in `ui/src/conversation/atom.test.ts` for the spawn_agents update must pass via `sse_message_updated` action.
- New test: reconnect path — simulate disconnect, update-in-place on server, reconnect; assert the updated `display_data` lands via `sse_init` merge.
- New test: `sse_message` strict-monotonic guard no longer has the update-in-place branch (regression prevention).
- `cargo test` + `./dev.py check` all green.

UX:
- No visible behavior change in the happy path.
- Confirmed via dev server: disconnect browser network tab during a sub-agent run, let it complete, reconnect — summary appears without full reload.

## Non-goals

- Not adding `updated_at` column / Option 2 unless Option 1 proves inadequate.
- Not touching unrelated SSE event variants (StateChange, Token, etc.).
- Not changing sub-agent streaming UX (SubAgentStatus widget, CompletedSubAgent rows).
