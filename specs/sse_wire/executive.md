# SSE Wire Protocol — Executive Summary

## Requirements Summary

The SSE wire protocol is the server's contract for streaming conversation state to connected clients. It governs what events the server emits, in what order, and under what conditions a stream is closed. The protocol is the dual of the client-side reducer (`specs/conversation_atom/`) — together they define how a UI's view of a conversation stays consistent with the server's truth across reloads, reconnects, and message bursts.

Nine event types are in scope: `init` (the snapshot delivered on subscribe), `message` (a newly persisted message), `message_updated` (a mutation to an existing message's fields), `state_change` (a phase transition), `token` (an LLM streaming chunk), `agent_done` (turn completion), `conversation_became_terminal` (terminal state reached), `conversation_update` (partial metadata change), and `error` (a user-facing error). Two ordering invariants are load-bearing: every delivered message event has a backing committed-to-DB row (PersistBeforeBroadcast), and `init` is always the first event on a stream (InitAlwaysFirst). Both invariants exist because we shipped a class of bugs (task 02679: "streaming finalizes then disappears") that the structural ordering catches.

## Technical Summary

The protocol is grounded in two server-side primitives:

1. **`SseBroadcaster`** (`crates/phoenix-ide/src/runtime.rs:155-250`) wraps a tokio broadcast channel plus a monotonic sequence counter (`AtomicI64`). Senders use either `next_seq()` to allocate or `send_message(...)` / `send_seq(...)` to allocate-and-emit atomically. `observe_seq(seq)` lets non-message senders fold their sequence ID back into the counter so a later `next_seq()` is strictly greater. A pre-allocated sequence ID is the wire-level handle the client uses for replay-suppression and ordering.

2. **`sse_stream`** (`crates/phoenix-ide/src/api/sse.rs:21+`) is the per-client handler that emits `init` first (a DB snapshot taken at subscribe time, including `last_sequence_id`), then forwards broadcast events. On `BroadcastStreamRecvError::Lagged` — the broadcast channel ring buffer wrapped because the client fell too far behind — the stream closes; the client's connection machine reconnects and the next `init` resyncs from the DB.

The persist-before-broadcast ordering is enforced at the call site: `db.save_message()` runs to completion before `broadcast_tx.send_message(message)` fires, so any message that reaches a stream is already committed. A reconnecting client's fresh `init` snapshot therefore cannot see a "missing" message that was broadcast to a now-dead stream — the DB read will include it.

Tokens are explicitly *ephemeral*: not persisted, not in `init` snapshots, sequence IDs share the broadcaster's counter so the client's `applyIfNewer` guard silently drops tokens that arrive after a reconnect.

The protocol is type-checked end-to-end via `ts-rs` codegen (`SseWireEvent` in `crates/phoenix-ide/src/api/wire.rs` → `ui/src/generated/`); valibot schemas in `ui/src/sseSchemas.ts` are annotated `satisfies v.GenericSchema<unknown, WireInitData>`, so a Rust-side change surfaces as a tsc error until the schema is updated. Byte-for-byte wire parity is guarded by `parity_*` tests in `src/api/sse.rs`.

## Status Summary

The `.allium` spec was distilled from a working, deployed implementation, so all rules and invariants are backed by code. The status table maps each rule/invariant to its anchor.

| Rule / Invariant | Status | Code anchor |
|---|---|---|
| **StreamOpened** (init delivered first) | ✅ Complete | `crates/phoenix-ide/src/api/sse.rs:42-47` (`init_event` is the first item in the stream) |
| **LagCloseStream** (close on broadcast lag) | ✅ Complete | `crates/phoenix-ide/src/api/sse.rs:52-64` (`BroadcastStreamRecvError::Lagged` returns `None`, ending the stream) |
| **MessageCommittedToDb → MessageBroadcast** (persist-then-broadcast) | ✅ Complete | `crates/phoenix-ide/src/runtime.rs:228-250` (`SseBroadcaster::send_message` is called only after `db.save_message` returns Ok) |
| **MessageUpdatedBroadcast** | ✅ Complete | `SseWireEvent::MessageUpdated` in `crates/phoenix-ide/src/api/wire.rs`; emitted on field mutations |
| **StateChangeBroadcast** | ✅ Complete | Emitted via `broadcast_tx.send_seq` on `ConvState` transitions (`runtime.rs`) |
| **ConversationBecameTerminalBroadcast** | ✅ Complete | Terminal-event emission tied to bedrock REQ-BED-007 |
| **ConversationUpdateBroadcast** | ✅ Complete | Partial-metadata SSE event; client merges shallowly |
| **TokenBroadcast** (ephemeral, not persisted) | ✅ Complete | Token streaming via `broadcast_tx.send_seq`; tokens excluded from `init` snapshots |
| **AgentDoneBroadcast** | ✅ Complete | Emitted after final message persist+broadcast on turn completion |
| **ErrorBroadcast** | ✅ Complete | `UserFacingError` payload carries flat string + typed kind |
| **PersistBeforeBroadcast** (invariant) | ✅ Enforced by construction | `SseBroadcaster::send_message` (`runtime.rs:228`) takes an already-persisted message; the call site is the enabling condition |
| **InitAlwaysFirst** (invariant) | ✅ Enforced by construction | `init_event` is hard-coded as the first stream item in `sse_stream` (`api/sse.rs:46-47`) — no path delivers events before init |
| **SequencesNonNegative** (invariant) | ✅ Enforced by construction | `SseBroadcaster::next_seq` starts at the DB watermark (always ≥ 0) and only increments via `fetch_add(1)` |

**Progress:** All rules and invariants implemented. The spec was distilled in response to task 02679 (streaming-finalize-disappear bug); the `PersistBeforeBroadcast` invariant is the formal statement of the leading fix hypothesis and now serves as a guardrail against regressions.

## Cross-Spec Relationships

- **`specs/conversation_atom/`**: client-side reducer, the dual of this spec. `applyIfNewer` is the client-side counterpart to the server's monotonic sequence allocation.
- **`specs/connection_machine/`**: client-side reconnect lifecycle. Together with `LagCloseStream` here, it closes the loop on broadcast-lag recovery.
- **`specs/bedrock/`**: the source of `state_change`, `agent_done`, and `conversation_became_terminal` events. Bedrock owns the state machine; this spec owns the wire envelope.
- **`specs/chains/`**: a parallel broadcaster (`ChainBroadcaster` in `crates/phoenix-ide/src/chain_runtime.rs`) for chain Q&A streams. Reuses the sequence-id pattern but has no reconnect-replay obligation (the persisted `chain_qa` row is canonical for late readers).
