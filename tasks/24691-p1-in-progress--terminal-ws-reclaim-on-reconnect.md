---
created: 2026-04-22
priority: p1
status: in-progress
artifact: src/terminal/ws.rs, src/terminal/relay.rs, src/terminal/session.rs
---

# Terminal WS reclaim — recover sessions when WS connections hang

## Summary

When the browser-side WebSocket silently dies (tab crash, network drop,
laptop sleep), the server's terminal relay has no way to know — it blocks
forever waiting for frames that never come. The shell and PTY stay healthy;
only the WS connection to the frontend is stuck. Today the 409 guard at
`src/terminal/ws.rs:63` rejects every reconnect attempt, leaving the user
with no recovery short of a server-side `kill`.

Fix this by letting a new WS connection reclaim an existing terminal session:
signal the old relay to detach (without tearing down the shell) and hand the
still-healthy PTY to the new relay.

## Context

Observed on prod 2026-04-22 with conversation
`f15964e8-d336-4afc-8e08-3ab91a19e852`:

- Session started 2026-04-21T21:57:23Z (shell PID 3675100).
- No matching `Terminal session ended` log for ~20h.
- Every reconnect attempt logged
  `WARN Terminal: 409 — session already active` at `src/terminal/ws.rs:63`.
- Shell process was still alive and healthy (`ps` showed zsh -i, Ss+,
  PPID = phoenix-ide server). Manual `kill -HUP <pid>` was the only recovery.

### Why the UI makes it look like "the click does nothing"

`TerminalPanel.tsx` `onmessage` (around line 459) only handles `0x00`-prefixed
binary frames. The server's 409 response is `Message::Text(...)` which is
silently dropped. The socket then closes, firing `ws.onclose` →
`setActivity('disconnected')` → the "Shell exited" prompt redraws. From the
user's perspective, clicking does nothing.

### Why reclaim is the right primary fix

The shell and PTY are healthy. Only the WS is stuck. A dead-peer timeout
(ping/pong) can reap the stale relay eventually, but makes the user wait 60s+
for something they want right now. Reclaim makes reconnect take over the
existing session immediately.

The existing code structure already supports this with small changes:

- `PtyMasterIo` (`relay.rs:51`) is constructed from a `dup()` of the master
  fd (`ws.rs:123`). Each relay owns its own fd; successive relays can each
  dup the same underlying master without disturbing it.
- `CommandTracker` lives in `TerminalHandle`, not the relay — naturally
  preserved across reconnects.
- `ActiveTerminals` holds `Arc<TerminalHandle>`. The shell dies when the
  Arc refcount hits zero, not when any particular WS closes.

The one real issue today: `ws.rs:206-212` unconditionally tears down the
registry entry on every WS exit, which conflates WS close with session
teardown. That's the behavior to split.

## Design

### Core change: branch cleanup on exit reason

Today `RelayExit` has three variants (`PtyEof`, `WsClosed`, `Stopped`) but the
handler runs the same teardown for all three. Split by cause:

| Exit reason | Action |
|---|---|
| `PtyEof` | Full teardown: shell died, reap child, remove entry, close fd. |
| `Stopped` via conversation terminal (REQ-TERM-012) | Full teardown. |
| `Stopped` via reclaim request | Detach only: drop this relay's `pty_io` dup. Registry entry and master_fd stay alive. |
| `WsClosed` | Detach only. Shell stays alive awaiting future reclaim. |

The `stop_rx` channel currently carries `bool`. Change it to carry a small
enum so the relay can distinguish "tear down everything" from "just detach."

### Reclaim signaling

A new connection for an already-active `conv_id` must be able to signal the
existing relay to exit without dropping the shell. Add the stop channel and a
detach notification to `TerminalHandle`:

```rust
pub enum StopReason {
    Running,    // initial
    Detach,     // reclaim or WS close — keep shell alive
    TearDown,   // conversation terminal — shell must die
}

pub struct TerminalHandle {
    pub master_fd: OwnedFd,
    pub child_pid: Pid,
    pub tracker: Arc<Mutex<CommandTracker>>,
    pub shell_integration_status: Arc<Mutex<ShellIntegrationStatus>>,
    // New:
    pub stop_tx: tokio::sync::watch::Sender<StopReason>,
    pub detached: Arc<tokio::sync::Notify>,
}
```

`TerminalHandle` is the natural home because it already owns the per-session
shared state and is refcounted.

### Handler flow for a reconnect

Pseudocode for `handle_socket` on a conv_id that's already active:

