# SSE Wire Protocol — Executive Summary

## Requirements Summary

The SSE wire protocol is the server's contract for streaming conversation state to connected clients. It governs what events the server emits, in what order, and under what conditions a stream is closed. The protocol is the dual of the client-side reducer (`specs/conversation_atom/`) — together they define how a UI's view of a conversation stays consistent with the server's truth across reloads, reconnects, and message bursts.

The event types in scope are: `init` (the snapshot delivered on subscribe), `message` (a newly persisted message), `message_updated` (a mutation to an existing message's fields), `state_change` (a phase transition), `token` (an LLM streaming chunk), `llm_first_byte` and `llm_attempt` (LLM request lifecycle markers), `agent_done` (turn completion), `conversation_became_terminal` (terminal state reached), `conversation_hard_deleted` (row removed), `conversation_update` (partial metadata change), `browser_session_state` (browser-session liveness edge), `steer_message_queued` (a queued steering message), `rate_limit_snapshot` (mid-stream quota detail), and `error` (a user-facing error). Two ordering invariants are load-bearing: every delivered message event has a backing committed-to-DB row (PersistBeforeBroadcast), and `init` is always the first event on a stream (InitAlwaysFirst). Together they guarantee that a reconnecting client's fresh snapshot can never be behind a message that was already broadcast to a now-dead stream.

A per-conversation `ReplayRing` buffers ephemeral events (tokens, state_changes, message_updates, the LLM lifecycle markers, and eager-broadcast in-flight assistant messages) between persisted-Message anchors. The ring contents ride into the `init` snapshot's `pending_events` field, and the client reducer replays them through its existing per-event rules. A client reconnecting mid-turn therefore resumes the in-flight view — streaming text, current tool, eager assistant card — instead of blanking out until the next checkpoint. The ring is bounded by `replay_ring_capacity` (512 entries); overflow clears the ring and sets `truncated`, forcing the next subscribe to do a full DB-only resync. A restarted server loses the ring (in-memory only); the client falls back to DB-only reconstruction, which is correct because no events are in flight if the process restarted.

## Technical Summary

The protocol is grounded in two server-side primitives:

1. **`SseBroadcaster`** (`crates/phoenix-ide/src/runtime.rs`) wraps a tokio broadcast channel plus a monotonic sequence counter (`AtomicI64`). Senders use either `next_seq()` to allocate or `send_persisted_message(...)` / `send_seq(...)` to allocate-and-emit. `observe_seq(seq)` lets the persisted-message path fold the DB-allocated sequence ID back into the counter so a later `next_seq()` is strictly greater. A pre-allocated sequence ID is the wire-level handle the client uses for replay-suppression and ordering.

2. **`sse_stream`** (`crates/phoenix-ide/src/api/sse.rs`) is the per-client handler that emits `init` first (a DB snapshot taken at subscribe time, including `last_sequence_id` and the ring's pending events), then forwards broadcast events. On `BroadcastStreamRecvError::Lagged` — the broadcast channel ring buffer wrapped because the client fell too far behind — the stream closes; the client's connection machine reconnects and the next `init` resyncs from the DB.

The persist-before-broadcast ordering is enforced at the call site: `db.save_message()` runs to completion before `broadcast_tx.send_persisted_message(message)` fires, so any message that reaches a stream is already committed. A reconnecting client's fresh `init` snapshot therefore cannot see a "missing" message that was broadcast to a now-dead stream — the DB read will include it.

Tokens are *ephemeral*: not persisted as DB rows, and not reconstructed from the DB on `init`. They do carry a `sequence_id` allocated from the broadcaster's shared counter (like every broadcast event except per-subscriber `init`), and they ride the `ReplayRing` into `init.pending_events` so a reconnect mid-stream resumes the in-flight streaming buffer. The shared counter is also what lets the client's `applyIfNewer` guard drop any token already superseded by a later event.

The protocol is type-checked end-to-end via `ts-rs` codegen (`SseWireEvent` in `crates/phoenix-ide/src/api/wire.rs` → `ui/src/generated/`); valibot schemas in `ui/src/sseSchemas.ts` are annotated `satisfies v.GenericSchema<unknown, WireInitData>`, so a Rust-side change surfaces as a tsc error until the schema is updated. Byte-for-byte wire parity is guarded by `parity_*` tests in `src/api/sse.rs`.

## Code Anchors

The `.allium` spec is the authoritative behavioural contract; every rule and invariant is backed by code. This table maps each to its implementation anchor for the reader who wants to cross-reference.

| Rule / Invariant | Code anchor |
|---|---|
| **StreamOpened** (init delivered first) | `crates/phoenix-ide/src/api/sse.rs` — `init_event` is the first item in the stream |
| **LagCloseStream** (close on broadcast lag) | `crates/phoenix-ide/src/api/sse.rs` — `BroadcastStreamRecvError::Lagged` returns `None`, ending the stream |
| **MessageCommittedToDb → MessageBroadcast** (persist-then-broadcast) | `crates/phoenix-ide/src/runtime.rs` — `SseBroadcaster::send_persisted_message` is called only after `db.save_message` returns Ok |
| **MessageUpdatedBroadcast** | `SseWireEvent::MessageUpdated` in `crates/phoenix-ide/src/api/wire.rs`; emitted on field mutations |
| **StateChangeBroadcast** | Emitted via `broadcast_tx.send_seq` on `ConvState` transitions (`runtime.rs`) |
| **ConversationBecameTerminalBroadcast** | Terminal-event emission tied to bedrock REQ-BED-007 |
| **ConversationUpdateBroadcast** | Partial-metadata SSE event; client merges shallowly |
| **TokenBroadcast** (ephemeral, not persisted) | Token streaming via `broadcast_tx.send_seq`; tokens carry a `sequence_id` and ride the `ReplayRing`, but are never reconstructed from the DB |
| **AgentDoneBroadcast** | Emitted after final message persist+broadcast on turn completion |
| **ErrorBroadcast** | `UserFacingError` payload carries flat string + typed kind |
| **PersistBeforeBroadcast** (invariant) | `SseBroadcaster::send_persisted_message` (`runtime.rs`) takes an already-persisted message; the call site is the enabling condition |
| **InitAlwaysFirst** (invariant) | `init_event` is the first stream item in `sse_stream` (`api/sse.rs`) — no path delivers events before init |
| **SequencesNonNegative** (invariant) | `SseBroadcaster::next_seq` starts at the DB watermark (always ≥ 0) and only increments via `fetch_add(1)` |
| **PersistedMessageClearsReplayRing** | `SseBroadcaster::send_persisted_message` (`runtime.rs`) — anchor reset on persisted Message broadcast |
| **EphemeralEventAppendedToReplayRing** | `SseBroadcaster::send_seq` (`runtime.rs`) — appends on broadcast; overflow clears the ring and sets `truncated` |
| **EagerAssistantMessageAppendedToReplayRing** | `SseBroadcaster::send_ephemeral_message` (`runtime.rs`); called from `Effect::BroadcastAssistantMessage` |
| **ReplayRingBounded** (invariant) | `ReplayRing::append` (`runtime.rs`) clears + sets truncated when `entries.len() >= REPLAY_RING_CAPACITY` |
| **ReplayRingEntriesAboveAnchor** (invariant) | Anchor reset clears entries simultaneously; `next_seq` strictly exceeds the most recent observed seq |
| **ReplayRingEntriesOrdered** (invariant) | `VecDeque::push_back` + head-only eviction preserves seq order (and `next_seq` is monotonic) |
| **InitSnapshotMirrorsRing** (invariant) | `crates/phoenix-ide/src/api/handlers.rs` — `init_event` construction calls `broadcast_tx.snapshot_pending()` and threads `pending_anchor_sequence_id` / `pending_events` / `pending_truncated` into `SseEvent::Init` |

The client reducer (`ui/src/conversation/atom.ts`) is the dual of the ring: on a fresh connect it seeds `lastSequenceId` from `pending_anchor_sequence_id` so the per-event `applyIfNewer` guard accepts the pending entries; on reconnect it preserves the live floor so already-observed entries drop as replays; a `max(current, payload.last_sequence_id)` safety belt covers the truncated case where the ring overflowed and `pending_events` is empty.

## Cross-Spec Relationships

- **`specs/conversation_atom/`**: client-side reducer, the dual of this spec. `applyIfNewer` is the client-side counterpart to the server's monotonic sequence allocation.
- **`specs/connection_machine/`**: client-side reconnect lifecycle. Together with `LagCloseStream` here, it closes the loop on broadcast-lag recovery.
- **`specs/bedrock/`**: the source of `state_change`, `agent_done`, and `conversation_became_terminal` events. Bedrock owns the state machine; this spec owns the wire envelope.
- **`specs/chains/`**: a parallel broadcaster (`ChainBroadcaster` in `crates/phoenix-ide/src/chain_runtime.rs`) for chain Q&A streams. Reuses the sequence-id pattern but has no reconnect-replay obligation (the persisted `chain_qa` row is canonical for late readers).
