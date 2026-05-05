---
created: 2026-05-05
priority: p2
status: ready
artifact: src/api/sse.rs
---

Force-close every SSE stream after a bounded lifetime so HTTP/1.1
connection saturation between client and server is self-healing.

## Symptom

Multiple Phoenix tabs/devices behind a single proxy can saturate the
proxy's HTTP/1.1 connection pool to phoenix. Each open conversation page
holds a `/api/conversations/:id/stream` SSE — long-lived by design.
When the pool fills, new requests (e.g. POST `/approve-task`) queue at
the proxy and "pend forever" from the browser's view.

Phoenix logs look healthy throughout — only successful API polls — because
the queued requests never reach phoenix at all.

## Root cause

`sse_stream()` in `src/api/sse.rs:40` produces a stream with
`KeepAlive` set but no maximum lifetime. SSE connections live until the
client gives up, which for a foregrounded tab is effectively forever.

## Fix

Cap stream lifetime (e.g. 30 minutes) and force-close. The browser's
`EventSource` auto-reconnects, briefly freeing the connection slot at
the proxy and any intermediates. This makes saturation self-healing
without changing the protocol or requiring HTTP/2.

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

Companion task: shutdown-sse-deadline (bound the same problem at process
exit). Bigger-but-conditional alternative not pursued here: HTTP/2 (h2c),
which multiplexes streams over a single TCP connection — only helps if
the proxy speaks HTTP/2 to phoenix downstream. Keeping HTTP/2 on the
shelf pending proxy investigation.

Discovered 2026-05-05 during prod incident; full diagnosis lives in the
companion task and conversation history.
