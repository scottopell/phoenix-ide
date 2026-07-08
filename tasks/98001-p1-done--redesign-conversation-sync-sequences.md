# Redesign conversation sync around separate event and message sequences

## Executive summary

We mixed two clocks.

That broke tool cards.

Fix:

- Event clock tells us which mutations we have applied.
- Message clock tells us where transcript rows render.
- Event gaps trigger replay/resync.
- Message gaps render as missing ranges and can be fetched.
- Browser stores a real local replica.
- SSE sends live mutations.
- HTTP fills message holes.
- Tool cards always have an explicit state.
- Blank card is not a valid state.

## Problem

The missing tool-result-card bug exposed that the current frontend treats heterogeneous sequenced values as one global timeline. A high-sequence `MessageUpdated` event for tool-result duration advanced the client cursor, so later lower-sequence preallocated tool-result message rows were dropped as “old.” The database was correct, but the UI lost the result card.

The core issue is not just that one event was mishandled. The model conflates:

- persisted transcript message order
- live SSE event replay/dedupe order
- message update events
- conversation metadata events
- tool result messages
- display/telemetry updates such as duration

This task redesigns the model so that bug class becomes structurally impossible.

## Goals

1. Separate `event_sequence_id` from `message_sequence_id`.
   - SSE replay/dedupe uses event sequence.
   - Transcript ordering uses message sequence.
2. Do not use a single scalar high-water mark for heterogeneous entities.
   - Track `lastAppliedEventSeq` separately from message availability.
   - Treat event gaps explicitly; never skip over missing events silently.
3. Represent message availability as ranges.
   - The client can represent holes like “have 2810, 2811, and 2815, missing 2812–2814.”
4. Treat IndexedDB/browser state as a local replicated database.
   - Normalized messages, sequence index, tool projections, sync state, transport state.
   - Idempotent upserts.
   - Pending patches when updates arrive before creates.
5. Use SSE for live mutation events and HTTP for range recovery.
   - SSE: low-latency tail and small replay after `after_event_sequence`.
   - HTTP: latest window, older pages, around-sequence windows, exact missing ranges.
6. Make impossible UI states explicit.
   - A tool card renders as declared/running/completed/failed/missing_result.
   - Blank tool card is impossible.
7. Avoid full-history replacement as the normal sync path.
   - Initial page load hydrates a cached window, opens SSE after local event high-water, and fetches only missing ranges as needed.

## Non-goals

- Do not remove full-history/debug/export endpoints immediately.
- Do not rewrite all message rendering in one risky change unless necessary; migrate incrementally.
- Do not create a second independent source of truth for tool state. Tool records should be normalized projections from messages unless an event represents ephemeral lifecycle state not yet persisted in messages.

## Correct model

### Two timelines

`event_sequence_id` answers: “Has the client already applied this mutation?”

`message_sequence_id` answers: “Where does this transcript row render?”

They are independent and must not be compared.

### Event envelope

Keep transport metadata on the envelope only:

```ts
type SseEventEnvelope<T> = {
  conversation_id: string;
  event_sequence_id: number;
  transcript_generation: string;
  emitted_at: string;
  event: T;
};
```

Do not duplicate `event_sequence_id` inside `event` payloads.

### Message record

```ts
type MessageRecord = {
  message_id: string;
  conversation_id: string;
  message_sequence_id: number;
  role: 'user' | 'agent' | 'tool' | 'system' | 'error' | 'continuation';
  content: MessageContent;
  display_data?: Record<string, unknown>;
  usage_data?: UsageData;
  created_at: string;
  message_version: number;
};
```

`message_sequence_id` is transcript order only.

### Message patch shape

`message_version` is metadata, not patched content:

```ts
type MessagePatched = {
  type: 'message.patched';
  message_id: string;
  base_message_version?: number;
  message_version: number;
  patch: MessagePatch;
};

type MessagePatch = {
  content?: MessageContent;
  display_data?: Record<string, unknown>;
  usage_data?: Partial<UsageData>;
};
```

Patch semantics must be explicit:

