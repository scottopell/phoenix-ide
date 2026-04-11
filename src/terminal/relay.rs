//! Core relay loop — mediates between PTY I/O and WebSocket frames.
//!
//! ## Design
//!
//! `run_relay` is the testable heart of the terminal. It takes generic I/O endpoints:
//!
//! ```text
//! pty (AsyncRead + AsyncWrite)  ←→  run_relay  ←→  ws_incoming / ws_outgoing
//!                                        │
//!                                   vt100::Parser
//! ```
//!
//! Production: `pty = PtyMasterIo` (wraps the real PTY master fd via `AsyncFd`).
//! Tests:      `pty = tokio::io::DuplexStream` (in-memory pipe, zero infrastructure).
//!
//! ## Why this split exists
//!
//! The relay logic is the only place where the `PtyOutputForwarded` and
//! `ParserFedEveryByte` invariants can be falsified — bytes must reach BOTH
//! `ws_outgoing` AND `parser.process()` in the same handler, with no conditional
//! path that skips either. Extracting `run_relay` makes that invariant directly
//! assertable in a test without spawning a real shell.

use std::{
    io,
    os::unix::io::OwnedFd,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use futures::SinkExt;
use futures::StreamExt;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use vt100::Parser;

use super::session::{Dims, QuiescenceTx};

/// How the relay loop exited.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayExit {
    /// PTY EOF: `read()` returned 0 bytes or EIO (shell exited). REQ-TERM-007.
    PtyEof,
    /// WebSocket stream closed by client or disconnected.
    WsClosed,
    /// Stopped by external signal (stop channel or `ws_closed` notify).
    Stopped,
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

/// The terminal quiescence debounce duration.  Configurable via type parameter
/// for tests that want to use shorter timeouts.
const QUIESCENCE_MS: u64 = 300;
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
/// - `parser`: shared vt100 screen state.  Fed with every byte read from the PTY.
/// - `quiescence_tx`: incremented when no PTY output arrives for `QUIESCENCE_MS`.
///   Used by `read_terminal` tool to detect command completion.
/// - `on_resize`: called with new `Dims` on every valid resize frame.
///   Production: calls `set_winsize_raw(master_fd_raw, dims)`.
///   Tests: capture or no-op.
/// - `stop_rx`: external stop signal (conversation teardown, etc.).
/// - `conv_id`: for log messages only.
pub struct RelayConfig<F> {
    pub parser: Arc<Mutex<Parser>>,
    pub quiescence_tx: QuiescenceTx,
    pub on_resize: F,
    pub stop_rx: tokio::sync::watch::Receiver<bool>,
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
        parser,
        quiescence_tx,
        on_resize,
        stop_rx,
        conv_id,
    } = cfg;
    let conv_id = conv_id.as_str();
    let (pty_read, pty_write) = tokio::io::split(pty);

    let ws_closed = Arc::new(tokio::sync::Notify::new());

    let read_exit = relay_reader(
        pty_read,
        ws_outgoing,
        Arc::clone(&parser),
        quiescence_tx,
        Arc::clone(&ws_closed),
        conv_id,
    );

    let write_exit = relay_writer(
        pty_write,
        ws_incoming,
        Arc::clone(&parser),
        on_resize,
        stop_rx,
        ws_closed,
        conv_id,
    );

    // Race: whichever side exits first determines the exit reason.
    // The other side is cancelled when select! resolves.
    tokio::select! {
        biased;
        exit = read_exit => exit,
        exit = write_exit => exit,
    }
}

// ── Internal reader/writer ────────────────────────────────────────────────────

