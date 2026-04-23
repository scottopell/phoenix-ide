//! Core relay loop — mediates between PTY I/O and WebSocket frames.
//!
//! ## Design
//!
//! `run_relay` is the testable heart of the terminal. It takes generic I/O endpoints:
//!
//! ```text
//! pty (AsyncRead + AsyncWrite)  ←→  run_relay  ←→  ws_incoming / ws_outgoing
//!                                        │
//!                                   CommandTracker
//! ```
//!
//! Production: `pty = PtyMasterIo` (wraps the real PTY master fd via `AsyncFd`).
//! Tests:      `pty = tokio::io::DuplexStream` (in-memory pipe, zero infrastructure).

use std::{
    io,
    os::unix::io::OwnedFd,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use super::command_tracker::CommandTracker;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use super::session::{Dims, StopReason};

/// How the relay loop exited.
///
/// `Stopped` carries the `StopReason` the writer observed so the handler
/// can branch on detach-vs-teardown without a follow-up `stop_rx.borrow()`
/// (which would race with a concurrent reclaimer resetting the channel).
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RelayExit {
    /// PTY EOF: `read()` returned 0 bytes or EIO (shell exited). REQ-TERM-007.
    PtyEof,
    /// WebSocket stream closed by client or disconnected.
    WsClosed,
    /// Stopped by the external stop channel; payload is the reason observed.
    Stopped(StopReason),
}

// ── PtyMasterIo ──────────────────────────────────────────────────────────────

/// Wraps an `OwnedFd` (PTY master) as `AsyncRead + AsyncWrite`.
///
/// Uses `tokio::io::unix::AsyncFd` for epoll-backed readiness notification
/// plus `libc::read/write` for the actual I/O — the same mechanism as the
/// previous `reader_task` implementation.  EIO is mapped to EOF (0-byte read)
/// so the relay loop sees a clean stream termination when the shell exits.
pub struct PtyMasterIo(pub AsyncFd<OwnedFd>);

impl PtyMasterIo {
    /// Wrap an `OwnedFd` (must already be set to `O_NONBLOCK`).
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        Ok(Self(AsyncFd::new(fd)?))
    }
}

impl AsyncRead for PtyMasterIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = match self.0.poll_read_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let result = guard.try_io(|inner| {
                use std::os::unix::io::AsRawFd;
                let slice = buf.initialize_unfilled();
                // SAFETY: fd is non-blocking; reading into a valid slice.
                let n = unsafe {
                    libc::read(inner.as_raw_fd(), slice.as_mut_ptr().cast(), slice.len())
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    #[allow(clippy::cast_sign_loss)] // n >= 0 guaranteed
                    Ok(n as usize)
                }
            });

            match result {
                Ok(Ok(n)) => {
                    buf.advance(n); // n is already usize from try_io callback
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => {
                    // EIO = shell exited → map to EOF (0-byte read).
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_would_block) => {} // spurious wakeup, re-poll
            }
        }
    }
}

