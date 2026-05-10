# fix-evict-runtime-sse-stranding

## Plan

# Fix: SSE clients stranded on dead broadcaster after evict_runtime

## Summary

After `evict_runtime()` is called (currently only from the model upgrade endpoint), the old conversation runtime task continues running indefinitely while new events go to a new broadcaster. SSE clients connected to the old broadcaster receive only axum keep-alive pings (every 15s) but no real events. They appear stuck until the user refreshes (cmd+R).

## Root cause

In `src/runtime.rs:get_or_create()`, the new runtime is created with a **fresh** `SseBroadcaster`:
```rust
let broadcaster = SseBroadcaster::new(SSE_BROADCAST_CAPACITY, initial_last_seq); // new channel
```

But existing SSE clients subscribed to the old runtime's `broadcast_rx` have no way to know a new channel exists. The old runtime holds `broadcaster_old.clone()` (line 955), so `broadcast_tx` is never dropped → the old BroadcastStream never ends → the axum SSE response never ends → EventSource never fires `error` → no reconnect.

The old runtime is also deadlocked: it holds its own `event_tx` clone (line 954), so `event_rx.recv()` never returns `None`, and `run()` never returns.

## Fix

### Option A (recommended): Signal old runtime to shut down on eviction

Add a `Shutdown` event variant to `Event`. In `evict_runtime()`, before removing from the HashMap, send `Shutdown` to the old runtime's `event_tx`. The runtime handles `Shutdown` by returning from `run()`. When `run()` returns, the runtime is dropped → `broadcaster_old.clone()` dropped. The HashMap entry removal also drops its clone. Now `broadcast_tx` is fully dropped → `BroadcastStream` ends → SSE response ends → EventSource fires error → client reconnects → `get_or_create()` → new runtime → `Init`.

```rust
pub async fn evict_runtime(&self, conversation_id: &str) {
    let runtimes = self.runtimes.write().await;
    if let Some(handle) = runtimes.get(conversation_id) {
        let _ = handle.event_tx.send(Event::Shutdown).await;
    }
    runtimes.remove(conversation_id);
}
```

In the executor loop, handle `Event::Shutdown` by returning immediately.

### Option B (alternative): Reuse the old broadcaster for the new runtime

When `get_or_create()` is called after eviction, pass the old broadcaster (if we can find it) to the new runtime. SSE clients stay subscribed to the same channel. But this requires the evicted broadcaster to be stored somewhere between eviction and the next `get_or_create()`.

### Option A is simpler and cleaner.

## Acceptance criteria

- After model upgrade, existing SSE clients detect the connection ended (within a few seconds) and automatically reconnect.
- On reconnect, client gets `Init` with all messages including the auto-resume system message and any agent responses.
- Old runtime task exits cleanly after `Shutdown` is received.
- No infinite deadlock in old runtime's `select!` loop.

## Files

- `src/runtime.rs` — `evict_runtime()`, `Event` enum, executor loop handling
- `src/runtime/executor.rs` — `run()` loop, `Shutdown` event handling


## Progress

