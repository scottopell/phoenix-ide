//! Live browser view (REQ-BT-018): per-conversation CDP screencast broker.
//!
//! The agent's `BrowserSession` is single-instance-per-conversation and
//! headless. This module relays its visual output to N WebSocket viewers
//! over CDP `Page.startScreencast` so the user can watch what the agent is
//! doing in real time.
//!
//! # View-only by design
//!
//! There is intentionally no input path back into the page from the panel.
//! Click/keyboard input would create an arbitration problem with the agent's
//! tool-driven activity (REQ-BT-008/009/016) that this MVP deliberately
//! avoids. See spec for the locked-in non-goal.
//!
//! # Lifetime
//!
//! `Page.startScreencast` is expensive on Chrome — it forces a paint per
//! frame. We start the screencast lazily on the first viewer attach and
//! stop it as soon as the last viewer detaches. This is enforced
//! structurally:
//!
//! - Each viewer holds an `Arc<ScreencastBroker>`.
//! - The session holds only a `Weak<ScreencastBroker>` in a slot.
//! - When the last `Arc` drops, the broker's `Drop` impl aborts the listener
//!   task and best-effort fires `Page.stopScreencast`.
//!
//! # Frame protocol on the broadcast channel
//!
//! Each broadcast item is a [`ScreencastEvent`] — already-decoded JPEG bytes
//! (or a URL change). The HTTP/WS layer in `api::browser_view` (in
//! phoenix-ide) converts these to the on-the-wire binary frame format.

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::page::{
    EventFrameNavigated, EventScreencastFrame, ScreencastFrameAckParams, StartScreencastFormat,
    StartScreencastParams, StopScreencastParams,
};
use chromiumoxide::Page;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::session::BrowserError;

/// Default JPEG compression quality (0-100). 70 is the value the
/// chrome-devtools team uses for their built-in screencast: sub-100KB frames
/// for typical pages without visible artefacting on text.
const DEFAULT_JPEG_QUALITY: i64 = 70;

/// Broadcast channel capacity. With everyNthFrame=1 and an interactive page,
/// chromium can emit ~30 frames/sec; if a slow viewer falls more than this
/// many frames behind it sees `RecvError::Lagged` and skips ahead. That's
/// the right behaviour for a live view — never block the source.
const BROADCAST_CAPACITY: usize = 16;

/// Outer liveness ceiling for best-effort teardown while the generation lock
/// serializes stop against replacement startup.
const SCREENCAST_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// One event broadcast to all attached viewers.
#[derive(Debug, Clone)]
pub enum ScreencastEvent {
    /// A new JPEG frame from CDP, already base64-decoded into raw bytes.
    Frame { jpeg: Arc<[u8]> },
    /// The page's main frame navigated to a new URL.
    Url(String),
}

#[derive(Default)]
pub(crate) struct ScreencastLifecycle {
    generation: Mutex<u64>,
}

fn next_generation(current: u64) -> u64 {
    current.wrapping_add(1)
}

fn generation_owns_stop(current: u64, broker: u64) -> bool {
    current == broker
}

#[derive(Debug, PartialEq, Eq)]
enum ScreencastStopOutcome<T> {
    Superseded,
    Completed(T),
    TimedOut,
}

async fn stop_owned_generation<F, T>(
    lifecycle: &ScreencastLifecycle,
    broker_generation: u64,
    timeout: Duration,
    stop: F,
) -> ScreencastStopOutcome<T>
where
    F: std::future::Future<Output = T>,
{
    let generation = lifecycle.generation.lock().await;
    if !generation_owns_stop(*generation, broker_generation) {
        return ScreencastStopOutcome::Superseded;
    }

    match tokio::time::timeout(timeout, stop).await {
        Ok(result) => ScreencastStopOutcome::Completed(result),
        Err(_) => ScreencastStopOutcome::TimedOut,
    }
}

/// Per-`BrowserSession` screencast broker. One source (CDP), many sinks
/// (WebSocket clients). Created on first viewer attach, dropped when the
/// last viewer detaches.
pub struct ScreencastBroker {
    tx: broadcast::Sender<ScreencastEvent>,
    /// Last URL observed on the main frame. Stored so a fresh viewer can be
    /// sent a catch-up [`ScreencastEvent::Url`] before the next navigation.
    last_url: Arc<Mutex<Option<String>>>,
    /// Keeps the page handle alive for the broker's lifetime so `Drop` can
    /// fire `Page.stopScreencast` without depending on the session.
    page: Page,
    /// Aborted on `Drop` so we don't leak a CDP listener after the last
    /// viewer detaches.
    listener_task: JoinHandle<()>,
    lifecycle: Arc<ScreencastLifecycle>,
    generation: u64,
}