impl AsyncWrite for PtyMasterIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = match self.0.poll_write_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let result = guard.try_io(|inner| {
                use std::os::unix::io::AsRawFd;
                // SAFETY: fd is non-blocking; writing from a valid slice.
                let n = unsafe { libc::write(inner.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    #[allow(clippy::cast_sign_loss)] // n >= 0 guaranteed
                    Ok(n as usize)
                }
            });

            match result {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => {} // spurious wakeup, re-poll
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ── run_relay ─────────────────────────────────────────────────────────────────

/// Internal read buffer size.
const READ_BUF: usize = 4096;

/// Run the PTY ↔ WebSocket relay until one side closes.
///
/// # Parameters
/// - `pty`: bidirectional I/O endpoint.  Production: `PtyMasterIo`.
///   Tests: `tokio::io::DuplexStream` (from `tokio::io::duplex()`).
/// - `ws_outgoing`: sink that receives outgoing binary frame payloads
///   (caller responsible for framing into `Message::Binary`).
/// - `ws_incoming`: stream of binary frame payloads received from the WS
///   client (caller strips `Message::Binary` wrapper before submitting).
/// - `tracker`: shared command tracker (REQ-TERM-010, REQ-TERM-021).
///   Fed with every byte read from the PTY in the same handler as the WS send.
/// - `on_resize`: called with new `Dims` on every valid resize frame.
///   Production: calls `set_winsize_raw(master_fd_raw, dims)`.
///   Tests: capture or no-op.
/// - `stop_rx`: external stop signal (conversation teardown, etc.).
/// - `conv_id`: for log messages only.
pub struct RelayConfig<F> {
    pub tracker: Arc<Mutex<CommandTracker>>,
    pub on_resize: F,
    pub stop_rx: tokio::sync::watch::Receiver<StopReason>,
    pub conv_id: String,
}

pub async fn run_relay<P, Out, F>(
    pty: P,
    ws_outgoing: Out,
    ws_incoming: impl futures::Stream<Item = Vec<u8>> + Unpin + Send,
    cfg: RelayConfig<F>,
) -> RelayExit
where
    P: AsyncRead + AsyncWrite + Unpin + Send,
    Out: futures::Sink<Vec<u8>> + Unpin + Send,
    Out::Error: std::fmt::Debug,
    F: Fn(Dims) + Send,
{
    let RelayConfig {
        tracker,
        on_resize,
        stop_rx,
        conv_id,
    } = cfg;
    let conv_id = conv_id.as_str();
    let (pty_read, pty_write) = tokio::io::split(pty);

    let ws_closed = Arc::new(tokio::sync::Notify::new());
    // Shared slot: writer publishes its exit reason here before firing
    // `ws_closed`. If the reader wakes on that notify first, it reads the
    // slot so callers see the writer's authoritative `StopReason(...)` /
    // `WsClosed` rather than a generic stop. `None` means the writer is
    // still running (or the reader's own read path fired first).
    let writer_outcome: Arc<Mutex<Option<RelayExit>>> = Arc::new(Mutex::new(None));

    let read_future = relay_reader(
        pty_read,
        ws_outgoing,
        Arc::clone(&tracker),
        Arc::clone(&ws_closed),
        Arc::clone(&writer_outcome),
        conv_id,
    );

    let write_future = relay_writer(
        pty_write,
        ws_incoming,
        on_resize,
        stop_rx,
        ws_closed,
        writer_outcome,
        conv_id,
    );

    // Race: whichever side exits first determines the exit reason.
    // The other side is cancelled when select! resolves.
    //
    // `biased` polls the reader first so `PtyEof` wins over any writer-side
    // condition that happens to be simultaneously ready (e.g. a `stream::empty`
    // `ws_incoming` in tests). The writer communicates its stop-reason back
    // to the reader via the shared `writer_outcome` slot before firing
    // `ws_closed`, so the reader's fallback path can surface the reason
    // honestly rather than substituting a generic `Stopped`.
    tokio::select! {
        biased;
        exit = read_future => exit,
        exit = write_future => exit,
    }
}

// ── Internal reader/writer ────────────────────────────────────────────────────

async fn relay_reader<R, Out>(
    mut pty_read: R,
    mut ws_outgoing: Out,
    tracker: Arc<Mutex<CommandTracker>>,
    ws_closed: Arc<tokio::sync::Notify>,
    writer_outcome: Arc<Mutex<Option<RelayExit>>>,
    conv_id: &str,
) -> RelayExit
where
    R: AsyncRead + Unpin,
    Out: futures::Sink<Vec<u8>> + Unpin,
    Out::Error: std::fmt::Debug,
{
    let mut buf = vec![0u8; READ_BUF];

    loop {
        tokio::select! {
            biased;
            () = ws_closed.notified() => {
                // The writer fired `ws_closed` because it's exiting. It
                // published its reason in `writer_outcome` before notifying, so
                // we surface that to the caller rather than a substituted
                // `Stopped` value. Falling back to `WsClosed` is correct when
                // the slot is somehow empty: `ws_closed` is only notified
                // from writer-exit paths, all of which match `WsClosed`
                // semantics if we have no further information.
                let exit = writer_outcome
                    .lock()
                    .expect("writer_outcome lock")
                    .take()
                    .unwrap_or(RelayExit::WsClosed);
                tracing::debug!(conv_id = %conv_id, ?exit, "Terminal relay: WS closed, reader exiting");
                return exit;
            }
            result = pty_read.read(&mut buf) => match result {
                Ok(0) => {
                    // EOF / EIO (shell exited).
                    tracing::debug!(conv_id = %conv_id, "Terminal relay: PTY EOF");
                    return RelayExit::PtyEof;
                }
                Ok(n) => {
                    let data = &buf[..n];

                    // REQ-TERM-010: feed tracker with SAME bytes in SAME order as WS send.
                    // Both happen here with no conditional path that skips either.
                    tracker.lock().expect("tracker lock").ingest(data);

                    // REQ-TERM-004: 0x00-prefix = PTY data frame.
                    let mut frame = Vec::with_capacity(n + 1);
                    frame.push(0x00u8);
                    frame.extend_from_slice(data);
                    if ws_outgoing.send(frame).await.is_err() {
                        tracing::debug!(conv_id = %conv_id, "Terminal relay: WS send failed");
                        return RelayExit::WsClosed;
                    }
                }
                Err(e) => {
                    tracing::warn!(conv_id = %conv_id, error = %e, "Terminal relay: PTY read error");
                    return RelayExit::PtyEof;
                }
            }
        }
    }
}

async fn relay_writer<W>(
    mut pty_write: W,
    mut ws_incoming: impl futures::Stream<Item = Vec<u8>> + Unpin,
    on_resize: impl Fn(Dims),
    mut stop_rx: tokio::sync::watch::Receiver<StopReason>,
    ws_closed: Arc<tokio::sync::Notify>,
    writer_outcome: Arc<Mutex<Option<RelayExit>>>,
    conv_id: &str,
) -> RelayExit
where
    W: AsyncWrite + Unpin,
{
    // Publish our exit reason to the shared slot, wake the reader, and
    // return. Must be called immediately before returning so the reader
    // sees our reason if the select races to the reader first.
    let exit_with = |exit: RelayExit| -> RelayExit {
        *writer_outcome.lock().expect("writer_outcome lock") = Some(exit.clone());
        ws_closed.notify_one();
        exit
    };

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                // Copy out the reason before notifying — the reader's wakeup
                // could race with another caller resetting the channel (e.g.
                // a reclaimer transitioning Running → Detach → Running for a
                // new relay). The copy freezes our view.
                let reason = *stop_rx.borrow();
                if reason != StopReason::Running {
                    return exit_with(RelayExit::Stopped(reason));
                }
            }
            msg = ws_incoming.next() => {
                let Some(data) = msg else {
                    return exit_with(RelayExit::WsClosed);
                };

                if !dispatch_incoming_frame(&mut pty_write, &on_resize, &data, conv_id).await {
                    return exit_with(RelayExit::WsClosed);
                }
            }
        }
    }
}

