---
created: 2026-04-22
priority: p1
status: ready
artifact: src/terminal/relay.rs, src/terminal/ws.rs
---

# Terminal WS keepalive — reap sessions stranded by ungraceful client disconnects

## Summary

Add an application-level WebSocket ping/pong with dead-peer timeout to the
terminal relay so sessions abandoned by the browser (tab crash, network drop,
laptop sleep) are detected and reaped on the server. Today they are not, and
the user's only recovery is for someone to `kill` the stranded shell PID.

## Context

Observed on prod 2026-04-22 with conversation
`f15964e8-d336-4afc-8e08-3ab91a19e852`:

- Session started 2026-04-21T21:57:23Z (shell PID 3675100).
- No matching `Terminal session ended` log for ~20h.
- Every reconnect attempt logged
  `WARN Terminal: 409 — session already active` at `src/terminal/ws.rs:63`.
- Shell process was still alive (`ps` showed zsh -i, Ss+, 20:44 elapsed,
  PPID = phoenix-ide server). Manual `kill -HUP <pid>` triggered PTY EIO →
  `run_relay` returned `PtyEof` → cleanup ran → HashMap entry cleared.

### Why `run_relay` never exits

`run_relay` in `src/terminal/relay.rs` exits only on:

1. PTY EOF/EIO (shell dies) — REQ-TERM-007 `PtyEof`.
2. `ws_incoming.next() == None` — `WsClosed`.
3. `stop_rx` signal — `Stopped`.

When the browser disappears without sending a Close frame (TCP reset never
delivered, NAT / middlebox silently dropping the flow, OS suspended, tab
killed), the server-side read side of the WebSocket blocks forever. The relay
loop is alive, the shell is alive, the HashMap entry in `ActiveTerminals`
persists indefinitely, and the guard at `src/terminal/ws.rs:63` rejects every
reconnect with a text frame the UI ignores.

### Why the UI makes it look like "the click does nothing"

`TerminalPanel.tsx` `onmessage` (around line 459) only handles `0x00`-prefixed
binary frames. The server's 409 response is `Message::Text("error: terminal
already active")`, which is silently dropped. The server then returns and the
socket is closed, firing `ws.onclose` → `setActivity('disconnected')` → the
"Shell exited — click to start a new one" prompt redraws. From the user's
perspective, clicking did nothing.

## Acceptance Criteria

- [ ] Relay sends WS `Ping` frames on a fixed cadence (default 30s; make it
      configurable via env or a constant in `src/terminal/relay.rs`).
- [ ] If no `Pong` is received within a deadline (default 60s / 2 missed
      pings), the relay treats the peer as dead: fires `stop_tx`, returns
      `RelayExit::WsClosed` (or a new `RelayExit::PeerTimeout` variant),
      cleanup at `ws.rs:206-213` runs, HashMap entry is removed, child is
      SIGHUP'd and reaped.
- [ ] Test in `src/terminal/relay.rs` that simulates a silent peer: no frames
      sent, no close — relay must exit within the deadline. Use
      `tokio::io::duplex` + a manually driven ws_incoming stream that blocks.
- [ ] Regression test: a peer that pongs on time keeps the session alive past
      the deadline (no false positive).
- [ ] Manual verification: close the browser tab abruptly (DevTools →
      "Disable cache" + offline + close tab); within ~60s the prod log
      shows `Terminal session ended` and a fresh reconnect succeeds without
      a server restart.

## Notes

### Secondary issue — out of scope here but linked

The 409 guard itself is a footgun: its only signal to the client is a text
frame the client doesn't render, and there's no affordance to forcibly reclaim
a stuck session. Once this task ships, stuck sessions become rare (bounded
by the ping deadline). If we still see reclaim issues after that, file a
follow-up to either (a) have the UI render text frames inline in xterm, or
(b) accept the reclaim and evict the old handle.

### Implementation hints

- Axum's `axum::extract::ws::Message::Ping(Vec<u8>)` / `Message::Pong(Vec<u8>)`
  are the right frames. A `tokio::time::interval` in a `select!` branch of
  `relay_writer` (or a new sibling task) is the natural home.
- The reader side needs to observe incoming `Pong` frames. Today
  `handle_socket` strips non-binary frames before they reach the relay
  (`src/terminal/ws.rs:172-180` — the `ws_in` adapter filters to binary).
  The ping/pong plumbing has to live above that filter, or the filter has to
  learn about pong frames. Prefer doing it in `ws.rs` so `run_relay` stays
  protocol-agnostic; pass a liveness signal into the relay via `stop_rx`.
- OS-level TCP keepalive is a weaker fallback (default timers are ~2h on
  Linux and can be blocked by middleboxes); not a substitute.

### Related files

- `src/terminal/ws.rs` — upgrade handler, 409 guard, cleanup.
- `src/terminal/relay.rs` — the relay loop that needs the liveness check.
- `src/terminal/session.rs` — `ActiveTerminals` HashMap.
- `ui/src/components/TerminalPanel.tsx` — `onmessage` drops text frames
  silently; `ws.onclose` produces the "Shell exited" state.