impl ScreencastBroker {
    /// Start a screencast on `page` and return a broker ready for subscribers.
    ///
    /// On success the screencast is already running; CDP will begin emitting
    /// `Page.screencastFrame` events that the listener task acks and
    /// broadcasts. On failure the screencast is *not* started — callers can
    /// retry without leaking a half-initialised broker.
    ///
    /// # Errors
    /// Returns [`BrowserError`] when subscribing to CDP events or issuing
    /// `Page.startScreencast` fails.
    pub(crate) async fn start(
        page: Page,
        lifecycle: Arc<ScreencastLifecycle>,
    ) -> Result<Arc<Self>, BrowserError> {
        // Subscribe to events FIRST so we don't miss the first frame between
        // startScreencast returning and the listener actually being ready.
        let frame_events = page
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|e| {
                BrowserError::OperationFailed(format!(
                    "screencast: failed to subscribe to frames: {e}"
                ))
            })?;
        let nav_events = page
            .event_listener::<EventFrameNavigated>()
            .await
            .map_err(|e| {
                BrowserError::OperationFailed(format!(
                    "screencast: failed to subscribe to nav events: {e}"
                ))
            })?;

        // Now actually start the screencast.
        let params = StartScreencastParams {
            format: Some(StartScreencastFormat::Jpeg),
            quality: Some(DEFAULT_JPEG_QUALITY),
            max_width: None,
            max_height: None,
            every_nth_frame: Some(1),
        };
        let mut generation = lifecycle.generation.lock().await;
        page.execute(params).await.map_err(|e| {
            BrowserError::OperationFailed(format!("screencast: Page.startScreencast failed: {e}"))
        })?;
        *generation = next_generation(*generation);
        let broker_generation = *generation;
        drop(generation);

        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let last_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // Seed last_url from whatever the page is already showing so the
        // first subscriber gets the URL even if no navigation happens.
        if let Ok(Some(url)) = page.url().await {
            *last_url.lock().await = Some(url);
        }

        let listener_task = spawn_listener_task(
            page.clone(),
            tx.clone(),
            last_url.clone(),
            frame_events,
            nav_events,
        );

        Ok(Arc::new(Self {
            tx,
            last_url,
            page,
            listener_task,
            lifecycle,
            generation: broker_generation,
        }))
    }

    /// Subscribe a new viewer. Returns a receiver and the URL the page is
    /// currently on (if known) so the viewer can paint its header before
    /// any frame arrives.
    pub async fn subscribe(&self) -> (broadcast::Receiver<ScreencastEvent>, Option<String>) {
        let url = self.last_url.lock().await.clone();
        (self.tx.subscribe(), url)
    }

    /// How many viewers are currently subscribed. Exposed for diagnostics
    /// and tests; not used to drive lifecycle (that is governed by `Arc`
    /// strong-count via `Drop`).
    #[must_use]
    pub fn viewer_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Drop for ScreencastBroker {
    fn drop(&mut self) {
        self.listener_task.abort();
        // Best-effort `Page.stopScreencast`. We can't `.await` in `Drop`,
        // and we don't care about the result — if the page or browser is
        // already gone, the call will just fail and the screencast was
        // implicitly torn down.
        let page = self.page.clone();
        let lifecycle = Arc::clone(&self.lifecycle);
        let broker_generation = self.generation;
        tokio::spawn(async move {
            match stop_owned_generation(
                &lifecycle,
                broker_generation,
                SCREENCAST_STOP_TIMEOUT,
                page.execute(StopScreencastParams::default()),
            )
            .await
            {
                ScreencastStopOutcome::Superseded | ScreencastStopOutcome::Completed(Ok(_)) => {}
                ScreencastStopOutcome::Completed(Err(e)) => {
                    tracing::debug!(error = %e, "Page.stopScreencast failed during broker drop — likely page already closed");
                }
                ScreencastStopOutcome::TimedOut => {
                    tracing::warn!(
                        timeout_secs = SCREENCAST_STOP_TIMEOUT.as_secs(),
                        "Page.stopScreencast timed out during broker drop"
                    );
                }
            }
        });
        tracing::debug!("ScreencastBroker dropped — last viewer detached");
    }
}