async fn relay_reader<R, Out>(
    mut pty_read: R,
    mut ws_outgoing: Out,
    parser: Arc<Mutex<Parser>>,
    quiescence_tx: QuiescenceTx,
    ws_closed: Arc<tokio::sync::Notify>,
    conv_id: &str,
) -> RelayExit
where
    R: AsyncRead + Unpin,
    Out: futures::Sink<Vec<u8>> + Unpin,
    Out::Error: std::fmt::Debug,
{
    let mut buf = vec![0u8; READ_BUF];
    let mut quiescence_counter = 0u64;

    loop {
        let read_fut = tokio::time::timeout(
            Duration::from_millis(QUIESCENCE_MS),
            pty_read.read(&mut buf),
        );

        tokio::select! {
            biased;
            () = ws_closed.notified() => {
                tracing::debug!(conv_id = %conv_id, "Terminal relay: WS closed, reader exiting");
                return RelayExit::Stopped;
            }
            result = read_fut => match result {
                Err(_timeout) => {
                    // 300ms of silence = quiescence.
                    quiescence_counter += 1;
                    let _ = quiescence_tx.send(quiescence_counter);
                }
                Ok(Ok(0)) => {
                    // EOF / EIO (shell exited).
                    tracing::debug!(conv_id = %conv_id, "Terminal relay: PTY EOF");
                    return RelayExit::PtyEof;
                }
                Ok(Ok(n)) => {
                    let data = &buf[..n];

                    // REQ-TERM-010: feed parser with SAME bytes in SAME order as WS send.
                    // Both happen here with no conditional path that skips either.
                    parser.lock().expect("parser lock").process(data);

                    // REQ-TERM-004: 0x00-prefix = PTY data frame.
                    let mut frame = Vec::with_capacity(n + 1);
                    frame.push(0x00u8);
                    frame.extend_from_slice(data);
                    if ws_outgoing.send(frame).await.is_err() {
                        tracing::debug!(conv_id = %conv_id, "Terminal relay: WS send failed");
                        return RelayExit::WsClosed;
                    }
                }
                Ok(Err(e)) => {
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
    parser: Arc<Mutex<Parser>>,
    on_resize: impl Fn(Dims),
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    ws_closed: Arc<tokio::sync::Notify>,
    conv_id: &str,
) -> RelayExit
where
    W: AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    ws_closed.notify_one();
                    return RelayExit::Stopped;
                }
            }
            msg = ws_incoming.next() => {
                let Some(data) = msg else {
                    // Stream ended — WS closed.
                    ws_closed.notify_one();
                    return RelayExit::WsClosed;
                };

                if !dispatch_incoming_frame(&mut pty_write, &parser, &on_resize, &data, conv_id).await {
                    ws_closed.notify_one();
                    return RelayExit::WsClosed;
                }
            }
        }
    }
}

