//! Server-Sent Events support
//!
//! REQ-API-005: Real-time Streaming

use super::handlers::enrich_message_for_api;
use crate::runtime::SseEvent;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde_json::json;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// Convert broadcast stream to SSE stream
pub fn sse_stream(
    init_event: SseEvent,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Create stream that starts with init event then broadcasts
    let init = futures::stream::once(async move { Ok(sse_event_to_axum(init_event)) });

    let broadcasts = BroadcastStream::new(broadcast_rx).filter_map(|result| match result {
        Ok(event) => Some(Ok(sse_event_to_axum(event))),
        Err(_) => None, // Skip lagged messages
    });

    let combined = init.chain(broadcasts);

    Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

fn sse_event_to_axum(event: SseEvent) -> Event {
    let (event_type, data) = match event {
        SseEvent::Init {
            conversation,
            messages,
            agent_working,
            display_state,
            last_sequence_id,
            context_window_size,
            model_context_window,
            breadcrumbs,
            commits_behind,
            commits_ahead,
            project_name,
        } => {
            // Serialize typed data at the SSE boundary
            let enriched_msgs: Vec<serde_json::Value> =
                messages.iter().map(enrich_message_for_api).collect();
            (
                "init",
                json!({
                    "type": "init",
                    "conversation": conversation,
                    "messages": enriched_msgs,
                    "agent_working": agent_working,
                    "display_state": display_state,
                    "last_sequence_id": last_sequence_id,
                    "context_window_size": context_window_size,
                    "model_context_window": model_context_window,
                    "breadcrumbs": breadcrumbs,
                    "commits_behind": commits_behind,
                    "commits_ahead": commits_ahead,
                    "project_name": project_name
                }),
            )
        }
        SseEvent::Message { message } => {
            let message_value = enrich_message_for_api(&message);
            (
                "message",
                json!({
                    "type": "message",
                    "message": message_value
                }),
            )
        }
        SseEvent::StateChange {
            state,
            display_state,
        } => (
            "state_change",
            json!({
                "type": "state_change",
                "state": serde_json::to_value(&state).unwrap_or(serde_json::Value::Null),
                "display_state": display_state
            }),
        ),
        SseEvent::Token { text, request_id } => (
            "token",
            json!({
                "type": "token",
                "text": text,
                "request_id": request_id
            }),
        ),
        SseEvent::AgentDone => (
            "agent_done",
            json!({
                "type": "agent_done"
            }),
        ),
        SseEvent::ConversationUpdate { update } => (
            "conversation_update",
            json!({
                "type": "conversation_update",
                "conversation": update
            }),
        ),
        SseEvent::Error { message } => (
            "error",
            json!({
                "type": "error",
                "message": message
            }),
        ),
    };

    Event::default().event(event_type).data(data.to_string())
}
