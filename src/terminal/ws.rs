//! WebSocket endpoint handler for the terminal (REQ-TERM-004 through REQ-TERM-009).
//!
//! Binary frame protocol:
//!   byte 0 = 0x00 → PTY data (bidirectional)
//!   byte 0 = 0x01 → resize: u16be cols, u16be rows (client → server only)
//!
//! The relay logic (byte forwarding, command tracking) lives in `relay.rs` as
//! `run_relay`.  This module handles only the axum/WebSocket wiring: auth, 409
//! guard, PTY spawn, frame type filtering, and process lifecycle.

use super::relay::{run_relay, PtyMasterIo, RelayConfig, RelayExit};
use super::session::{ActiveTerminals, Dims, StopReason, TerminalHandle};
use super::spawn::{set_nonblocking, set_winsize_raw, spawn_pty};
use crate::api::AppState;
use crate::runtime::{RuntimeManager, SseEvent};
use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Path, State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use nix::sys::wait::waitpid;
use std::{
    os::unix::io::{AsRawFd, FromRawFd},
    sync::Arc,
    time::Duration,
};

/// Axum handler: `GET /api/conversations/:id/terminal` (WebSocket upgrade).
///
/// Auth is handled by the surrounding middleware before this function is called.
pub async fn terminal_ws_handler(
    ws: WebSocketUpgrade,
    Path(conversation_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let terminals = state.terminals.clone();
    let db = state.db.clone();
    let runtime = Arc::clone(&state.runtime);
    ws.on_upgrade(move |socket| handle_socket(socket, conversation_id, terminals, db, runtime))
}

#[allow(clippy::too_many_lines)] // inherently dense PTY lifecycle; see relay.rs for the testable core
async fn handle_socket(
    socket: WebSocket,
    conversation_id: String,
    terminals: ActiveTerminals,
    db: crate::db::Database,
    runtime: Arc<RuntimeManager>,
) {
    let cwd = match db.get_conversation(&conversation_id).await {
        Ok(conv) => std::path::PathBuf::from(&conv.cwd),
        Err(e) => {
            tracing::warn!(conv_id = %conversation_id, error = %e, "Terminal: conversation not found");
            return;
        }
    };

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Wait for the initial resize frame from xterm.js FitAddon (REQ-TERM-005).
    // We need the dims before either spawning a new PTY (fresh path) or
    // resizing an existing one (reclaim path).
    let Some(initial_dims) = wait_for_resize(&mut ws_receiver).await else {
        tracing::warn!(conv_id = %conversation_id, "Terminal: no initial resize frame");
        return;
    };

    // Acquire a handle: reclaim an existing session, or spawn a new one.
    let Some(arc_handle) = acquire_handle(
        &conversation_id,
        &terminals,
        &cwd,
        initial_dims,
        &mut ws_sender,
    )
    .await
    else {
        return;
    };

    let child_pid = arc_handle.child_pid;
    tracing::info!(conv_id = %conversation_id, pid = %child_pid, "Terminal session attached");

    let master_fd_raw = arc_handle.master_fd.as_raw_fd();

    // Apply the new client's dims to the existing PTY (reclaim path) or confirm
    // the freshly-spawned PTY matches them (fresh path — spawn already used these).
    // Either way, the reclaim case needs this so the new browser tab's viewport
    // matches the PTY's winsize.
    if let Err(e) = set_winsize_raw(master_fd_raw, initial_dims) {
        tracing::warn!(conv_id = %conversation_id, error = %e, "Terminal: initial TIOCSWINSZ failed");
    }

    // Wrap master_fd in PtyMasterIo (AsyncRead + AsyncWrite).
    // SAFETY: we take an owning copy of the raw fd here via `dup()`.  arc_handle
    // keeps the original OwnedFd alive; PtyMasterIo owns its own dup for the
    // relay lifetime, so a fd drop here doesn't affect the master (and lets
    // successive reclaimers each build a fresh PtyMasterIo without racing).
    let pty_io = match build_pty_io(master_fd_raw) {
        Ok(io) => io,
        Err(reason) => {
            tracing::error!(conv_id = %conversation_id, error = %reason, "Terminal: fd setup failed");
            // Full teardown — we failed before the relay started.
            full_teardown(&terminals, &conversation_id, arc_handle, child_pid).await;
            return;
        }
    };

    // Reset the stop channel to `Running` before the relay starts. A
    // reclaimer may have just transitioned it Running → Detach to evict the
    // previous relay; without this reset the new relay could observe the
    // stale `Detach` and exit immediately.
    let _ = arc_handle.stop_tx.send(StopReason::Running);
    let stop_rx = arc_handle.stop_tx.subscribe();

    // REQ-TERM-012: tear down terminal when conversation reaches a terminal state.
    let teardown_stop = arc_handle.stop_tx.clone();
    let teardown_conv_id = conversation_id.clone();
    if let Ok(mut bcast_rx) = runtime.subscribe(&conversation_id).await {
        tokio::spawn(async move {
            loop {
                match bcast_rx.recv().await {
                    Ok(SseEvent::ConversationBecameTerminal) => {
                        tracing::debug!(conv_id = %teardown_conv_id, "Terminal: conversation ended, tearing down PTY");
                        let _ = teardown_stop.send(StopReason::TearDown);
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
    }

    // Adapt WS sender: Sink<Message> → Sink<Vec<u8>> by wrapping in Message::Binary.
    // Pin the With adaptor so it implements Unpin.
    let ws_out =
        Box::pin(ws_sender.with(|v: Vec<u8>| async { Ok::<_, axum::Error>(Message::Binary(v)) }));

    // Adapt WS receiver: filter to binary frames only, unwrap payloads.
    // Non-binary frames (Close, Text, Ping/Pong, errors) terminate the stream.
    // Box to give the combined stream a concrete Unpin type.
    let ws_in = ws_receiver
        .take_while(|msg| std::future::ready(!matches!(msg, Ok(Message::Close(_)) | Err(_))))
        .filter_map(|msg| async {
            match msg {
                Ok(Message::Binary(data)) => Some(data),
                _ => None,
            }
        })
        .boxed();

    // The resize callback: calls TIOCSWINSZ on the real PTY master fd.
    let tracker = Arc::clone(&arc_handle.tracker);
    let on_resize = move |dims: Dims| {
        set_winsize_raw(master_fd_raw, dims)
            .unwrap_or_else(|e| tracing::warn!(error = %e, "Terminal: TIOCSWINSZ failed"));
    };

    let exit = run_relay(
        pty_io,
        ws_out,
        ws_in,
        RelayConfig {
            tracker,
            on_resize,
            stop_rx,
            conv_id: conversation_id.clone(),
        },
    )
    .await;

    tracing::debug!(conv_id = %conversation_id, ?exit, "Terminal relay exited");

    // Branch cleanup on the exit reason:
    //   PtyEof                  → shell is already gone, full teardown (reap + remove).
    //   Stopped(TearDown)       → REQ-TERM-012 fired, full teardown.
    //   Stopped(Detach)         → reclaimer wants the session, detach only.
    //   Stopped(Running)        → unreachable: the writer only returns Stopped for
    //                             non-Running reasons (see relay.rs). Treat as detach
    //                             to err on the side of keeping the shell alive.
    //   WsClosed                → browser disconnected, detach only so a reclaim
    //                             can recover the session.
    match exit {
        RelayExit::PtyEof | RelayExit::Stopped(StopReason::TearDown) => {
            full_teardown(&terminals, &conversation_id, arc_handle, child_pid).await;
        }
        RelayExit::Stopped(StopReason::Detach | StopReason::Running) | RelayExit::WsClosed => {
            detach_only(&conversation_id, arc_handle);
        }
    }
}

/// Acquire an `Arc<TerminalHandle>` for the relay: reclaim an existing session
/// or spawn a new one.
///
/// Reclaim path (session already registered):
///   1. Clone the existing Arc (drops the registry mutex immediately).
///   2. Signal the sitting relay to detach via `stop_tx.send(Detach)`.
///   3. Await `detached.notified()` — the old relay fires this on clean exit.
///   4. Return the same Arc for the new relay to use.
///
/// Fresh path (no session):
///   1. Spawn a new PTY via `spawn_blocking`.
///   2. `try_insert` into the registry. If that loses a race (another
///      connection inserted first), recurse into reclaim with the winner.
///
/// The `Notify`'s one-permit semantics cover both orderings: if the old
/// relay exits and calls `notify_one` before we call `notified()`, the
/// permit is stored; our later `notified()` consumes it and returns
/// immediately. This is the key property that makes concurrent reclaim
/// safe: exactly one notify ↔ one wakeup, whichever order they arrive.
async fn acquire_handle(
    conversation_id: &str,
    terminals: &ActiveTerminals,
    cwd: &std::path::Path,
    initial_dims: Dims,
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Option<Arc<TerminalHandle>> {
    // Fast path: reclaim if a handle already exists.
    if let Some(existing) = terminals.get(conversation_id) {
        return reclaim(conversation_id, existing).await;
    }

    // Slow path: spawn a new PTY.
    let cwd_owned = cwd.to_path_buf();
    let handle = match tokio::task::spawn_blocking(move || spawn_pty(&cwd_owned, initial_dims))
        .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: PTY spawn failed");
            return None;
        }
        Err(e) => {
            tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: spawn_blocking panicked");
            return None;
        }
    };

    let child_pid = handle.child_pid;

    // Atomic check-and-insert. If we lose the race, the handle we just spawned
    // is dropped (closing master_fd → SIGHUP), and we fall back to reclaiming
    // the winner so the caller still gets an attached session.
    if let Some(arc_handle) = terminals.try_insert(conversation_id.to_string(), handle) {
        return Some(arc_handle);
    }

    tracing::warn!(conv_id = %conversation_id, "Terminal: post-spawn race lost, reclaiming winner");
    // Reap the child from the losing spawn to avoid a zombie. The handle was
    // consumed by try_insert (on loss it was dropped), so master_fd is closed
    // and the child will receive SIGHUP.
    let _ = tokio::task::spawn_blocking(move || {
        let _ = waitpid(child_pid, None);
    })
    .await;

    let Some(existing) = terminals.get(conversation_id) else {
        // Winner removed itself between try_insert and get. Report back to
        // the client and bail — retrying again could loop on edge-case timing.
        let _ = ws_sender
            .send(Message::Text("error: terminal unavailable".to_string()))
            .await;
        return None;
    };
    reclaim(conversation_id, existing).await
}

/// Evict the relay currently attached to `existing` and return the handle for
/// the caller to reuse. The handle's `tracker`, `master_fd`, etc. are preserved.
async fn reclaim(
    conversation_id: &str,
    existing: Arc<TerminalHandle>,
) -> Option<Arc<TerminalHandle>> {
    tracing::info!(conv_id = %conversation_id, "Terminal: reclaiming existing session");

    let detached = Arc::clone(&existing.detached);
    // Signal the sitting relay to detach. `send` returns Err only if there
    // are no receivers; that means no relay is actually attached — another
    // reclaimer has already evicted it, or the relay hasn't subscribed yet.
    // Either way, proceeding is safe: if no relay is attached, there's
    // nothing to wait for.
    let send_result = existing.stop_tx.send(StopReason::Detach);

    if send_result.is_ok() {
        // Await the old relay's clean exit. If a concurrent reclaimer races
        // with us, `Notify`'s permit semantics ensure both wakers observe a
        // valid notification (the old relay fires once; the second reclaimer
        // either gets the stored permit from a prior cycle, or waits for the
        // in-flight cycle's notify — still one-to-one).
        //
        // Bound the wait so a pathologically stuck relay (e.g. blocked in a
        // poll_ready we can't cancel from outside) doesn't deadlock the
        // reclaimer forever.
        if tokio::time::timeout(Duration::from_secs(5), detached.notified())
            .await
            .is_err()
        {
            tracing::warn!(conv_id = %conversation_id,
                "Terminal: reclaim timed out waiting for old relay to detach; proceeding anyway");
        }
    } else {
        tracing::debug!(conv_id = %conversation_id,
            "Terminal: reclaim found no attached relay (stop_tx has no receivers)");
    }

    Some(existing)
}

/// Full teardown: shell must die, registry entry goes away, child reaped.
async fn full_teardown(
    terminals: &ActiveTerminals,
    conversation_id: &str,
    arc_handle: Arc<TerminalHandle>,
    child_pid: nix::unistd::Pid,
) {
    terminals.remove(conversation_id);
    // When this and any other Arc clones drop, master_fd closes → SIGHUP → shell exits.
    drop(arc_handle);
    let _ = tokio::task::spawn_blocking(move || {
        let _ = waitpid(child_pid, None);
    })
    .await;
    tracing::info!(conv_id = %conversation_id, "Terminal session ended");
}

/// Detach only: release our Arc clone but leave the registry entry intact so
/// the shell stays alive awaiting a future reclaim. Signal the detach so a
/// waiting reclaimer can proceed.
fn detach_only(conversation_id: &str, arc_handle: Arc<TerminalHandle>) {
    arc_handle.detached.notify_one();
    // Dropping our clone is safe: the registry holds its own Arc, so the
    // refcount stays > 0 and master_fd remains open.
    drop(arc_handle);
    tracing::info!(conv_id = %conversation_id, "Terminal WS detached; shell preserved for reclaim");
}

/// Build a `PtyMasterIo` backed by a fresh `dup()` of `master_fd_raw`.
/// Each relay holds its own dup so dropping the relay's fd doesn't close
/// the registry's master fd.
fn build_pty_io(master_fd_raw: std::os::unix::io::RawFd) -> Result<PtyMasterIo, String> {
    set_nonblocking(master_fd_raw).map_err(|e| format!("set_nonblocking: {e}"))?;
    let pty_fd = nix::unistd::dup(master_fd_raw)
        .map(|raw| unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw) })
        .map_err(|e| format!("dup: {e}"))?;
    PtyMasterIo::new(pty_fd).map_err(|e| format!("AsyncFd: {e}"))
}

/// Receive WS frames until a resize (0x01) frame arrives. Times out after 10 s.
async fn wait_for_resize(ws: &mut futures::stream::SplitStream<WebSocket>) -> Option<Dims> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            if let Ok(Message::Binary(data)) = msg {
                if data.len() >= 5 && data[0] == 0x01 {
                    let cols = u16::from_be_bytes([data[1], data[2]]);
                    let rows = u16::from_be_bytes([data[3], data[4]]);
                    // Reject zero-dimension initial resize (ResizeFrameRejected rule).
                    if let Some(dims) = Dims::try_new(cols, rows) {
                        return Some(dims);
                    }
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

// ─── Reclaim integration tests ───────────────────────────────────────────────
//
// Task 24691 acceptance: a second connection for an already-active session
// must reclaim it (not 409), and two concurrent reclaimers must not deadlock
// or double-teardown. These tests drive the reclaim primitives directly —
// `StopReason`, `stop_tx`, `detached`, the `tracker` on `TerminalHandle` —
// without the axum/WebSocket layer, which requires too much scaffolding.
// The `ws.rs` handler composes these primitives; the composition is
// exercised manually in the acceptance smoke test.

#[cfg(test)]
mod reclaim_tests {
    #![allow(clippy::unwrap_used)]
    use super::super::relay::{run_relay, RelayConfig, RelayExit};
    use super::super::session::{ShellIntegrationStatus, StopReason, TerminalHandle};
    use crate::terminal::command_tracker::CommandTracker;
    use crate::terminal::test_helpers::full_command;
    use futures::channel::mpsc;
    use futures::StreamExt;
    use std::sync::{Arc, Mutex};
    use tokio::io::{duplex, AsyncWriteExt};
    use tokio::sync::{watch, Notify};

    /// Build a `TerminalHandle`-shaped value suitable for reclaim tests.
    /// We reuse the real struct but back `master_fd` with `/dev/null` since
    /// these tests drive the relay via `DuplexStream` rather than a real PTY.
    fn build_handle() -> Arc<TerminalHandle> {
        use std::fs::OpenOptions;
        use std::os::unix::io::{FromRawFd, IntoRawFd};

        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap();
        let raw = f.into_raw_fd();
        // SAFETY: we own the fd, transferring to OwnedFd.
        let owned_fd = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw) };

        let (stop_tx, _stop_rx) = watch::channel(StopReason::Running);

        Arc::new(TerminalHandle {
            master_fd: owned_fd,
            child_pid: nix::unistd::Pid::from_raw(1),
            tracker: Arc::new(Mutex::new(CommandTracker::new("reclaim-test".to_string()))),
            shell_integration_status: Arc::new(Mutex::new(ShellIntegrationStatus::Unknown)),
            stop_tx,
            detached: Arc::new(Notify::new()),
        })
    }

    /// Emulate the handler's relay step for a single connection.
    /// Returns the `RelayExit` so the test can assert on detach-vs-teardown.
    async fn run_connection(
        handle: Arc<TerminalHandle>,
        shell_writes: Vec<u8>,
    ) -> (RelayExit, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
        let (mut shell_end, pty_end) = duplex(4096);
        let (ws_tx, mut ws_rx) = mpsc::channel::<Vec<u8>>(32);

        // Reset the channel to Running (handler guarantee) before subscribing.
        let _ = handle.stop_tx.send(StopReason::Running);
        let stop_rx = handle.stop_tx.subscribe();

        let ws_in = futures::stream::pending::<Vec<u8>>().boxed();

        let (frames_tx, frames_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        // Drain ws frames into the unbounded channel the test asserts against.
        let drain = tokio::spawn(async move {
            while let Some(frame) = ws_rx.next().await {
                let _ = frames_tx.send(frame);
            }
        });

        let tracker = Arc::clone(&handle.tracker);
        let detached = Arc::clone(&handle.detached);
        let handle_clone = Arc::clone(&handle);
        let relay = tokio::spawn(async move {
            let exit = run_relay(
                pty_end,
                ws_tx,
                ws_in,
                RelayConfig {
                    tracker,
                    on_resize: |_| {},
                    stop_rx,
                    conv_id: "reclaim".to_string(),
                },
            )
            .await;

            // Mirror `ws.rs` behaviour for the detach path: notify waiters
            // before dropping the handle clone.
            if matches!(
                exit,
                RelayExit::Stopped(StopReason::Detach | StopReason::Running) | RelayExit::WsClosed
            ) {
                detached.notify_one();
            }
            drop(handle_clone);
            exit
        });

        if !shell_writes.is_empty() {
            shell_end.write_all(&shell_writes).await.unwrap();
        }
        // Let the reader ingest.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let exit = relay.await.unwrap();
        drop(drain);
        drop(shell_end);
        (exit, frames_rx)
    }

    /// Acceptance: two sequential connections for the same conv_id — the
    /// second reclaims. Both commands end up in the same tracker.
    #[tokio::test]
    async fn two_sequential_connections_reclaim_same_handle() {
        let handle = build_handle();

        // Connection 1: capture "cmd1", then reclaimer signals Detach.
        let h1 = Arc::clone(&handle);
        let writes_1 = full_command("cmd1", "out1\n", Some(0));
        let conn_1 = tokio::spawn(async move { run_connection(h1, writes_1).await });

        // Simulate the "new connection's" reclaim signal.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        handle.stop_tx.send(StopReason::Detach).unwrap();

        // Reclaimer waits for detach notification, then subscribes for its own relay.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle.detached.notified(),
        )
        .await
        .expect("detach notification must fire within the bound");

        let (exit_1, _) = conn_1.await.unwrap();
        assert_eq!(
            exit_1,
            RelayExit::Stopped(StopReason::Detach),
            "first connection must exit via Detach"
        );

        // Connection 2: runs on the same handle, captures "cmd2".
        let h2 = Arc::clone(&handle);
        let writes_2 = full_command("cmd2", "out2\n", Some(0));
        let conn_2 = tokio::spawn(async move { run_connection(h2, writes_2).await });

        // Let relay 2 finish on its own (WsClosed via shell_end drop at end of run_connection).
        // We need to give it a chance to stop — signal Detach so it exits cleanly.
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        handle.stop_tx.send(StopReason::Detach).unwrap();

        let (exit_2, _) = conn_2.await.unwrap();
        assert!(
            matches!(
                exit_2,
                RelayExit::Stopped(StopReason::Detach) | RelayExit::PtyEof
            ),
            "second connection must exit cleanly; got {exit_2:?}"
        );

        // Both commands in the same tracker — reclaim preserved state.
        let tracker = handle.tracker.lock().unwrap();
        let texts: Vec<String> = tracker
            .recent_commands(10)
            .iter()
            .map(|r| r.command_text.clone())
            .collect();
        assert!(
            texts.contains(&"cmd1".to_string()) && texts.contains(&"cmd2".to_string()),
            "tracker must contain commands from both connections, got {texts:?}"
        );
    }

    /// The `tracker` Arc survives reclaim — a third Arc holder outside the
    /// relay sees both commands.
    #[tokio::test]
    async fn tracker_arc_survives_reclaim() {
        let handle = build_handle();
        let external_tracker = Arc::clone(&handle.tracker);

        // Run + detach relay 1.
        let h1 = Arc::clone(&handle);
        let writes_1 = full_command("a", "1\n", Some(0));
        let conn_1 = tokio::spawn(async move { run_connection(h1, writes_1).await });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        handle.stop_tx.send(StopReason::Detach).unwrap();
        let (_, _) = conn_1.await.unwrap();

        // Tracker already has "a".
        assert_eq!(
            external_tracker
                .lock()
                .unwrap()
                .last_command()
                .map(|r| r.command_text.clone()),
            Some("a".to_string()),
        );

        // Run + detach relay 2.
        let h2 = Arc::clone(&handle);
        let writes_2 = full_command("b", "2\n", Some(0));
        let conn_2 = tokio::spawn(async move { run_connection(h2, writes_2).await });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        handle.stop_tx.send(StopReason::Detach).unwrap();
        let (_, _) = conn_2.await.unwrap();

        // Same external tracker now has "b" (reclaim preserved the Arc).
        let texts: Vec<String> = external_tracker
            .lock()
            .unwrap()
            .recent_commands(10)
            .iter()
            .map(|r| r.command_text.clone())
            .collect();
        assert!(
            texts.contains(&"a".to_string()) && texts.contains(&"b".to_string()),
            "external tracker Arc must see all commands; got {texts:?}"
        );
    }

    /// Concurrent reclaim: two reclaimers race to evict the sitting relay.
    /// Exactly one signal gets the relay to exit; both reclaimers observe
    /// `detached` and proceed. No deadlock, no panic.
    #[tokio::test]
    async fn concurrent_reclaim_two_racers_no_deadlock() {
        let handle = build_handle();

        // Sitting relay.
        let h_sit = Arc::clone(&handle);
        let sitting = tokio::spawn(async move { run_connection(h_sit, Vec::new()).await });

        // Give the sitting relay time to subscribe to stop_tx.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        // Two reclaimers run in parallel. Each does:
        //   1. send(Detach) — both succeed (watch::send never fails as long
        //      as there's a receiver; a redundant Detach is a no-op on the
        //      already-Detach channel but triggers `changed()` for any new
        //      subscriber).
        //   2. await detached.notified()
        //
        // The sitting relay fires `notify_one` exactly once on exit. The
        // Notify's permit semantics allow the second racer to observe a
        // stored permit from any later detach cycle — but since no further
        // relay runs, the second racer would block forever in a naive
        // implementation. The bounded timeout in `reclaim()` (in ws.rs) is
        // what prevents that in production; here we cap the await.
        let reclaimer = |label: &'static str| {
            let h = Arc::clone(&handle);
            tokio::spawn(async move {
                h.stop_tx.send(StopReason::Detach).unwrap();
                let got = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    h.detached.notified(),
                )
                .await;
                (label, got.is_ok())
            })
        };

        let r1 = reclaimer("r1");
        let r2 = reclaimer("r2");

        let (exit_sit, _) = sitting.await.unwrap();
        assert_eq!(
            exit_sit,
            RelayExit::Stopped(StopReason::Detach),
            "sitting relay must exit via Detach exactly once"
        );

        let (l1, ok1) = r1.await.unwrap();
        let (l2, ok2) = r2.await.unwrap();

        // At least one reclaimer observed the notify. The other may have
        // raced ahead of the notify and timed out — that's acceptable: the
        // runtime's bounded timeout surfaces the missed notify, and the
        // reclaimer proceeds anyway (see `reclaim()` in ws.rs). What we
        // must NOT see is a deadlock — both spawns must return.
        assert!(
            ok1 || ok2,
            "at least one reclaimer must observe the detach notify ({l1}={ok1}, {l2}={ok2})"
        );
    }
}
