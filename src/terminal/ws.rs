//! WebSocket endpoint handler for the terminal (REQ-TERM-004 through REQ-TERM-009).
//!
//! Binary frame protocol:
//!   byte 0 = 0x00 → PTY data (bidirectional)
//!   byte 0 = 0x01 → resize: u16be cols, u16be rows (client → server only)
//!
//! The relay logic (byte forwarding, parser feeding, quiescence detection) lives
//! in `relay.rs` as `run_relay`.  This module handles only the axum/WebSocket
//! wiring: auth, 409 guard, PTY spawn, frame type filtering, and process lifecycle.

use super::relay::{run_relay, PtyMasterIo, RelayConfig};
use super::session::{ActiveTerminals, Dims};
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

    // 409 guard: atomic check before spawning PTY (REQ-TERM-003).
    // Check first to avoid wasting a fork+exec on duplicate connections.
    if terminals.is_active(&conversation_id) {
        tracing::warn!(conv_id = %conversation_id, "Terminal: 409 — session already active");
        let _ = ws_sender
            .send(Message::Text("error: terminal already active".to_string()))
            .await;
        return;
    }

    // Wait for the initial resize frame from xterm.js FitAddon (REQ-TERM-005).
    let Some(initial_dims) = wait_for_resize(&mut ws_receiver).await else {
        tracing::warn!(conv_id = %conversation_id, "Terminal: no initial resize frame");
        return;
    };

    let cwd_clone = cwd.clone();
    let handle = match tokio::task::spawn_blocking(move || spawn_pty(&cwd_clone, initial_dims))
        .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: PTY spawn failed");
            return;
        }
        Err(e) => {
            tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: spawn_blocking panicked");
            return;
        }
    };

    // Final atomic check-and-insert: guards against the race where two connections
    // pass the pre-spawn check concurrently, then both try to register.
    let Some(arc_handle) = terminals.try_insert(conversation_id.clone(), handle) else {
        tracing::warn!(conv_id = %conversation_id, "Terminal: 409 — session already active (post-spawn race)");
        let _ = ws_sender
            .send(Message::Text("error: terminal already active".to_string()))
            .await;
        return;
    };

    tracing::info!(conv_id = %conversation_id, pid = %arc_handle.child_pid, "Terminal session started");

    let master_fd_raw = arc_handle.master_fd.as_raw_fd();

    // Set non-blocking so PtyMasterIo (AsyncFd) works correctly.
    if let Err(e) = set_nonblocking(master_fd_raw) {
        tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: set_nonblocking failed");
        return;
    }

    // Wrap master_fd in PtyMasterIo (AsyncRead + AsyncWrite).
    // SAFETY: we take an owning copy of the raw fd here.  arc_handle keeps the
    // OwnedFd alive; PtyMasterIo borrows it by raw fd number for the relay lifetime.
    //
    // We cannot transfer ownership into PtyMasterIo because arc_handle.master_fd
    // needs to be dropped AFTER run_relay returns (to trigger SIGHUP via Drop).
    // Instead we duplicate the fd so PtyMasterIo owns its own file description.
    let pty_fd = match nix::unistd::dup(master_fd_raw) {
        Ok(raw) => unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw) },
        Err(e) => {
            tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: dup(master_fd) failed");
            return;
        }
    };
    let pty_io = match PtyMasterIo::new(pty_fd) {
        Ok(io) => io,
        Err(e) => {
            tracing::error!(conv_id = %conversation_id, error = %e, "Terminal: AsyncFd::new failed");
            return;
        }
    };

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    // REQ-TERM-012: tear down terminal when conversation reaches a terminal state.
    let teardown_terminals = terminals.clone();
    let teardown_conv_id = conversation_id.clone();
    let teardown_stop = stop_tx.clone();
    if let Ok(mut bcast_rx) = runtime.subscribe(&conversation_id).await {
        tokio::spawn(async move {
            loop {
                match bcast_rx.recv().await {
                    Ok(SseEvent::ConversationBecameTerminal) => {
                        tracing::debug!(conv_id = %teardown_conv_id, "Terminal: conversation ended, tearing down PTY");
                        teardown_terminals.remove(&teardown_conv_id);
                        let _ = teardown_stop.send(true);
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
    // Note: relay.rs also calls parser.set_size(), maintaining ParserDimensionSync.
    let quiescence_tx = arc_handle.quiescence_tx.clone();
    let parser = Arc::clone(&arc_handle.parser);
    let on_resize = move |dims: Dims| {
        set_winsize_raw(master_fd_raw, dims)
            .unwrap_or_else(|e| tracing::warn!(error = %e, "Terminal: TIOCSWINSZ failed"));
    };

    let exit = run_relay(
        pty_io,
        ws_out,
        ws_in,
        RelayConfig {
            parser,
            quiescence_tx,
            on_resize,
            stop_rx,
            conv_id: conversation_id.clone(),
        },
    )
    .await;

    tracing::debug!(conv_id = %conversation_id, ?exit, "Terminal relay exited");

    let child_pid = arc_handle.child_pid;
    // Remove from registry then drop arc_handle → original master_fd closes →
    // SIGHUP → shell exits → EIO on the pty_io fd (already past run_relay).
    terminals.remove(&conversation_id);
    drop(arc_handle);
    // Reap child process. Errors (ECHILD if already reaped) are ignored.
    let _ = tokio::task::spawn_blocking(move || {
        let _ = waitpid(child_pid, None);
    })
    .await;
    tracing::info!(conv_id = %conversation_id, "Terminal session ended");
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
                    if cols > 0 && rows > 0 {
                        return Some(Dims { cols, rows });
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