- `content`, if present, replaces message content.
- `display_data`, if present, shallow-merges into existing display data.
- `usage_data`, if present, shallow-merges into existing usage data.
- If key deletion is needed, represent it explicitly; do not rely on ambiguous omission.
- Avoid parallel homes for the same value. If `duration_ms` is UI/display metadata, keep it inside `display_data.duration_ms`; if it is canonical message data, model it as one typed top-level field and do not also store it in `display_data`.

### Patch version semantics

Prefer simple self-contained patch semantics:

- Message patches are idempotent and self-contained enough to apply over any older message version.
- If incoming `message_version <= existing.message_version`, drop as stale.
- If incoming `message_version > existing.message_version + 1`, apply the patch but mark the message as possibly stale and schedule a message refresh.
- If a stricter contiguous version-chain model is chosen instead, buffer mismatched patches and fetch the message by id; do not silently apply out-of-chain patches.

The implementation must document which model it uses and enforce it in one reducer helper.

## Client replica state

The browser should store a normalized local replica, not a full `Message[]` cache blob.

```ts
type ConversationReplica = {
  conversationId: string;
  conversation: ConversationRecord | null;

  messagesById: Map<MessageId, MessageRecord>;
  messageIdBySeq: Map<MessageSeq, MessageId>;

  toolUsesByCallId: Map<ToolCallId, ToolUseProjection>;
  toolResultsByCallId: Map<ToolCallId, ToolResultProjection>;

  pendingMessagePatches: Map<MessageId, PendingMessagePatch[]>;
  pendingToolPatches: Map<ToolCallId, PendingToolPatch[]>;

  sync: ConversationSyncState;
  transport: ConversationTransportState;
};
```

### Sync state

```ts
type ConversationSyncState = {
  transcriptGeneration: string | null;

  lastAppliedEventSeq: number;
  bufferedEventEnvelopes: Map<number, SseEventEnvelope<ConversationEvent>>;
  eventGap: { start: number; end: number } | null;

  contiguousMessageHighWater: number | null;
  messageRanges: MessageRange[];

  serverEventTail: number | null;
  serverMessageTail: number | null;
};

type MessageRange = { start: number; end: number };
```

A scalar `lastAppliedEventSeq` is valid only with a strict contiguous event-application rule. It must never skip gaps.

## Reducer invariants

### Event application

```ts
function applyEnvelope(envelope: SseEventEnvelope<ConversationEvent>) {
  const eventSeq = envelope.event_sequence_id;

  if (sync.transcriptGeneration == null) {
    sync.transcriptGeneration = envelope.transcript_generation;
  }

  if (envelope.transcript_generation !== sync.transcriptGeneration) {
    enterGenerationResync(envelope.transcript_generation);
    return;
  }

  if (eventSeq <= sync.lastAppliedEventSeq) {
    return;
  }

  if (eventSeq !== sync.lastAppliedEventSeq + 1) {
    bufferEnvelope(envelope);
    markEventGap({
      start: sync.lastAppliedEventSeq + 1,
      end: eventSeq - 1,
    });
    requestEventReplayOrHttpResync();
    return;
  }

  applyConversationEvent(envelope.event);
  sync.lastAppliedEventSeq = eventSeq;
  drainBufferedContiguousEnvelopes();
}
```

Rules:

- Dedupe by `event_sequence_id` only.
- Apply only contiguous event sequences.
- If an event gap is observed, buffer the future event and enter replay/resync.
- Do not advance `lastAppliedEventSeq` across a gap.
- Event sequence advancement never implies message availability.

### Generation changes

Every SSE envelope and HTTP response carries `transcript_generation`.

If incoming generation differs from local generation:

- Stop applying incremental events.
- Mark local message ranges stale.
- Clear or quarantine pending patches that cannot safely apply.
- Fetch latest window.
- Reset sync state for the new generation only after recovery establishes a coherent baseline.

Initial null generation adopts the first server generation.

### Message create

```ts
function applyMessageCreated(event: MessageCreated) {
  const message = event.message;
  upsertMessage(message);
  indexMessageSequence(message);
  markMessageRangeAvailable(message.message_sequence_id);
  applyPendingMessagePatches(message.message_id);
  indexToolRecordsDerivedFromMessage(message);
}
```