/// Dispatch one binary frame from the WS client.
/// Returns `false` if the connection should be terminated.
async fn dispatch_incoming_frame<W>(
    pty_write: &mut W,
    on_resize: &impl Fn(Dims),
    data: &[u8],
    conv_id: &str,
) -> bool
where
    W: AsyncWrite + Unpin,
{
    let Some(&type_byte) = data.first() else {
        return true;
    };
    match type_byte {
        0x00 => {
            // REQ-TERM-004: PTY data frame → write to shell.
            if pty_write.write_all(&data[1..]).await.is_err() {
                tracing::debug!(conv_id = %conv_id, "Terminal relay: PTY write failed");
                return false;
            }
        }
        0x01 => {
            // REQ-TERM-006: resize frame — validate then apply to PTY.
            // CommandTracker has no concept of screen dimensions (ResizeApplied rule).
            if data.len() < 5 {
                return true;
            }
            let cols = u16::from_be_bytes([data[1], data[2]]);
            let rows = u16::from_be_bytes([data[3], data[4]]);
            let Some(dims) = Dims::try_new(cols, rows) else {
                tracing::warn!(conv_id = %conv_id, cols, rows,
                    "Terminal relay: ignoring resize frame with invalid dimension (cols<2 or rows=0)");
                return true;
            };
            on_resize(dims);
            tracing::debug!(conv_id = %conv_id, cols, rows, "Terminal relay: resize applied");
        }
        _ => {
            tracing::debug!(conv_id = %conv_id, type_byte,
                "Terminal relay: unknown frame type, ignored");
        }
    }
    true
}