/// Spawn the long-running listener task. Owns the CDP event subscriptions
/// and the broadcast sender clone; aborts when the broker drops.
fn spawn_listener_task(
    page: Page,
    tx: broadcast::Sender<ScreencastEvent>,
    last_url: Arc<Mutex<Option<String>>>,
    mut frame_events: chromiumoxide::listeners::EventStream<EventScreencastFrame>,
    mut nav_events: chromiumoxide::listeners::EventStream<EventFrameNavigated>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                next = frame_events.next() => {
                    let Some(frame) = next else {
                        tracing::debug!("screencast: frame stream ended");
                        break;
                    };
                    handle_frame(&page, &tx, &frame).await;
                }
                next = nav_events.next() => {
                    let Some(nav) = next else {
                        // The frame stream is the authoritative source; if
                        // nav events stop but frames keep coming we keep
                        // serving frames.
                        continue;
                    };
                    handle_nav(&tx, &last_url, &nav).await;
                }
            }
        }
    })
}

async fn handle_frame(
    page: &Page,
    tx: &broadcast::Sender<ScreencastEvent>,
    frame: &EventScreencastFrame,
) {
    // CDP delivers the JPEG as base64 in `frame.data`. We decode once here
    // (rather than per-viewer) and broadcast the raw bytes wrapped in an
    // `Arc` so all WS sinks share one allocation.
    let b64: &str = frame.data.as_ref();
    let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(error = %e, "screencast: failed to base64-decode frame; skipping");
            // We must still ack so chrome doesn't stall waiting for us.
            ack_frame(page, frame.session_id).await;
            return;
        }
    };

    // Broadcast first, ack second. Order matters: if the broadcast queue is
    // full and a viewer lags, we still want to keep the source advancing.
    let _ = tx.send(ScreencastEvent::Frame {
        jpeg: Arc::from(decoded.into_boxed_slice()),
    });
    ack_frame(page, frame.session_id).await;
}

async fn ack_frame(page: &Page, session_id: i64) {
    if let Err(e) = page
        .execute(ScreencastFrameAckParams::new(session_id))
        .await
    {
        // If acks consistently fail, chrome will eventually stop emitting
        // frames and the broker will sit idle. Logged at warn so it's visible
        // but doesn't spam: a handful of failed acks during teardown is
        // normal.
        tracing::warn!(error = %e, session_id, "screencast: ScreencastFrameAck failed");
    }
}

async fn handle_nav(
    tx: &broadcast::Sender<ScreencastEvent>,
    last_url: &Arc<Mutex<Option<String>>>,
    nav: &EventFrameNavigated,
) {
    // We only care about main-frame navigations — child iframes navigating
    // are noise for the URL header.
    if nav.frame.parent_id.is_some() {
        return;
    }
    let url = nav.frame.url.clone();
    *last_url.lock().await = Some(url.clone());
    let _ = tx.send(ScreencastEvent::Url(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_event_clone_is_cheap() {
        // ScreencastEvent::Frame uses Arc<[u8]> so cloning a frame doesn't
        // copy the JPEG bytes. This is what makes the N-viewer fan-out
        // affordable. If someone refactors `Frame` to hold a `Vec<u8>` by
        // value this test will start failing meaningfully (slowly).
        let bytes: Arc<[u8]> = Arc::from(vec![0u8; 1_000_000].into_boxed_slice());
        let frame = ScreencastEvent::Frame {
            jpeg: bytes.clone(),
        };
        let clone = frame.clone();
        match (frame, clone) {
            (ScreencastEvent::Frame { jpeg: a }, ScreencastEvent::Frame { jpeg: b }) => {
                assert!(Arc::ptr_eq(&a, &b), "clone must share the same Arc");
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{
        generation_owns_stop, next_generation, stop_owned_generation, ScreencastLifecycle,
        ScreencastStopOutcome,
    };
    use std::sync::Arc;

    #[test]
    fn stale_broker_cannot_stop_replacement_generation() {
        let first = next_generation(0);
        assert!(generation_owns_stop(first, first));
        let replacement = next_generation(first);
        assert!(!generation_owns_stop(replacement, first));
        assert!(generation_owns_stop(replacement, replacement));
    }

    #[tokio::test]
    async fn stalled_owned_stop_releases_generation_lock_at_deadline() {
        let lifecycle = Arc::new(ScreencastLifecycle::default());
        let generation = next_generation(0);
        *lifecycle.generation.lock().await = generation;

        assert_eq!(
            stop_owned_generation(
                &lifecycle,
                generation,
                std::time::Duration::ZERO,
                std::future::pending::<()>(),
            )
            .await,
            ScreencastStopOutcome::TimedOut
        );
        assert!(
            lifecycle.generation.try_lock().is_ok(),
            "bounded stop must release the generation lock"
        );
    }
}