Rules:

- Ordering uses `message_sequence_id` only.
- Within `(conversation_id, transcript_generation)`, `message_sequence_id` is unique.
- Within a conversation, `message_id` is unique.
- A `message_id` cannot move to a different `message_sequence_id` within the same generation. If this happens, enter resync.
- Duplicate `message_sequence_id` with different message IDs is corruption; enter resync.

### Message patch

```ts
function applyMessagePatched(event: MessagePatched) {
  const existing = messagesById.get(event.message_id);

  if (!existing) {
    pendingMessagePatches.add(event.message_id, event);
    return;
  }

  applyVersionedPatch(existing, event);
}
```

Rules:

- Update-before-create is valid.
- Patches for missing messages are stored by `message_id`.
- When the message later arrives, pending patches apply in event order/version order.
- Applying a patch never marks a message sequence available.

### Message availability and tombstones

Represent transcript slots explicitly:

```ts
type TranscriptSlot =
  | { kind: 'message'; sequence: number; messageId: string }
  | { kind: 'missing_range'; start: number; end: number }
  | { kind: 'tombstone'; sequence: number; message_id?: string; reason: 'deleted' | 'redacted' | 'compacted' }
  | { kind: 'streaming'; requestId: string };
```

Distinguish:

- missing range = client does not know what exists there
- tombstone = server says there is intentionally no renderable message there

A range response should return ordered items, not silent absence:

```ts
type MessageRangeItem =
  | { kind: 'message'; sequence: number; message: MessageRecord }
  | { kind: 'tombstone'; sequence: number; message_id?: string; reason: 'deleted' | 'redacted' | 'compacted' };
```

## Tool-card model

Tool state should primarily be a normalized projection derived from messages:

- `message.created` for an assistant message contains tool-use blocks.
- Client indexes those blocks into `toolUsesByCallId`.
- `message.created` for a tool-result message contains `tool_call_id`.
- Client indexes that into `toolResultsByCallId`.
- `tool_execution.patched` is only for ephemeral lifecycle state not yet represented by a persisted message.

Suggested event set:

```ts
type ConversationEvent =
  | { type: 'stream.init'; ... }
  | { type: 'message.created'; message: MessageRecord }
  | { type: 'message.patched'; message_id: string; base_message_version?: number; message_version: number; patch: MessagePatch }
  | { type: 'message.deleted'; message_id: string; message_sequence_id: number; tombstone: MessageTombstone }
  | { type: 'conversation.patched'; patch: ConversationPatch }
  | { type: 'tool_execution.patched'; tool_call_id: string; patch: ToolExecutionPatch }
  | { type: 'stream.token'; request_id: string; delta: string }
  | { type: 'stream.finalized'; request_id: string; message_id: string; message_sequence_id: number };
```

Avoid separate `tool_result.linked` unless needed. If used, it may arrive before either side exists and must be stored as a pending/partial relation keyed by `tool_call_id` and `result_message_id`.

Tool card selector must produce an exhaustive state:

```ts
type ToolCardState =
  | { kind: 'declared'; toolUse: ToolUseProjection }
  | { kind: 'running'; toolUse: ToolUseProjection }
  | { kind: 'completed'; toolUse: ToolUseProjection; result: ToolResultProjection }
  | { kind: 'failed'; toolUse: ToolUseProjection; error: string; result?: ToolResultProjection }
  | { kind: 'missing_result'; toolUse: ToolUseProjection; expectedResultMessageId?: string; expectedResultSequence?: number };
```

Rendering must exhaustively switch over this union. There is no blank/default state.

## HTTP contracts

All message-history HTTP parameters use message sequence names.

### Metadata

```txt
GET /api/conversations/by-slug/:slug/meta
```

Returns conversation metadata, `transcript_generation`, `server_event_tail`, and `server_message_tail`. No messages.

### Latest window

```txt
GET /api/conversations/:id/messages/latest?limit=100
```

### Before/after pages

```txt
GET /api/conversations/:id/messages?before_message_sequence=2810&limit=100
GET /api/conversations/:id/messages?after_message_sequence=2810&limit=100
```

