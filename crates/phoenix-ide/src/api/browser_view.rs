//! Live browser view WebSocket endpoint (REQ-BT-018).
//!
//! `GET /api/conversations/:id/browser-view` upgrades to a binary WebSocket
//! that streams CDP screencast frames. The wire format is:
//!
//! ```text
//!   byte 0 = 0x00 → JPEG frame: [0x00][u32be jpeg_length][jpeg bytes...]
//!   byte 0 = 0x01 → URL change: [0x01][utf-8 url string]
//!   byte 0 = 0x02 → status:     [0x02][utf-8 status string]
//!                                  Out-of-band human-readable status:
//!                                  "no-session", "started", "error: ..."
//! ```
//!
//! View-only by design (REQ-BT-018-NG-INPUT): the client never sends data
//! frames back. We still drain client → server messages so close frames
//! are observed, but any non-Close payload is ignored. Future input support
//! would add a fourth byte tag.

use super::AppState;
use crate::tools::browser::screencast::ScreencastEvent;
use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Path, State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// Wire-frame tag bytes. Mirror these in [`ui/src/components/BrowserViewPanel.tsx`].
const TAG_FRAME: u8 = 0x00;
const TAG_URL: u8 = 0x01;
const TAG_STATUS: u8 = 0x02;

/// Axum handler: `GET /api/conversations/:id/browser-view`.
///
/// Auth is handled by the surrounding middleware before this is reached.
pub async fn browser_view_ws_handler(
    ws: WebSocketUpgrade,
    Path(conversation_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, conversation_id, state))
}

async fn handle_socket(socket: WebSocket, conversation_id: String, state: AppState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Don't lazy-create a browser session here. The contract is: a viewer
    // attaches to whatever session the agent already has. If none exists,
    // tell the client and close — they can reconnect later when the agent
    // does something browser-related.
    let manager = state.runtime.browser_sessions().clone();
    let Some(session_arc) = session_if_exists(&manager, &conversation_id).await else {
        tracing::debug!(
            conv_id = %conversation_id,
            "browser-view: no session yet; closing with status"
        );
        let _ = ws_sender
            .send(Message::Binary(status_frame("no-session")))
            .await;
        let _ = ws_sender.send(Message::Close(None)).await;
        return;
    };

    // Attach a viewer. The returned Arc keeps the broker alive for the
    // duration of this connection; when this function returns, it drops
    // and (if we were the last viewer) the screencast stops.
    let attach_result = {
        let session = session_arc.read().await;
        session.attach_viewer().await
    };
    let (broker, mut rx, initial_url) = match attach_result {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                conv_id = %conversation_id,
                error = %e,
                "browser-view: attach_viewer failed"
            );
            let _ = ws_sender
                .send(Message::Binary(status_frame(&format!("error: {e}"))))
                .await;
            let _ = ws_sender.send(Message::Close(None)).await;
            return;
        }
    };

    tracing::info!(
        conv_id = %conversation_id,
        viewer_count = broker.viewer_count(),
        "browser-view: viewer attached"
    );

    // Send a 'started' status frame so the client knows the screencast is
    // live (separate from the no-session and error cases).
    if ws_sender
        .send(Message::Binary(status_frame("started")))
        .await
        .is_err()
    {
        return;
    }

    // Catch up the new viewer with the URL we already know about.
    if let Some(url) = initial_url {
        if ws_sender
            .send(Message::Binary(url_frame(&url)))
            .await
            .is_err()
        {
            return;
        }
    }

    // Main pump. We exit as soon as either side closes:
    //   - WS closes (browser tab gone): drop broker arc, screencast may stop.
    //   - Broker channel closes (broker dropped because page died): we're
    //     done; tell the client and close.
    loop {
        tokio::select! {
            client_msg = ws_receiver.next() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::debug!(conv_id = %conversation_id, "browser-view: client closed");
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::debug!(conv_id = %conversation_id, error = %e, "browser-view: ws error");
                        break;
                    }
                    // Anything else is silently ignored — view-only.
                    Some(Ok(_)) => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(ScreencastEvent::Frame { jpeg }) => {
                        if ws_sender.send(Message::Binary(frame_payload(&jpeg))).await.is_err() {
                            break;
                        }
                    }
                    Ok(ScreencastEvent::Url(url)) => {
                        if ws_sender.send(Message::Binary(url_frame(&url))).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Slow viewer fell behind; drop the missed frames
                        // and keep streaming. Live screencast — stale frames
                        // are worse than skipped ones.
                        tracing::debug!(conv_id = %conversation_id, dropped = n, "browser-view: lagged");
                    }
                    Err(RecvError::Closed) => {
                        tracing::debug!(conv_id = %conversation_id, "browser-view: broker closed");
                        let _ = ws_sender.send(Message::Binary(status_frame("ended"))).await;
                        let _ = ws_sender.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
        }
    }

    // Explicit drop — the lifetime story matters here. As soon as `broker`
    // drops, if no other viewer is still attached, `ScreencastBroker::drop`
    // fires `Page.stopScreencast`. Putting the drop at the end of the
    // function makes the read order match the runtime order.
    drop(broker);
}

async fn session_if_exists(
    manager: &Arc<crate::tools::browser::BrowserSessionManager>,
    conversation_id: &str,
) -> Option<Arc<tokio::sync::RwLock<crate::tools::browser::session::BrowserSession>>> {
    manager.get_existing(conversation_id).await
}

fn frame_payload(jpeg: &[u8]) -> Vec<u8> {
    let len: u32 = jpeg.len().try_into().unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(1 + 4 + jpeg.len());
    out.push(TAG_FRAME);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(jpeg);
    out
}

fn url_frame(url: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + url.len());
    out.push(TAG_URL);
    out.extend_from_slice(url.as_bytes());
    out
}

fn status_frame(status: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + status.len());
    out.push(TAG_STATUS);
    out.extend_from_slice(status.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_payload_layout() {
        let jpeg = vec![0xff, 0xd8, 0xff, 0xe0];
        let p = frame_payload(&jpeg);
        assert_eq!(p[0], TAG_FRAME);
        assert_eq!(&p[1..5], &4u32.to_be_bytes());
        assert_eq!(&p[5..], &jpeg[..]);
    }

    #[test]
    fn url_frame_layout() {
        let p = url_frame("http://example.com");
        assert_eq!(p[0], TAG_URL);
        assert_eq!(&p[1..], b"http://example.com");
    }

    #[test]
    fn status_frame_layout() {
        let p = status_frame("started");
        assert_eq!(p[0], TAG_STATUS);
        assert_eq!(&p[1..], b"started");
    }
}
