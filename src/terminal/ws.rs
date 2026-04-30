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
use tokio::sync::OwnedSemaphorePermit;

/// How long to wait for the sitting relay to release the `attach_permit`
/// before bailing out. The permit is the authoritative single-occupancy
/// mechanism; this timeout is a safety net against a pathologically stuck
/// relay (e.g. blocked in a `poll_ready` we can't cancel from outside).
/// If it fires, the reclaimer returns `None` rather than proceeding without
/// the permit — that would create concurrent attached relays, which is the
/// exact bug this permit exists to prevent.
const ATTACH_PERMIT_TIMEOUT: Duration = Duration::from_secs(5);

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

    // Acquire a handle AND the single attach_permit: reclaim an existing
    // session, or spawn a new one. The permit is the structural guarantee
    // that only one relay is attached to this handle at a time; we hold it
    // for the entire lifetime of this relay and drop it in the cleanup
    // branch (full_teardown / detach_only).
    let Some((arc_handle, attach_permit)) = acquire_handle(
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
                    Ok(SseEvent::ConversationBecameTerminal { .. }) => {
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
    // Release the single-attach permit AFTER cleanup runs. This wakes the
    // next reclaimer's `acquire_owned()` at the earliest point that the
    // slot is actually free. `drop` is explicit here so the ordering is
    // obvious at the call site; the code is correct without it, but the
    // lint-free discipline helps anyone tracing the release point.
    drop(attach_permit);
}

/// Acquire an `Arc<TerminalHandle>` AND the single `attach_permit` for the
/// relay: reclaim an existing session or spawn a new one.
///
/// The permit is the structural single-occupancy mechanism: whoever holds it
/// is the sole attached relay. Two concurrent reclaimers cannot both acquire
/// it — the semaphore serializes them, so the second reclaimer's
/// `acquire_owned()` blocks until the first relay releases the permit (on
/// detach or teardown).
///
/// Reclaim path (session already registered):
///   1. Clone the existing Arc (drops the registry mutex immediately).
///   2. Signal the sitting relay to detach via `stop_tx.send(Detach)`.
///   3. `acquire_owned()` on the permit — blocks until the sitting relay
///      drops its permit. Bounded by `ATTACH_PERMIT_TIMEOUT` as a safety net.
///   4. Return `(handle, permit)` to the caller.
///
/// Fresh path (no session):
///   1. Spawn a new PTY via `spawn_blocking`.
///   2. `try_insert` into the registry. If that loses a race (another
///      connection inserted first), reap the losing child and reclaim the
///      winner so the caller still gets an attached session.
///   3. `acquire_owned()` the permit — available immediately since the
///      handle is fresh and nobody holds it yet.
async fn acquire_handle(
    conversation_id: &str,
    terminals: &ActiveTerminals,
    cwd: &std::path::Path,
    initial_dims: Dims,
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Option<(Arc<TerminalHandle>, OwnedSemaphorePermit)> {
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
        // Fresh handle — permit is available immediately (initialized with 1).
        return acquire_permit(conversation_id, arc_handle).await;
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

/// Evict the relay currently attached to `existing` and take over the single
/// attach slot. The handle's `tracker`, `master_fd`, etc. are preserved.
///
/// Returns `None` if we fail to acquire the permit within
/// `ATTACH_PERMIT_TIMEOUT` — proceeding without it would break the
/// single-attached-relay invariant (the permit is the authoritative slot).
async fn reclaim(
    conversation_id: &str,
    existing: Arc<TerminalHandle>,
) -> Option<(Arc<TerminalHandle>, OwnedSemaphorePermit)> {
    tracing::info!(conv_id = %conversation_id, "Terminal: reclaiming existing session");

    // Signal the sitting relay to detach. `send` returns Err only if there
    // are no receivers; that means no relay is actually attached — the
    // permit should be available and `acquire_permit` will return
    // immediately. Either way we fall through to permit acquisition: the
    // permit is what authoritatively gates the attach slot.
    let _ = existing.stop_tx.send(StopReason::Detach);

    acquire_permit(conversation_id, existing).await
}

/// Acquire the `attach_permit` for `handle`, bounded by
/// `ATTACH_PERMIT_TIMEOUT`. Returns `None` on timeout — the caller MUST
/// NOT proceed to attach a relay without a permit, because that would
/// create concurrent attached relays reading from the same PTY master.
async fn acquire_permit(
    conversation_id: &str,
    handle: Arc<TerminalHandle>,
) -> Option<(Arc<TerminalHandle>, OwnedSemaphorePermit)> {
    let sem = Arc::clone(&handle.attach_permit);
    match tokio::time::timeout(ATTACH_PERMIT_TIMEOUT, sem.acquire_owned()).await {
        Ok(Ok(permit)) => Some((handle, permit)),
        Ok(Err(e)) => {
            // Semaphore closed — shouldn't happen (we never call `close()`).
            tracing::error!(conv_id = %conversation_id, error = %e,
                "Terminal: attach_permit semaphore closed unexpectedly");
            None
        }
        Err(_timeout) => {
            // Sitting relay is stuck. Bail rather than risk concurrent attach.
            tracing::warn!(conv_id = %conversation_id,
                "Terminal: timed out waiting for attach_permit; sitting relay appears stuck, aborting reclaim");
            None
        }
    }
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
/// the shell stays alive awaiting a future reclaim. The caller drops the
/// `attach_permit` after this returns — that permit release is what wakes
/// any pending reclaimer in `acquire_permit`.
fn detach_only(conversation_id: &str, arc_handle: Arc<TerminalHandle>) {
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
    use super::{acquire_permit, ATTACH_PERMIT_TIMEOUT};
    use crate::terminal::command_tracker::CommandTracker;
    use crate::terminal::test_helpers::full_command;
    use futures::channel::mpsc;
    use futures::StreamExt;
    use std::sync::{Arc, Mutex};
    use tokio::io::{duplex, AsyncWriteExt};
    use tokio::sync::{watch, Semaphore};

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
            attach_permit: Arc::new(Semaphore::new(1)),
        })
    }

    /// Emulate the handler's relay step for a single connection.
    ///
    /// Follows the same acquire → run → release sequence as
    /// `handle_socket`: acquire the `attach_permit`, run the relay, drop the
    /// permit on exit. This way concurrency tests exercise the full
    /// single-occupancy path, not just the signal level.
    async fn run_connection(
        handle: Arc<TerminalHandle>,
        shell_writes: Vec<u8>,
    ) -> (RelayExit, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
        // Acquire the permit via the same path the handler uses. This blocks
        // until any previous relay releases its permit, matching production
        // behaviour exactly.
        let (handle, permit) = acquire_permit("reclaim", handle)
            .await
            .expect("test acquire must succeed");

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

            drop(handle_clone);
            // Release the permit AFTER the relay has exited (mirrors the
            // `drop(attach_permit)` at the end of `handle_socket`). This is
            // the authoritative signal that the attach slot is free.
            drop(permit);
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

    /// Acceptance: two sequential connections for the same `conv_id` — the
    /// second reclaims. Both commands end up in the same tracker.
    #[tokio::test]
    async fn two_sequential_connections_reclaim_same_handle() {
        let handle = build_handle();

        // Connection 1: capture "cmd1", then signal Detach to evict.
        let h1 = Arc::clone(&handle);
        let writes_1 = full_command("cmd1", "out1\n", Some(0));
        let conn_1 = tokio::spawn(async move { run_connection(h1, writes_1).await });

        // Give conn_1 time to acquire the permit and start its relay.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        handle.stop_tx.send(StopReason::Detach).unwrap();

        let (exit_1, _) = conn_1.await.unwrap();
        assert_eq!(
            exit_1,
            RelayExit::Stopped(StopReason::Detach),
            "first connection must exit via Detach"
        );

        // Connection 2: runs on the same handle, captures "cmd2". Its
        // `acquire_permit` succeeds immediately because conn_1 released the
        // permit on exit.
        let h2 = Arc::clone(&handle);
        let writes_2 = full_command("cmd2", "out2\n", Some(0));
        let conn_2 = tokio::spawn(async move { run_connection(h2, writes_2).await });

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

    /// Concurrent reclaim — exactly-one-winner at the permit level.
    ///
    /// Two reclaimers race to acquire the `attach_permit` while a sitting
    /// relay holds it. The semaphore's single-permit semantics guarantee
    /// that at most ONE racer holds the permit at any moment; the other
    /// blocks in `acquire_owned()` until the first releases.
    ///
    /// This is the structural property that prevents two relays from
    /// concurrently reading the same PTY master — which is the actual bug
    /// the permit exists to prevent. The prior signal-level test
    /// (`detached.notified()`) could not catch that bug, because the bug
    /// manifests only after both racers reach the relay step.
    #[tokio::test]
    async fn concurrent_reclaim_exactly_one_winner() {
        let handle = build_handle();

        // Sitting relay holds the one permit. We'll evict it after both
        // reclaimers have started their acquire.
        let h_sit = Arc::clone(&handle);
        let sitting = tokio::spawn(async move { run_connection(h_sit, Vec::new()).await });

        // Give the sitting relay time to acquire the permit and subscribe
        // to stop_tx.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(
            handle.attach_permit.available_permits(),
            0,
            "sitting relay must hold the one permit"
        );

        // Shared counter: how many racers are currently holding the permit.
        // Must never exceed 1.
        let inflight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        // Each racer goes through the real `acquire_permit` path, holds the
        // permit briefly (simulating a relay lifetime), then releases. If
        // the semaphore were broken, both racers could be inside the
        // "holding" region simultaneously — we trip the max_seen > 1 check.
        let racer = |label: &'static str| {
            let h = Arc::clone(&handle);
            let inflight = Arc::clone(&inflight);
            let max_seen = Arc::clone(&max_seen);
            let order = Arc::clone(&order);
            tokio::spawn(async move {
                // Drive a detach on the sitting relay; idempotent if already
                // driven by a sibling racer.
                let _ = h.stop_tx.send(StopReason::Detach);
                let got = acquire_permit("race", Arc::clone(&h)).await;
                let (handle_back, permit) = got.expect("permit acquisition must succeed");

                // Record entry order and check single-occupancy.
                order.lock().unwrap().push(label);
                let now = inflight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max_seen.fetch_max(now, std::sync::atomic::Ordering::SeqCst);

                // Simulate a relay lifetime. If another racer is holding the
                // permit concurrently, `max_seen` will observe > 1.
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;

                inflight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                drop(permit);
                drop(handle_back);
                label
            })
        };

        let r1 = racer("r1");
        let r2 = racer("r2");

        let (exit_sit, _) = sitting.await.unwrap();
        assert_eq!(
            exit_sit,
            RelayExit::Stopped(StopReason::Detach),
            "sitting relay must exit via Detach"
        );

        let l1 = r1.await.unwrap();
        let l2 = r2.await.unwrap();

        // The exactly-one-winner invariant: at no point were two racers
        // holding the permit simultaneously.
        let peak = max_seen.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            peak, 1,
            "attach_permit must enforce single-occupancy; saw {peak} concurrent holders"
        );

        // Both racers eventually got the permit — first one immediately
        // after the sitting relay released, second one after the first
        // released. No deadlock, no double-teardown.
        let seen = order.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "both racers must eventually acquire");
        assert!(
            seen.contains(&l1) && seen.contains(&l2),
            "both labels present; got {seen:?}"
        );

        // Permit is released back to the semaphore when both racers drop.
        assert_eq!(
            handle.attach_permit.available_permits(),
            1,
            "permit must be released after all racers finish"
        );
    }

    /// Reclaim timeout bail-out: if the sitting permit holder never
    /// releases, a reclaimer must NOT proceed without the permit. Returning
    /// `None` is the correct failure mode — proceeding anyway would create
    /// concurrent relays reading from the same PTY master.
    #[tokio::test]
    async fn reclaim_times_out_rather_than_bypass_permit() {
        let handle = build_handle();

        // Take the permit permanently. Mirrors a pathologically stuck relay.
        let stuck = Arc::clone(&handle.attach_permit)
            .acquire_owned()
            .await
            .expect("initial acquire");

        // Reclaimer should time out (not proceed without the permit).
        let start = std::time::Instant::now();
        let result = acquire_permit("stuck-test", Arc::clone(&handle)).await;
        let elapsed = start.elapsed();

        assert!(
            result.is_none(),
            "reclaimer must return None on permit timeout, not bypass and proceed"
        );
        // Bounded by ATTACH_PERMIT_TIMEOUT (with slack for CI scheduling).
        assert!(
            elapsed >= ATTACH_PERMIT_TIMEOUT,
            "must actually wait the full timeout before bailing; got {elapsed:?}"
        );

        drop(stuck);

        // After the stuck holder releases, a new reclaimer succeeds.
        let after = acquire_permit("recovery", Arc::clone(&handle)).await;
        assert!(
            after.is_some(),
            "after stuck holder releases, reclaim must succeed again"
        );
    }

    /// Teardown path must release the permit so the session isn't stuck.
    /// Simulates `full_teardown` (shell-exit) path: the permit is dropped
    /// by the handler after cleanup; the semaphore returns to 1 permit.
    #[tokio::test]
    async fn teardown_releases_permit() {
        let handle = build_handle();
        let sem = Arc::clone(&handle.attach_permit);

        // Attacher runs a connection to completion; `run_connection` releases
        // the permit on exit regardless of which teardown branch the handler
        // would take.
        let h = Arc::clone(&handle);
        let conn = tokio::spawn(async move { run_connection(h, Vec::new()).await });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        // TearDown path — models conversation-terminal cleanup (REQ-TERM-012).
        handle.stop_tx.send(StopReason::TearDown).unwrap();
        let (exit, _) = conn.await.unwrap();
        assert_eq!(exit, RelayExit::Stopped(StopReason::TearDown));

        // Permit returned to the semaphore → a future reclaimer can acquire.
        assert_eq!(
            sem.available_permits(),
            1,
            "permit must be released after teardown so the session isn't stuck"
        );
    }
}