Do not use ambiguous `after_sequence` for new APIs.

### Exact missing range

```txt
GET /api/conversations/:id/messages/range?start_message_sequence=2812&end_message_sequence=2814
```

Returns messages and tombstones. Silent holes are invalid.

### Around-sequence recovery

```txt
GET /api/conversations/:id/messages/around/2814?before=50&after=50
```

Useful for search, deep links, and missing tool-result recovery.

### Event replay

```txt
GET /api/conversations/:id/events?after_event_sequence=5000&limit=1000
```

Optional if SSE replay can cover reconnects; useful for explicit gap recovery.

## SSE contract

All stream parameters use event sequence names.

```txt
GET /api/conversations/:id/stream?after_event_sequence=1234&transcript_generation=abc
```

First event is `stream.init` metadata, not full transcript history:

```ts
type StreamInit = {
  type: 'stream.init';
  conversation: ConversationRecord;
  server_event_tail: number;
  server_message_tail: number | null;
  transcript_generation: string;
  replay:
    | { kind: 'current' }
    | { kind: 'replaying'; from_event_sequence_exclusive: number }
    | { kind: 'gap'; reason: 'cursor_too_old' | 'generation_changed' };
};
```

The server must deliver replay/live events in contiguous `event_sequence_id` order per conversation stream. If the client observes a gap, it buffers and enters replay/resync instead of applying out of order.

## Page load flow

1. Hydrate cached replica from IndexedDB.
2. If a cached latest/visible window exists, render it immediately.
3. Fetch conversation metadata.
4. If no cached latest window exists, or the local cached ranges do not cover the server tail, fetch `/messages/latest`.
5. Open SSE with `after_event_sequence=lastAppliedEventSeq`.
6. Apply replay only when event sequences are contiguous.
7. Fetch HTTP ranges for visible message holes, missing tool-result links, or generation recovery.
8. Persist applied messages/events/sync state to IndexedDB.
9. Notify only selectors whose underlying records changed.

## IndexedDB shape

Persist replica tables:

- `conversations`
- `messages`, keyed by `[conversation_id, message_id]`
- `message_sequence_index`, keyed by `[conversation_id, transcript_generation, message_sequence_id]`
- `message_ranges`, keyed by `[conversation_id, transcript_generation, start, end]`
- `tool_uses`, projection keyed by `[conversation_id, tool_call_id]`
- `tool_results`, projection keyed by `[conversation_id, tool_call_id]`
- `pending_message_patches`, keyed by `[conversation_id, message_id, event_sequence_id]`
- `sync_state`, keyed by `conversation_id`
- optional bounded `event_log`, keyed by `[conversation_id, event_sequence_id]`

## Observability

Add debug logging for event/message gaps and tool missing-result diagnostics. Include:

- `conversation_id`
- `transcript_generation`
- `lastAppliedEventSeq`
- incoming `event_sequence_id`
- `message_sequence_id`, when relevant
- `message_id`, when relevant
- `event_type`
- known message ranges
- missing message ranges
- `tool_call_id`, for tool-card issues
- expected result message ID/sequence, when known

Dev diagnostics for a missing tool result should say roughly:

```txt
Tool result missing for tool_call_id X.
Expected result message Y.
Known message ranges: 2810-2811, 2815-2815.
Last applied event seq: 9000.
```

## Migration path

### Phase 1: Boundary naming split

Add explicit `event_sequence_id` and `message_sequence_id` types/fields at the API boundary.

Compatibility rule:

- Legacy `sequence_id` may exist only as a boundary shim.
- Do not expose legacy `sequence_id` to new frontend code.
- Convert legacy values immediately into either `event_sequence_id` or `message_sequence_id`.

Done when new/changed code cannot accidentally pass a generic `sequence_id` through the conversation store or SSE reducer.

### Phase 2: Split client cursors

Replace global `lastSequenceId` semantics with:

- `lastAppliedEventSeq`
- event gap/buffer state
- `contiguousMessageHighWater`
- `messageRanges`

Existing full-history loads can initially populate one complete message range.