```text
if let Some(existing) = terminals.get(&conv_id) {
    // Reclaim path
    existing.stop_tx.send(StopReason::Detach).ok();
    existing.detached.notified().await;  // old relay has exited
    // master_fd in handle is intact; dup a fresh pty_io from it.
    // Reset stop_tx back to Running before the new relay starts.
    // Run the new relay against the existing handle (tracker preserved).
} else {
    // Fresh session path (today's code, minus the unconditional teardown)
}
```

At the end of any relay (fresh or reclaimed):

```text
match (exit, stop_reason_at_exit) {
    (PtyEof, _)                 => full_teardown,
    (Stopped, TearDown)         => full_teardown,
    (Stopped, Detach)           => detach_only,
    (WsClosed, _)               => detach_only,
}

detach_only:
    notify arc_handle.detached (wake a waiting reclaimer, if any)
    drop local pty_io and the Arc<TerminalHandle> clone the handler held
    // The registry's Arc keeps the handle + fd alive.

full_teardown:
    terminals.remove(conv_id)
    drop arc_handle  // refcount → 0, master_fd closes, SIGHUP, child reaps
    waitpid
```

### REQ-TERM-012 teardown path

The existing broadcast subscriber at `ws.rs:147-162` fires
`teardown_stop.send(true)` when the conversation becomes terminal. Change that
to `send(StopReason::TearDown)` so the relay exits via the full-teardown arm.

## Acceptance Criteria

- [ ] A second WS connection for an already-active `conv_id` reclaims the
      existing session instead of returning 409. The new connection begins
      streaming PTY output immediately and can send input to the same shell.
- [ ] `CommandTracker` state survives reclaim (e.g. an in-progress command's
      record is still available after reconnect — no re-initialization).
- [ ] When a WS connection closes without `PtyEof` or conversation-terminal
      teardown, the shell process and `ActiveTerminals` entry remain alive.
      Verified by log inspection and registry state.
- [ ] When the shell exits (`PtyEof`), full teardown still runs: child
      reaped, registry entry removed, master_fd closed.
- [ ] When the conversation reaches a terminal state (REQ-TERM-012 path),
      full teardown runs regardless of WS state.
- [ ] Unit test in `src/terminal/relay.rs`: drive a relay via `Detach`;
      confirm it exits `Stopped`, tracker state is retained, and a second
      relay over a fresh `DuplexStream` pair can continue against the same
      tracker.
- [ ] Unit/integration test in `src/terminal/ws.rs` or `relay.rs`: simulate
      two sequential connections to the same `conv_id` — second one reclaims,
      first one exits cleanly without killing the (simulated) shell.
- [ ] Concurrency test: two reclaim attempts racing for the same session.
      Exactly one wins; the other either loses cleanly or reclaims the
      winner. No deadlock, no double-teardown.
- [ ] Regression test: `PtyEof` exit still triggers full teardown, not
      detach.
- [ ] `./dev.py check` passes (clippy + fmt + tests + task validation).
- [ ] Manual verification: close the browser tab abruptly, open a new tab
      for the same conversation within seconds — terminal reconnects to the
      same shell immediately, any in-flight command output continues
      streaming.

## Out of Scope

- WS keepalive / ping-pong. Needed as a background reaper for truly
  abandoned sessions (user never reconnects); file as a follow-up task. With
  reclaim in place the timer can be generous (minutes, not 60s) since the
  user-facing pain is gone.
- Server-side scrollback buffer — new xterm attaches blank until fresh PTY
  output arrives. Known limitation; separate, larger problem.
- UI text-frame rendering (the 409 error frame was silently dropped).
  Reclaim removes the 409 entirely, so this is moot.

## Related Files

- `src/terminal/ws.rs` — upgrade handler, 409 guard → reclaim branch,
  cleanup branching on exit reason.
- `src/terminal/relay.rs` — `RelayExit`, stop channel type change,
  writer/reader select arms.
- `src/terminal/session.rs` — `TerminalHandle` additions (`stop_tx`,
  `detached`, `StopReason` enum).
- `ui/src/components/TerminalPanel.tsx` — no change required; reclaim is
  server-side.

## Implementation Hints

- The resize callback in `ws.rs:184-187` captures `master_fd_raw`. Each
  reclaim builds a new callback from `arc_handle.master_fd.as_raw_fd()` —
  same fd, same semantics.
- `PtyMasterIo::new` expects the fd to already be `O_NONBLOCK`. The
  original master already has that flag set on first spawn; dups inherit
  it, so no extra `set_nonblocking` is needed on reclaim.
- When resetting `stop_tx` to `Running` before a new relay starts, send
  the value before handing `stop_rx` to the relay so no stale `Detach` is
  observed.
- Don't hold the `ActiveTerminals` mutex across `.await`. The reclaim path
  does `get()` (clones the Arc), drops the mutex, then awaits on the
  detach notification.
