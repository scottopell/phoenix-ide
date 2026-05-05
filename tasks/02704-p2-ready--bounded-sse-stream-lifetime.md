---
created: 2026-05-05
priority: p2
status: ready
artifact: src/api/sse.rs
---

Force-close every SSE stream after a bounded lifetime so HTTP/1.1
connection exhaustion is self-healing without protocol changes.

## Symptom

The browser's connection to phoenix can wedge: new requests (e.g. POST
`/approve-task`, fresh page loads, asset fetches) "pend forever" in
network devtools, while phoenix's logs show only successful API polls.
The queued requests never reach phoenix at all.

## Root cause

The path is direct: browser ↔ phoenix over plain HTTP/1.1 (no proxy
terminating between them). HTTP/1.1 caps a browser at **6 concurrent
connections per origin**. Each open conversation page holds one SSE
stream (`/api/conversations/:id/stream`) — a long-lived connection that
permanently occupies one of those 6 slots.

With ~6 conversation tabs open, the slot pool is full. The 7th request
queues in the browser's connection scheduler and never gets dispatched.
From the browser's perspective: "pending forever." From phoenix's: it
never happens.

`sse_stream()` in `src/api/sse.rs:40` sets `KeepAlive` but no maximum
lifetime, so SSE connections live until the client gives up — which
for a foregrounded tab is effectively never.

## Fix

Cap stream lifetime (e.g. 30 minutes) and force-close from the server.
The browser's `EventSource` auto-reconnects, freeing the slot
momentarily and letting any queued requests drain. Saturation becomes
self-healing without changing the protocol.

Existing `ConnectionMachine` on the client (referenced in
`src/api/sse.rs:25`) already handles broadcast-lag-induced closes;
extending it to handle a server-initiated lifetime close should be
mostly free.

## Done When

- [ ] SSE stream auto-closes after N minutes (configurable, default 30m)
- [ ] Client reconnects cleanly (`EventSource` does this for free; verify
      `ConnectionMachine` resyncs state without dropping events)
- [ ] Reconnect interval is jittered to avoid thundering-herd if many
      streams expire at once
- [ ] Test: a single SSE stream survives multiple lifetime windows by
      reconnecting; no event loss

## Notes

Companion tasks:
- shutdown-sse-deadline (02703): bound the same hang at process exit
- http2-tls (TBD): structural fix that eliminates the 6-connection
  limit entirely via HTTP/2 stream multiplexing — bigger project
  (requires TLS) and only worth doing if the lifetime fix here proves
  insufficient.

Discovered 2026-05-05 during a prod incident triggered by clicking
"Approve task" with multiple conversation tabs open. Full diagnosis is
in the companion tasks and conversation history.