/// Test-only: synchronously dispatch one binary frame through the relay's
/// frame handler, using a `Cursor` as the PTY write sink. Returns false if
/// the frame signals a disconnect.
///
/// Used by `proptests.rs` resize rejection tests.
#[cfg(test)]
pub(crate) async fn dispatch_frame_for_test(data: &[u8], conv_id: &str) -> bool {
    let mut sink = std::io::Cursor::new(Vec::<u8>::new());
    dispatch_incoming_frame(&mut sink, &|_| {}, data, conv_id).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use tokio::sync::watch;

    fn make_tracker() -> Arc<Mutex<CommandTracker>> {
        Arc::new(Mutex::new(CommandTracker::new("test-session".to_string())))
    }

    fn default_stop() -> tokio::sync::watch::Receiver<StopReason> {
        watch::channel(StopReason::Running).1
    }

    fn null_resize() -> impl Fn(Dims) {
        |_dims| {}
    }

    /// Build a futures mpsc sender adapted to Sink<Vec<u8>>.
    fn ws_out_channel(cap: usize) -> (mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
        mpsc::channel(cap)
    }

    // ── Test 1: PtyOutputForwarded invariant ──────────────────────────────────

    /// Spec invariant `PtyOutputForwarded` (REQ-TERM-004, REQ-TERM-010):
    /// bytes read from the PTY must reach BOTH the WebSocket client AND the
    /// `CommandTracker` — in the same handler, with no conditional path that skips
    /// either.
    ///
    /// This is the core invariant the relay exists to maintain.
    #[tokio::test]
    async fn pty_output_forwarded_to_ws_and_tracker() {
        use crate::terminal::test_helpers::full_command;

        let (mut shell_end, pty_end) = tokio::io::duplex(4096);
        let tracker = make_tracker();
        let (ws_tx, mut ws_rx) = ws_out_channel(32);
        let ws_in = futures::stream::empty::<Vec<u8>>();

        // Write a complete OSC 133 command sequence from the "shell" end, then close.
        let bytes = full_command("ls", "file.txt\n", Some(0));
        shell_end.write_all(&bytes).await.unwrap();
        drop(shell_end); // EOF → relay exits with PtyEof

        let exit = run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                tracker: Arc::clone(&tracker),
                on_resize: null_resize(),
                stop_rx: default_stop(),
                conv_id: "test".to_string(),
            },
        )
        .await;

        assert_eq!(exit, RelayExit::PtyEof);

        // PtyOutputForwarded: bytes reached the WebSocket outgoing channel.
        let frame = ws_rx.try_recv().unwrap();
        assert_eq!(frame[0], 0x00, "outgoing frame must have 0x00 data prefix");
        // First WS frame must carry the OSC 133 bytes.
        assert!(!frame[1..].is_empty(), "WS frame payload must be non-empty");

        // CommandTrackerFedEveryByte: the tracker has the completed record.
        let rec = tracker.lock().unwrap().last_command().cloned();
        let rec = rec.expect("tracker must have a command record after processing");
        assert_eq!(rec.command_text, "ls");
        assert_eq!(rec.output, "file.txt\n");
        assert_eq!(rec.exit_code, Some(0));
    }

    // ── Test 2: WS input forwarded to PTY ─────────────────────────────────────

    /// Input frames (0x00 prefix) received from the WS client must be written
    /// to the PTY — i.e. forwarded to the shell as keyboard input.
    #[tokio::test]
    async fn ws_input_forwarded_to_pty() {
        let (mut shell_end, pty_end) = tokio::io::duplex(4096);
        let tracker = make_tracker();
        let (ws_tx, _ws_rx) = ws_out_channel(32);

        // Construct a 0x00 (data) frame containing keyboard input.
        let mut frame = vec![0x00u8];
        frame.extend_from_slice(b"ls\n");

        // Stream ends after the one frame → relay exits WsClosed.
        let ws_in = futures::stream::once(async { frame }).boxed();

        run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                tracker: Arc::clone(&tracker),
                on_resize: null_resize(),
                stop_rx: default_stop(),
                conv_id: "test".to_string(),
            },
        )
        .await;

        // The PTY write side (shell_end) should have received "ls\n".
        let mut received = vec![0u8; 3];
        shell_end.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"ls\n", "keyboard input must reach the PTY");
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Encode a 0x00 data frame (client → PTY direction).
    fn data_frame(payload: &[u8]) -> Vec<u8> {
        let mut f = vec![0x00u8];
        f.extend_from_slice(payload);
        f
    }

    /// Encode a 0x01 resize frame (client → server).
    fn resize_frame(cols: u16, rows: u16) -> Vec<u8> {
        let mut f = vec![0x01u8];
        f.extend_from_slice(&cols.to_be_bytes());
        f.extend_from_slice(&rows.to_be_bytes());
        f
    }

    /// Drain all frames from the mpsc receiver into a flat Vec of byte payloads
    /// (0x00 prefix stripped).
    fn collect_ws_payloads(rx: &mut mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            assert_eq!(frame[0], 0x00, "unexpected non-data frame in output");
            out.extend_from_slice(&frame[1..]);
        }
        out
    }

    // ========================================================================
    // Property 1: PtyOutputForwarded — large & multiple chunks
    // ========================================================================
    //
    // Verifies that all PTY bytes are relayed to the WebSocket with no gaps.

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 256,
            ..proptest::test_runner::Config::default()
        })]

        #[test]
        fn prop_pty_output_forwarded_large_sequences(
            chunks in proptest::collection::vec(
                proptest::collection::vec(proptest::num::u8::ANY, 1..512),
                1..10,
            ),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let (mut shell_end, pty_end) = tokio::io::duplex(65_536);
                let tracker = make_tracker();
                let (ws_tx, mut ws_rx) = ws_out_channel(256);
                let ws_in = futures::stream::empty::<Vec<u8>>().boxed();

                let expected: Vec<u8> = chunks.concat();

                // Write all chunks then close the shell end to trigger EOF.
                for chunk in &chunks {
                    shell_end.write_all(chunk).await.unwrap();
                }
                drop(shell_end);

                let exit = run_relay(
                    pty_end, ws_tx, ws_in,
                    RelayConfig {
                        tracker: Arc::clone(&tracker),
                        on_resize: null_resize(),
                        stop_rx: default_stop(),
                        conv_id: "prop-test".to_string(),
                    },
                ).await;

                prop_assert_eq!(exit, RelayExit::PtyEof);

                // WS frames must contain exactly the expected bytes.
                let ws_bytes = collect_ws_payloads(&mut ws_rx);
                prop_assert_eq!(
                    ws_bytes, expected.clone(),
                    "WS frames must carry all PTY bytes in order (PtyOutputForwarded)"
                );

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Property 2: ResizeFrameApplied — on_resize called for each valid resize
    // ========================================================================

    proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 256,
            ..proptest::test_runner::Config::default()
        })]

        #[test]
        fn prop_resize_frames_call_on_resize(
            ops in proptest::collection::vec(
                prop_oneof![
                    (2u16..=200u16, 1u16..=80u16)
                        .prop_map(|(cols, rows)| (true, cols, rows)),
                    proptest::collection::vec(proptest::num::u8::ANY, 0..128)
                        .prop_map(|bytes| (false, u16::try_from(bytes.len()).unwrap_or(u16::MAX), 0u16)),
                ],
                1..20,
            ),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let resizes_seen = Arc::new(Mutex::new(Vec::<Dims>::new()));
                let resizes_spy = Arc::clone(&resizes_seen);

                let (_shell_end, pty_end) = tokio::io::duplex(65_536);
                let tracker = make_tracker();
                let (ws_tx, _ws_rx) = ws_out_channel(256);

                let mut ws_frames: Vec<Vec<u8>> = Vec::new();
                let mut last_resize: Option<Dims> = None;
                let mut resize_count = 0;

                for &(is_resize, a, b) in &ops {
                    if is_resize {
                        ws_frames.push(resize_frame(a, b));
                        last_resize = Some(Dims { cols: a, rows: b });
                        resize_count += 1;
                    }
                }

                let ws_in = futures::stream::iter(ws_frames).boxed();

                run_relay(
                    pty_end, ws_tx, ws_in,
                    RelayConfig {
                        tracker: Arc::clone(&tracker),
                        on_resize: move |dims| {
                            resizes_spy.lock().unwrap().push(dims);
                        },
                        stop_rx: default_stop(),
                        conv_id: "prop-resize".to_string(),
                    },
                ).await;

                if last_resize.is_some() {
                    let seen = resizes_seen.lock().unwrap();
                    prop_assert_eq!(seen.len(), resize_count,
                        "on_resize must be called exactly once per valid resize frame");
                }

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Property 2b: Resize spy — deep scenario test
    // ========================================================================

    #[tokio::test]
    async fn resize_spy_deep_scenario() {
        let resizes_applied: Arc<Mutex<Vec<Dims>>> = Arc::new(Mutex::new(Vec::new()));
        let spy = Arc::clone(&resizes_applied);

        let tracker = make_tracker();
        let (ws_tx, _ws_rx) = ws_out_channel(32);

        let ws_frames = vec![
            resize_frame(40, 10),
            data_frame(b"hello"),
            resize_frame(120, 30),
            data_frame(b"world"),
        ];
        let ws_in = futures::stream::iter(ws_frames).boxed();

        let (_shell_end, pty_end) = tokio::io::duplex(4096);

        run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                tracker: Arc::clone(&tracker),
                on_resize: move |dims| spy.lock().unwrap().push(dims),
                stop_rx: default_stop(),
                conv_id: "deep-spy".to_string(),
            },
        )
        .await;

        let applied = resizes_applied.lock().unwrap();
        assert_eq!(applied.len(), 2, "exactly two resize frames sent");
        assert_eq!(applied[0], Dims { cols: 40, rows: 10 });
        assert_eq!(
            applied[1],
            Dims {
                cols: 120,
                rows: 30
            }
        );
    }

    // ========================================================================
    // Property 3: RelayExitsCleanly — no bytes lost on EOF
    // ========================================================================

    proptest! {
        #[test]
        fn prop_relay_exits_cleanly_no_bytes_lost(
            chunks in proptest::collection::vec(
                proptest::collection::vec(proptest::num::u8::ANY, 1..256),
                1..8,
            ),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let (mut shell_end, pty_end) = tokio::io::duplex(65_536);
                let tracker = make_tracker();
                let (ws_tx, mut ws_rx) = ws_out_channel(256);
                let ws_in = futures::stream::empty::<Vec<u8>>().boxed();

                let expected: Vec<u8> = chunks.concat();

                for chunk in &chunks {
                    shell_end.write_all(chunk).await.unwrap();
                }
                drop(shell_end);

                let exit = run_relay(
                    pty_end, ws_tx, ws_in,
                    RelayConfig {
                        tracker: Arc::clone(&tracker),
                        on_resize: null_resize(),
                        stop_rx: default_stop(),
                        conv_id: "clean-exit".to_string(),
                    },
                ).await;

                prop_assert_eq!(exit, RelayExit::PtyEof,
                    "relay must exit cleanly on PTY EOF, not error");

                let ws_bytes = collect_ws_payloads(&mut ws_rx);
                prop_assert_eq!(ws_bytes, expected,
                    "no bytes must be lost when shell closes (RelayExitsCleanly)");

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Property 4: UnknownFramesIgnored — malformed input never disconnects
    // ========================================================================

    proptest! {
        #[test]
        fn prop_unknown_frame_type_ignored(
            type_byte in 0x02u8..=0xffu8,
            payload in proptest::collection::vec(proptest::num::u8::ANY, 0..256),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut frame = vec![type_byte];
                frame.extend_from_slice(&payload);

                let result = dispatch_frame_for_test(&frame, "test").await;

                prop_assert!(result, "unknown frame type must not disconnect the session");

                Ok(())
            }).map_err(|e: proptest::test_runner::TestCaseError| e)?;
        }

        #[test]
        fn prop_short_resize_frame_ignored(
            payload_len in 0usize..4usize,
            payload in proptest::collection::vec(proptest::num::u8::ANY, 4),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut frame = vec![0x01u8];
                frame.extend_from_slice(&payload[..payload_len]);

                let result = dispatch_frame_for_test(&frame, "test").await;

                prop_assert!(result, "short resize frame must not disconnect");

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Reclaim-on-reconnect: stop-reason branching
    // ========================================================================
    //
    // Task 24691: a WS reconnect for an already-active session signals the
    // sitting relay via `StopReason::Detach`. The relay must exit with
    // `RelayExit::Stopped(Detach)` (not a generic `Stopped`), leaving the
    // tracker and any other handle state untouched so a second relay can
    // continue the session.

    /// Detach exit: the relay returns `Stopped(Detach)` and the tracker
    /// retains the command records it captured before the detach signal.
    #[tokio::test]
    async fn detach_exit_preserves_tracker_state() {
        use crate::terminal::test_helpers::full_command;

        let (mut shell_end, pty_end) = tokio::io::duplex(4096);
        let tracker = make_tracker();
        let (ws_tx, _ws_rx) = ws_out_channel(32);
        let ws_in = futures::stream::pending::<Vec<u8>>().boxed();

        let (stop_tx, stop_rx) = watch::channel(StopReason::Running);

        // Write a complete command into the tracker before stopping.
        let bytes = full_command("echo hi", "hi\n", Some(0));
        shell_end.write_all(&bytes).await.unwrap();

        // Drive the relay, then send Detach after the tracker has ingested.
        let tracker_for_relay = Arc::clone(&tracker);
        let relay = tokio::spawn(async move {
            run_relay(
                pty_end,
                ws_tx,
                ws_in,
                RelayConfig {
                    tracker: tracker_for_relay,
                    on_resize: null_resize(),
                    stop_rx,
                    conv_id: "detach-test".to_string(),
                },
            )
            .await
        });

        // Give the relay a moment to ingest the write, then signal detach.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop_tx.send(StopReason::Detach).unwrap();

        let exit = relay.await.unwrap();
        assert_eq!(
            exit,
            RelayExit::Stopped(StopReason::Detach),
            "detach signal must surface as Stopped(Detach)"
        );

        // Tracker kept the command record across the detach — shell-and-session
        // state survives, only the relay is torn down.
        let rec = tracker
            .lock()
            .unwrap()
            .last_command()
            .cloned()
            .expect("tracker must retain the command record across detach");
        assert_eq!(rec.command_text, "echo hi");
        assert_eq!(rec.exit_code, Some(0));
    }

    /// TearDown exit: the relay returns `Stopped(TearDown)` so the handler
    /// can branch into full shell teardown (REQ-TERM-012).
    #[tokio::test]
    async fn teardown_exit_returns_teardown_reason() {
        let (_shell_end, pty_end) = tokio::io::duplex(4096);
        let tracker = make_tracker();
        let (ws_tx, _ws_rx) = ws_out_channel(32);
        let ws_in = futures::stream::pending::<Vec<u8>>().boxed();

        let (stop_tx, stop_rx) = watch::channel(StopReason::Running);

        let relay = tokio::spawn(async move {
            run_relay(
                pty_end,
                ws_tx,
                ws_in,
                RelayConfig {
                    tracker,
                    on_resize: null_resize(),
                    stop_rx,
                    conv_id: "teardown-test".to_string(),
                },
            )
            .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        stop_tx.send(StopReason::TearDown).unwrap();

        let exit = relay.await.unwrap();
        assert_eq!(exit, RelayExit::Stopped(StopReason::TearDown));
    }

    /// Second relay over the same tracker continues processing after a detach.
    /// Emulates the reclaim path: relay 1 exits via Detach, relay 2 starts
    /// fresh on the same tracker and captures another command.
    #[tokio::test]
    async fn second_relay_continues_over_same_tracker() {
        use crate::terminal::test_helpers::full_command;

        let tracker = make_tracker();

        // --- Relay 1: capture "first" command, then detach. ------------------
        let (mut shell_end_1, pty_end_1) = tokio::io::duplex(4096);
        let (ws_tx_1, _ws_rx_1) = ws_out_channel(32);
        let ws_in_1 = futures::stream::pending::<Vec<u8>>().boxed();
        let (stop_tx_1, stop_rx_1) = watch::channel(StopReason::Running);

        let tracker_clone = Arc::clone(&tracker);
        let relay_1 = tokio::spawn(async move {
            run_relay(
                pty_end_1,
                ws_tx_1,
                ws_in_1,
                RelayConfig {
                    tracker: tracker_clone,
                    on_resize: null_resize(),
                    stop_rx: stop_rx_1,
                    conv_id: "relay-1".to_string(),
                },
            )
            .await
        });

        shell_end_1
            .write_all(&full_command("first", "ok\n", Some(0)))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop_tx_1.send(StopReason::Detach).unwrap();

        let exit_1 = relay_1.await.unwrap();
        assert_eq!(exit_1, RelayExit::Stopped(StopReason::Detach));

        // --- Relay 2: capture "second" command on the SAME tracker. ---------
        let (mut shell_end_2, pty_end_2) = tokio::io::duplex(4096);
        let (ws_tx_2, _ws_rx_2) = ws_out_channel(32);
        let ws_in_2 = futures::stream::pending::<Vec<u8>>().boxed();
        let (stop_tx_2, stop_rx_2) = watch::channel(StopReason::Running);

        let tracker_clone_2 = Arc::clone(&tracker);
        let relay_2 = tokio::spawn(async move {
            run_relay(
                pty_end_2,
                ws_tx_2,
                ws_in_2,
                RelayConfig {
                    tracker: tracker_clone_2,
                    on_resize: null_resize(),
                    stop_rx: stop_rx_2,
                    conv_id: "relay-2".to_string(),
                },
            )
            .await
        });

        shell_end_2
            .write_all(&full_command("second", "also ok\n", Some(0)))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop_tx_2.send(StopReason::Detach).unwrap();
        let exit_2 = relay_2.await.unwrap();
        assert_eq!(exit_2, RelayExit::Stopped(StopReason::Detach));

        // Both commands are present in the same tracker, in order.
        let tracker = tracker.lock().unwrap();
        let recent = tracker.recent_commands(5);
        let cmds: Vec<&str> = recent.iter().map(|r| r.command_text.as_str()).collect();
        assert!(
            cmds.contains(&"first") && cmds.contains(&"second"),
            "tracker must contain both commands across the relay swap; got {cmds:?}"
        );
    }

    /// PtyEof regression: if the shell exits while a stop signal is also
    /// arriving, the exit reason must still be `PtyEof` so the handler runs
    /// full teardown. The reader detects EOF first and wins the race.
    #[tokio::test]
    async fn pty_eof_triggers_full_teardown_even_with_pending_stop() {
        let (shell_end, pty_end) = tokio::io::duplex(4096);
        let tracker = make_tracker();
        let (ws_tx, _ws_rx) = ws_out_channel(32);
        let ws_in = futures::stream::pending::<Vec<u8>>().boxed();

        // Pre-fill a Running channel — we never actually send a stop. This
        // test's intent: when the shell closes, PtyEof wins, regardless of
        // what the stop channel might have been poised to carry.
        let (_stop_tx, stop_rx) = watch::channel(StopReason::Running);

        // Close the shell end immediately — drops the write end, reader gets EOF.
        drop(shell_end);

        let exit = run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                tracker,
                on_resize: null_resize(),
                stop_rx,
                conv_id: "eof-regression".to_string(),
            },
        )
        .await;

        assert_eq!(
            exit,
            RelayExit::PtyEof,
            "PtyEof must be reported for shell-exit, not Stopped/WsClosed"
        );
    }
}
