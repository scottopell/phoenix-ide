Wire the server-side SSE ReplayRing into the `Init` event payload so reconnecting clients receive the pending-events list and can resume mid-turn views.

Phase 2 of the SSE ReplayRing rollout. Builds on commit c5d91af (SseBroadcaster ring buffer landed in #74).

Scope:
- Extend `SseEvent::Init` and `SseWireEvent::Init` (crates/phoenix-ide/src/runtime.rs + src/api/wire.rs) with three new fields:
    `pending_anchor_sequence_id: i64`
    `pending_events: Vec<SseWireEvent>` (or `Vec<SseEvent>` converted at wire layer)
    `pending_truncated: bool`
- Update `init_event` construction in `crates/phoenix-ide/src/api/handlers.rs` to call `broadcast_tx.snapshot_pending()` atomically with the DB read and populate the new fields.
- Regenerate `ui/src/generated/` via `./dev.py codegen`.
- Update valibot schema in `ui/src/sseSchemas.ts`.
- Update `legacy_sse_event_to_json` parity test in `crates/phoenix-ide/src/api/sse.rs:106` to include the new fields; assert byte-for-byte parity with typed path.
- Add a Rust integration test (in `crates/phoenix-ide/src/runtime/testing.rs` or similar): start runtime, emit several ephemeral events without persisting a Message, subscribe a fresh stream, assert init carries the events.
- Flip relevant 🚧 status rows in `specs/sse_wire/executive.md` to ✅ once landed.

Acceptance:
- `./dev.py check` clean.
- Connecting to a conversation mid-LLM-turn shows the in-flight assistant message (the one broadcast eagerly via `send_ephemeral_message`) in the init payload.
- Connecting to a conversation post-ring-overflow shows `pending_events: []` and `pending_truncated: true`.

Out of scope: client-side handling of `pending_events` (Phase 3, separate task).