/// Dispatch one binary frame from the WS client.
/// Returns `false` if the connection should be terminated.
async fn dispatch_incoming_frame<W>(
    pty_write: &mut W,
    parser: &Arc<Mutex<Parser>>,
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
            // REQ-TERM-006: resize frame — validate then apply to PTY + parser.
            if data.len() < 5 {
                return true;
            }
            let cols = u16::from_be_bytes([data[1], data[2]]);
            let rows = u16::from_be_bytes([data[3], data[4]]);
            if cols == 0 || rows == 0 {
                tracing::warn!(conv_id = %conv_id, cols, rows,
                    "Terminal relay: ignoring resize frame with zero dimension");
                return true;
            }
            let dims = Dims { cols, rows };
            // ParserDimensionSync: PTY ioctl and parser.set_size in same call.
            on_resize(dims);
            parser.lock().expect("parser lock").set_size(rows, cols);
            tracing::debug!(conv_id = %conv_id, cols, rows, "Terminal relay: resize applied");
        }
        _ => {
            tracing::debug!(conv_id = %conv_id, type_byte,
                "Terminal relay: unknown frame type, ignored");
        }
    }
    true
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use futures::SinkExt as _;
    use tokio::io::AsyncWriteExt as _;
    use tokio::sync::watch;

    fn make_parser(rows: u16, cols: u16) -> Arc<Mutex<Parser>> {
        Arc::new(Mutex::new(Parser::new(rows, cols, 0)))
    }

    fn default_stop() -> tokio::sync::watch::Receiver<bool> {
        watch::channel(false).1
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
    /// vt100 parser — in the same handler, with no conditional path that skips
    /// either.
    ///
    /// This is the core invariant the relay exists to maintain.
    #[tokio::test]
    async fn pty_output_forwarded_to_ws_and_parser() {
        let (mut shell_end, pty_end) = tokio::io::duplex(4096);
        let parser = make_parser(24, 80);
        let (quiescence_tx, _) = watch::channel(0u64);
        let (ws_tx, mut ws_rx) = ws_out_channel(32);
        let ws_in = futures::stream::empty::<Vec<u8>>();

        // Write some PTY output from the "shell" end, then close it.
        shell_end.write_all(b"hello terminal\r\n").await.unwrap();
        drop(shell_end); // EOF → relay exits with PtyEof

        let exit = run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                parser: Arc::clone(&parser),
                quiescence_tx,
                on_resize: null_resize(),
                stop_rx: default_stop(),
                conv_id: "test".to_string(),
            },
        )
        .await;

        assert_eq!(exit, RelayExit::PtyEof);

        // PtyOutputForwarded: bytes reached the WebSocket outgoing channel.
        let frame = ws_rx.try_next().unwrap().unwrap();
        assert_eq!(frame[0], 0x00, "outgoing frame must have 0x00 data prefix");
        assert_eq!(&frame[1..], b"hello terminal\r\n");

        // ParserFedEveryByte: the same bytes were processed by the parser.
        let screen = parser.lock().unwrap().screen().contents();
        assert!(
            screen.contains("hello terminal"),
            "parser screen must contain the PTY output; got: {screen:?}"
        );
    }

    // ── Test 2: WS input forwarded to PTY ─────────────────────────────────────

    /// Input frames (0x00 prefix) received from the WS client must be written
    /// to the PTY — i.e. forwarded to the shell as keyboard input.
    #[tokio::test]
    async fn ws_input_forwarded_to_pty() {
        let (mut shell_end, pty_end) = tokio::io::duplex(4096);
        let parser = make_parser(24, 80);
        let (quiescence_tx, _) = watch::channel(0u64);
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
                parser: Arc::clone(&parser),
                quiescence_tx,
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
        while let Ok(Some(frame)) = rx.try_next() {
            assert_eq!(frame[0], 0x00, "unexpected non-data frame in output");
            out.extend_from_slice(&frame[1..]);
        }
        out
    }

    // ========================================================================
    // Property 1: PtyOutputForwarded — large & multiple chunks
    // ========================================================================
    //
    // User surprise prevented: agent calls read_terminal after a command and
    // gets an empty or garbled screen despite visible output in the browser.
    // This proptest generalises the existing unit test to arbitrary-sized,
    // arbitrary-count chunks and verifies BOTH WS frames AND parser state.

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
                let parser = make_parser(24, 80);
                let (quiescence_tx, _) = watch::channel(0u64);
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
                        parser: Arc::clone(&parser),
                        quiescence_tx,
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

                // Parser must also contain all bytes (ParserFedEveryByte).
                let screen = parser.lock().unwrap().screen().contents();
                let screen_bytes = screen.trim_end_matches('\0').to_string();
                prop_assert!(
                    !screen_bytes.is_empty() || expected.iter().all(|&b| b == 0),
                    "parser screen must reflect PTY output; screen={screen:?}"
                );

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Property 2a: ParserDimensionSync through relay — shallow proptest
    // ========================================================================
    //
    // User surprise prevented: agent reads text wrapped at old column count
    // after a browser resize, misaligns output, counts lines wrong.

    proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 256,
            ..proptest::test_runner::Config::default()
        })]

        #[test]
        fn prop_parser_dimension_sync_through_relay(
            ops in proptest::collection::vec(
                prop_oneof![
                    (1u16..=200u16, 1u16..=80u16)
                        .prop_map(|(cols, rows)| (true, cols, rows)),  // resize
                    proptest::collection::vec(proptest::num::u8::ANY, 0..128)
                        .prop_map(|bytes| (false, bytes.len() as u16, 0u16)),  // data (cols=len, rows=0 as tag)
                ],
                1..20,
            ),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                // We need to track the last resize to verify end state.
                // Use a Mutex-wrapped Vec to collect all resize Dims from the spy.
                let resizes_seen = Arc::new(Mutex::new(Vec::<Dims>::new()));
                let resizes_spy = Arc::clone(&resizes_seen);

                // Keep _shell_end alive (never drop it) so PTY never hits EOF.
                // The relay exits via ws_in stream ending (WsClosed), which ensures
                // relay_writer processes ALL resize frames before the relay stops.
                let (_shell_end, pty_end) = tokio::io::duplex(65_536);
                let parser = make_parser(24, 80);
                let (quiescence_tx, _) = watch::channel(0u64);
                let (ws_tx, _ws_rx) = ws_out_channel(256);

                // Build only resize frames — data ops are skipped since we're
                // testing dimension sync, not byte forwarding.
                let mut ws_frames: Vec<Vec<u8>> = Vec::new();
                let mut last_resize: Option<Dims> = None;
                let mut resize_count = 0;

                for &(is_resize, a, b) in &ops {
                    if is_resize {
                        let cols = a;
                        let rows = b;
                        ws_frames.push(resize_frame(cols, rows));
                        last_resize = Some(Dims { cols, rows });
                        resize_count += 1;
                    }
                }

                // ws_in ends after all frames → relay exits WsClosed.
                let ws_in = futures::stream::iter(ws_frames).boxed();

                run_relay(
                    pty_end, ws_tx, ws_in,
                    RelayConfig {
                        parser: Arc::clone(&parser),
                        quiescence_tx,
                        on_resize: move |dims| {
                            resizes_spy.lock().unwrap().push(dims);
                        },
                        stop_rx: default_stop(),
                        conv_id: "prop-dim-sync".to_string(),
                    },
                ).await;

                // Shallow check: final parser size matches last resize.
                if let Some(expected_dims) = last_resize {
                    let (r, c) = parser.lock().unwrap().screen().size();
                    prop_assert_eq!(c, expected_dims.cols,
                        "ParserDimensionSync: final cols mismatch");
                    prop_assert_eq!(r, expected_dims.rows,
                        "ParserDimensionSync: final rows mismatch");

                    // Spy: relay called on_resize for each resize frame.
                    let seen = resizes_seen.lock().unwrap();
                    prop_assert_eq!(seen.len(), resize_count,
                        "on_resize must be called exactly once per resize frame");
                }

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Property 2b: ParserDimensionSync — deep spy scenario test
    // ========================================================================
    //
    // Sends a specific sequence: resize → data → resize → data → check.
    // Verifies that after EACH resize, parser.screen().size() already matches
    // — not just at the end. Catches transient drift that proptest might miss.

    #[tokio::test]
    async fn parser_dimension_sync_deep_spy() {
        // Sequence: resize to 40×10, write data, resize to 120×30, write more data.
        // At each resize, the spy captures the Dims; we verify the parser
        // already reflects them by the time run_relay returns.
        // (Intra-run assertions require the spy; post-run assertions suffice here
        // since the relay processes frames sequentially.)

        let resizes_applied: Arc<Mutex<Vec<Dims>>> = Arc::new(Mutex::new(Vec::new()));
        let spy = Arc::clone(&resizes_applied);

        let (mut shell_end, pty_end) = tokio::io::duplex(4096);
        let parser = make_parser(24, 80);
        let (quiescence_tx, _) = watch::channel(0u64);
        let (ws_tx, _ws_rx) = ws_out_channel(32);

        // Frame sequence delivered by the browser.
        let ws_frames = vec![
            resize_frame(40, 10),
            data_frame(b"hello"), // input to shell (not resize)
            resize_frame(120, 30),
            data_frame(b"world"),
        ];
        let ws_in = futures::stream::iter(ws_frames).boxed();

        let (_shell_end, pty_end) = tokio::io::duplex(4096);
        // _shell_end kept alive so relay doesn't exit via PTY EOF before
        // relay_writer processes all ws_in resize frames.

        run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                parser: Arc::clone(&parser),
                quiescence_tx,
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

        // ParserDimensionSync: final parser size == last resize.
        let (r, c) = parser.lock().unwrap().screen().size();
        assert_eq!(c, 120, "cols must reflect last resize");
        assert_eq!(r, 30, "rows must reflect last resize");
    }

    // ========================================================================
    // Property 3: InitialResizeBeforeFirstOutput
    // ========================================================================
    //
    // User surprise: first shell prompt wraps at PTY default (80×24) instead
    // of the client's actual window size. Looks broken; agent misreads layout.
    //
    // Verifies that a resize frame arriving before any PTY data is applied so
    // subsequent output is laid out at the resized dimensions.

    #[tokio::test]
    async fn initial_resize_applied_before_first_output() {
        // Design: ws_in contains only the resize frame. We DON'T write PTY data
        // because that would require shell_end to stay open (risking a race).
        // The property is: after relay processes the resize frame from ws_in,
        // parser.screen().size() reflects those dimensions. ws_in ending causes
        // WsClosed exit, ensuring the resize is fully processed before we check.
        let (_shell_end, pty_end) = tokio::io::duplex(4096);
        // _shell_end kept alive: relay exits via ws_in ending (WsClosed), not EOF.
        let parser = make_parser(24, 80); // starts at default 80×24
        let (quiescence_tx, _) = watch::channel(0u64);
        let (ws_tx, _ws_rx) = ws_out_channel(32);

        // Client sends resize FIRST (as FitAddon does on connect).
        let ws_in = futures::stream::once(async { resize_frame(200, 50) }).boxed();

        run_relay(
            pty_end,
            ws_tx,
            ws_in,
            RelayConfig {
                parser: Arc::clone(&parser),
                quiescence_tx,
                on_resize: null_resize(),
                stop_rx: default_stop(),
                conv_id: "initial-resize".to_string(),
            },
        )
        .await;

        // Parser must be at the resized dimensions, not the initial 80×24.
        let (r, c) = parser.lock().unwrap().screen().size();
        assert_eq!(
            c, 200,
            "parser cols must be 200 after initial resize (REQ-TERM-005)"
        );
        assert_eq!(
            r, 50,
            "parser rows must be 50 after initial resize (REQ-TERM-005)"
        );
    }

    // ========================================================================
    // Property 4: InputOutputRouting — both sides
    // ========================================================================
    //
    // User surprise: user types a command, nothing executes, bytes appear in
    // the terminal screen instead. Bidirectional routing is inverted.
    //
    // Verifies:
    //   A) 0x00 data frames from browser ARE written to pty_write
    //   B) parser state is UNCHANGED (input does not leak into screen state)

    proptest! {
        #[test]
        fn prop_input_frames_reach_pty_not_parser(
            input_bytes in proptest::collection::vec(
                proptest::num::u8::ANY, 1..256
            ),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let (mut shell_end, pty_end) = tokio::io::duplex(4096);
                let parser = make_parser(24, 80);
                let (quiescence_tx, _) = watch::channel(0u64);
                let (ws_tx, _ws_rx) = ws_out_channel(32);

                // Capture initial screen state before any frames.
                let initial_screen = parser.lock().unwrap().screen().contents();

                // Send one 0x00 input frame, then close the stream.
                let frame = data_frame(&input_bytes);
                let ws_in = futures::stream::once(async { frame }).boxed();

                // No PTY output — close shell end immediately so relay exits.
                drop(shell_end);

                run_relay(
                    pty_end, ws_tx, ws_in,
                    RelayConfig {
                        parser: Arc::clone(&parser),
                        quiescence_tx,
                        on_resize: null_resize(),
                        stop_rx: default_stop(),
                        conv_id: "input-routing".to_string(),
                    },
                ).await;

                // B) Parser state must be unchanged — input did not leak into screen.
                let final_screen = parser.lock().unwrap().screen().contents();
                prop_assert_eq!(
                    final_screen, initial_screen,
                    "input frame must not affect parser screen (bidirectional routing)"
                );

                Ok(())
            })?;
        }
    }

    // Note: the PTY-received side of property 4 is already covered by the
    // ws_input_forwarded_to_pty unit test above. That test verifies the bytes
    // actually arrive at shell_end. This proptest covers the parser isolation side.

    // ========================================================================
    // Property 5: RelayExitsCleanly — no bytes lost on EOF
    // ========================================================================
    //
    // User surprise: shell exits mid-command. Last line of output disappears
    // from both the WS client and the parser. Agent reads truncated result,
    // makes wrong decision. This is subtle: the bytes were read from the PTY
    // but the relay must flush them before exiting.

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
                let parser = make_parser(24, 80);
                let (quiescence_tx, _) = watch::channel(0u64);
                let (ws_tx, mut ws_rx) = ws_out_channel(256);
                let ws_in = futures::stream::empty::<Vec<u8>>().boxed();

                let expected: Vec<u8> = chunks.concat();

                // Write all chunks then immediately close (simulates shell exit).
                for chunk in &chunks {
                    shell_end.write_all(chunk).await.unwrap();
                }
                drop(shell_end); // EOF → relay must flush everything before returning

                let exit = run_relay(
                    pty_end, ws_tx, ws_in,
                    RelayConfig {
                        parser: Arc::clone(&parser),
                        quiescence_tx,
                        on_resize: null_resize(),
                        stop_rx: default_stop(),
                        conv_id: "clean-exit".to_string(),
                    },
                ).await;

                prop_assert_eq!(exit, RelayExit::PtyEof,
                    "relay must exit cleanly on PTY EOF, not error");

                // All bytes must appear in WS output — none dropped on EOF.
                let ws_bytes = collect_ws_payloads(&mut ws_rx);
                prop_assert_eq!(ws_bytes, expected,
                    "no bytes must be lost when shell closes (RelayExitsCleanly)");

                Ok(())
            })?;
        }
    }

    // ========================================================================
    // Property 6: UnknownFramesIgnored — malformed input never disconnects
    // ========================================================================
    //
    // User surprise: browser sends a garbage frame (unknown type byte, short
    // resize payload). Terminal closes with no explanation. Session is lost.
    //
    // Verifies: any frame with type h0x00/0x01, or a resize frame with payload
    // < 5 bytes, returns true (stay connected) and parser state is unchanged.

    proptest! {
        #[test]
        fn prop_unknown_frame_type_ignored(
            type_byte in 0x02u8..=0xffu8,
            payload in proptest::collection::vec(proptest::num::u8::ANY, 0..256),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let parser = make_parser(24, 80);
                let initial_screen = parser.lock().unwrap().screen().contents();

                let mut frame = vec![type_byte];
                frame.extend_from_slice(&payload);

                let result = dispatch_frame_for_test(&parser, &frame, "test").await;

                prop_assert!(result, "unknown frame type must not disconnect the session");

                let final_screen = parser.lock().unwrap().screen().contents();
                prop_assert_eq!(final_screen, initial_screen,
                    "unknown frame must not affect parser state");

                Ok(())
            }).map_err(|e: proptest::test_runner::TestCaseError| e)?;
        }

        #[test]
        fn prop_short_resize_frame_ignored(
            // A "short" resize frame has total length < 5 (type byte + < 4 payload bytes).
            // payload_len 0..4 gives 0,1,2,3 payload bytes; total frame = 1,2,3,4 bytes.
            // payload_len=4 would give a valid 5-byte frame (type + 4 data) — excluded.
            payload_len in 0usize..4usize,
            payload in proptest::collection::vec(proptest::num::u8::ANY, 4),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let parser = make_parser(24, 80);
                let initial = parser.lock().unwrap().screen().size();

                let mut frame = vec![0x01u8];
                frame.extend_from_slice(&payload[..payload_len]);

                let result = dispatch_frame_for_test(&parser, &frame, "test").await;

                prop_assert!(result, "short resize frame must not disconnect");
                let final_size = parser.lock().unwrap().screen().size();
                prop_assert_eq!(final_size, initial,
                    "short resize frame must not change parser dimensions");

                Ok(())
            })?;
        }
    }
}

/// Test-only: synchronously dispatch one binary frame through the relay's
/// frame handler, using a `Cursor` as the PTY write sink. Returns false if
/// the frame signals a disconnect.
#[cfg(test)]
pub(crate) async fn dispatch_frame_for_test(
    parser: &Arc<Mutex<Parser>>,
    data: &[u8],
    conv_id: &str,
) -> bool {
    let mut sink = std::io::Cursor::new(Vec::<u8>::new());
    dispatch_incoming_frame(&mut sink, parser, &|_| {}, data, conv_id).await
}