Done when message availability and event replay state are stored separately.

### Phase 3: Contiguous event reducer

Implement the envelope reducer rule:

- dedupe by event sequence only
- apply only `lastAppliedEventSeq + 1`
- buffer future events
- mark event gaps
- request replay/resync
- handle generation mismatch

Done when out-of-order events cannot silently advance the applied cursor.

### Phase 4: Safe message patches

Implement update-before-create handling:

- missing target message stores pending patch
- later message create applies pending patches
- patch application never marks message sequence available
- version semantics are centralized and tested

Done when a high-event-sequence `message.patched` cannot cause a lower-message-sequence `message.created` to be dropped.

### Phase 5: Explicit tool-card states

Normalize tool projections from messages and add exhaustive `ToolCardState` rendering.

Done when tool-use-with-missing-result renders a visible `missing_result` state rather than blank UI.

### Phase 6: HTTP range APIs

Add message range/window endpoints using explicit `*_message_sequence` parameter names:

- latest
- before
- after
- exact range
- around

Responses include messages and tombstones. Silent holes are invalid.

Done when the frontend has an HTTP recovery path for missing transcript ranges.

### Phase 7: IndexedDB replica persistence

Persist sync state, message ranges, sequence indexes, pending patches, and normalized records.

Done when reload can hydrate a known latest/visible window plus exact sync state from IndexedDB.

### Phase 8: Incremental load path

Change page load from full cache -> full HTTP -> full SSE init to:

- hydrate cached window
- fetch meta
- fetch latest only if cache does not cover useful tail
- open SSE with `after_event_sequence`
- fetch missing ranges as needed

Done when normal conversation open no longer requires full-history replacement.

### Phase 9: Metadata-only SSE init

Change SSE init to send metadata/replay status instead of full transcript.

Keep legacy full-init compatibility temporarily, but isolate it behind a boundary adapter.

Done when incremental SSE is the normal path and full init is a compatibility fallback only.

### Phase 10: Selector-based normalized rendering

Move React subscriptions from one full `messages: Message[]` field to normalized selectors:

- `useTranscriptSlots(conversationId, visibleRange)`
- `useMessage(messageId)`
- `useToolCardState(toolCallId)`
- `useConversationMeta(conversationId)`

Done when a single message update does not replace/rerender the whole transcript array.

### Phase 11: Retire full-history replacement from normal UI

Keep full-history fetches only for export/debug/emergency recovery/tests.

Done when normal UI load and reconnect rely on replica hydration, SSE events, and HTTP range recovery.

## Verification expectations

Add tests for:

- event 100, 102, 101 does not drop 101; 102 is buffered and gap recovery is requested
- `message.patched` before `message.created` stores a pending patch and later applies it
- high `event_sequence_id` patch does not advance message availability
- message holes render `missing_range`
- server tombstones render as tombstones, not missing ranges
- duplicate `message_sequence_id` with different IDs triggers resync
- same `message_id` moving sequence within a generation triggers resync
- generation mismatch enters resync and does not apply stale events
- tool use without loaded result renders `missing_result`
- SSE URL/API uses `after_event_sequence`, not ambiguous `after_sequence`
- message HTTP APIs use explicit message-sequence parameter names

## Files to inspect first

- `ui/src/conversation/atom.ts`
- `ui/src/conversation/ConversationStore.ts`
- `ui/src/conversation/useConversationAtom.ts`
- `ui/src/pages/ConversationPage.tsx`
- `ui/src/hooks/useConnection.ts`
- `ui/src/cache.ts`
- `ui/src/sseSchemas.ts`
- `ui/src/api.ts`
- `crates/phoenix-ide/src/api/wire.rs`
- `crates/phoenix-ide/src/api/sse.rs`
- `crates/phoenix-ide/src/api/handlers.rs`
- `crates/phoenix-ide/src/runtime.rs`
- relevant message/tool rendering components under `ui/src/components/`

## Notes

This is intentionally a multi-phase architectural task. The first correctness milestone is not full pagination or normalized rendering. The first correctness milestone is separating event replay from message transcript order and making update-before-create safe.
