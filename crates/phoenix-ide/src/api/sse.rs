//! Server-Sent Events support
//!
//! REQ-API-005: Real-time Streaming
//!
//! The serialization boundary lives in [`super::wire::SseWireEvent`]: every
//! broadcast [`SseEvent`] is asynchronously projected into the typed wire enum and
//! then through `serde_json::to_string`. See `super::wire` for the rationale
//! and for the ts-rs-driven TS codegen that downstream clients consume.

use super::wire::SseWireEvent;
use crate::runtime::SseEvent;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

const CONVERSATION_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
pub(crate) enum SseStreamAudience {
    Authenticated,
    SharedTranscript,
}

impl SseStreamAudience {
    fn allows(self, event: &SseEvent) -> bool {
        match self {
            Self::Authenticated => true,
            Self::SharedTranscript => match event {
                SseEvent::SteerMessageQueued { .. } | SseEvent::SteerMessageCancelled { .. } => {
                    false
                }
                SseEvent::Init { .. }
                | SseEvent::Message { .. }
                | SseEvent::MessageUpdated { .. }
                | SseEvent::StateChange { .. }
                | SseEvent::Token { .. }
                | SseEvent::LlmFirstByte { .. }
                | SseEvent::LlmAttempt { .. }
                | SseEvent::AgentDone { .. }
                | SseEvent::ConversationBecameTerminal { .. }
                | SseEvent::ConversationUpdate { .. }
                | SseEvent::Error { .. }
                | SseEvent::ConversationHardDeleted { .. }
                | SseEvent::BrowserSessionState { .. }
                | SseEvent::BashToolProgress { .. }
                | SseEvent::RateLimitSnapshot { .. }
                | SseEvent::WorkScopeUpdate { .. } => true,
            },
        }
    }

    fn prepare(self, mut event: SseEvent) -> Option<SseEvent> {
        if !self.allows(&event) {
            return None;
        }
        if matches!(self, Self::SharedTranscript) {
            if let SseEvent::Init {
                steering_messages,
                pending_events,
                opening,
                ..
            } = &mut event
            {
                steering_messages.clear();
                if matches!(
                    opening,
                    Some(super::types::ContinuationOpeningView::Pending { .. })
                ) {
                    *opening = None;
                }
                *pending_events = std::mem::take(pending_events)
                    .into_iter()
                    .filter_map(|pending| self.prepare(pending))
                    .collect();
            }
        }
        if matches!(self, Self::SharedTranscript) {
            if let SseEvent::MessageUpdated { opening, .. } | SseEvent::Message { opening, .. } =
                &mut event
            {
                if matches!(
                    opening,
                    Some(super::types::ContinuationOpeningView::Pending { .. })
                ) {
                    *opening = None;
                }
            }
        }
        Some(event)
    }
}

async fn enrich_update_opening(
    db: &crate::db::Database,
    event: &mut SseEvent,
) -> Result<(), crate::db::DbError> {
    if let SseEvent::MessageUpdated {
        message_id,
        transcript_generation,
        content: Some(content),
        opening,
        display_data,
        duration_ms,
        ..
    } = event
    {
        use phoenix_db::ContinuationOpeningUpdateRead;
        *opening = match db
            .read_message_continuation_opening_at_update(
                message_id,
                *transcript_generation,
                content,
            )
            .await?
        {
            None => None,
            Some(ContinuationOpeningUpdateRead::Current(read)) => Some(read.into()),
            Some(ContinuationOpeningUpdateRead::Superseded {
                content: committed_content,
                transcript_generation: committed_generation,
                display_data: committed_display,
                opening: read,
            }) => {
                tracing::debug!(%message_id, ?transcript_generation, committed_generation,
                    "reconciling superseded SSE opening update from committed state");
                *content = committed_content;
                *transcript_generation = Some(committed_generation);
                *duration_ms = committed_display
                    .as_ref()
                    .and_then(|display| display.get("duration_ms"))
                    .and_then(serde_json::Value::as_u64);
                *display_data = committed_display;
                Some(read.into())
            }
        };
    }
    Ok(())
}

async fn prepare_sse_delivery(
    db: &crate::db::Database,
    event: SseEvent,
    audience: SseStreamAudience,
) -> Result<Option<SseEvent>, crate::db::DbError> {
    let Some(mut event) = audience.prepare(event) else {
        return Ok(None);
    };
    if let SseEvent::Init { pending_events, .. } = &mut event {
        for pending in pending_events {
            enrich_update_opening(db, pending).await?;
        }
    } else {
        enrich_update_opening(db, &mut event).await?;
    }
    Ok(audience.prepare(event))
}

/// Short-lived trace for the work required to produce the first usable SSE
/// init item. Unlike the HTTP stream span, this ends when init is serialized
/// and yielded, not when the long-lived `EventSource` disconnects.
pub(crate) struct SseInitTrace {
    started: std::time::Instant,
    span: tracing::Span,
}

impl SseInitTrace {
    pub(crate) fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            span: tracing::info_span!(
                target: "phoenix_ide::otel",
                "conversation.stream.init",
                "stream.broadcaster_reservation_ms" = tracing::field::Empty,
                "stream.transcript_read_ms" = tracing::field::Empty,
                "stream.metadata_hydration_ms" = tracing::field::Empty,
                "stream.pr_cache_lookup_ms" = tracing::field::Empty,
                "stream.serialization_ms" = tracing::field::Empty,
                "stream.time_to_init_ms" = tracing::field::Empty,
                "stream.message_count" = tracing::field::Empty,
                "open.id" = tracing::field::Empty,
            ),
        }
    }

    pub(crate) fn record_ms(&self, field: &'static str, duration: Duration) {
        self.span.record(
            field,
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        );
    }

    pub(crate) fn record_open_id(&self, open_id: uuid::Uuid) {
        self.span
            .record("open.id", tracing::field::display(open_id));
    }

    pub(crate) fn record_message_count(&self, count: usize) {
        self.span.record(
            "stream.message_count",
            u64::try_from(count).unwrap_or(u64::MAX),
        );
    }
}

fn conversation_keep_alive() -> KeepAlive {
    KeepAlive::new()
        .interval(CONVERSATION_KEEPALIVE_INTERVAL)
        .event(Event::default().event("ping").data("ping"))
}

/// Stream `init_event` followed by broadcast events to an SSE client.
///
/// On `BroadcastStreamRecvError::Lagged` — the client fell far enough behind
/// that the `broadcast::channel` overwrote unread entries — this stream ends.
/// The client's `EventSource` observes the close, its `ConnectionMachine`
/// reconnects, and the next `init` event pulls in everything that was in
/// the gap (the server persisted it all to `SQLite` regardless). Silently
/// dropping Lagged — which this function used to do — left the client's
/// state strictly behind truth with no way to notice the gap.
///
/// `conv_id` is threaded through only for the Lagged log line; the stream
/// itself does not consume it. Capacity of the underlying channel lives
/// at `crate::runtime::SSE_BROADCAST_CAPACITY`.
///
/// Sets `X-Accel-Buffering: no` so any HTTP-aware intermediary on the path
/// (nginx, ingress controllers, etc.) flushes events immediately rather than
/// batching them. Without this hint such a proxy may hold `state_change`
/// events long enough to mask client UI as "stuck on stale phase" between
/// transitions. No-op for TCP-level forwarders, harmless either way.
pub fn sse_stream(
    db: crate::db::Database,
    conv_id: String,
    init_event: SseEvent,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
    init_trace: Option<SseInitTrace>,
    audience: SseStreamAudience,
) -> impl IntoResponse {
    let init_db = db.clone();
    let init = futures::stream::once(async move {
        let serialization_started = std::time::Instant::now();
        let event = prepare_sse_delivery(&init_db, init_event, audience)
            .await?
            .expect("init is visible to every SSE audience");
        let event = project_sse_event(&init_db, audience, event).await;
        if let Some(trace) = init_trace {
            trace.record_ms("stream.serialization_ms", serialization_started.elapsed());
            trace.record_ms("stream.time_to_init_ms", trace.started.elapsed());
            drop(trace);
        }
        Ok::<_, crate::db::DbError>(Some(event))
    });

    let broadcasts = BroadcastStream::new(broadcast_rx)
        .take_while(move |result| match result {
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(conv_id = %conv_id, lagged_by = n,
                    "SSE broadcast lagged; closing stream so client reconnects and resyncs");
                false
            }
            _ => true,
        })
        .then(move |result| {
            let db = db.clone();
            async move {
                let event = result.expect("lagged stream is closed before delivery");
                match prepare_sse_delivery(&db, event, audience).await? {
                    Some(event) => Ok(Some(project_sse_event(&db, audience, event).await)),
                    None => Ok(None),
                }
            }
        });

    let combined = init.chain(broadcasts)
        .take_while(|result| {
            if let Err(error) = result {
                tracing::error!(%error, "SSE opening observation mismatch/read failed; closing stream for reconciliation");
                false
            } else { true }
        })
        .filter_map(|result| result.ok().flatten().map(Ok::<_, Infallible>));

    // Typed `ping` event with non-empty data so the browser's EventSource
    // observes it via an explicit listener and the heartbeat watchdog can
    // bump `lastSseEventAt` (specs/working-phase-visibility/ REQ-WPV-004).
    // The previous `.text("ping")` form emitted an SSE comment line which
    // EventSource does NOT surface, leaving the watchdog blind to keep-
    // alives. axum's `Event::data` drops empty-data events so the payload
    // MUST be non-empty; the listener bumps lastSseEventAt and discards
    // the body.
    let sse = Sse::new(combined).keep_alive(conversation_keep_alive());

    let mut headers = HeaderMap::new();
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    (headers, sse)
}

fn aggregate_broadcast_events(
    db: crate::db::Database,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    let events = BroadcastStream::new(broadcast_rx)
        .take_while(|result| match result {
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(
                    lagged_by = n,
                    "aggregate SSE broadcast lagged; closing stream so client reconnects and reconciles"
                );
                false
            }
            _ => true,
        })
        .filter_map(Result::ok);
    futures::StreamExt::then(events, move |event| {
        let db = db.clone();
        async move { Ok(project_sse_event(&db, SseStreamAudience::Authenticated, event).await) }
    })
}

pub(crate) fn aggregate_event_stream(
    db: crate::db::Database,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
) -> impl IntoResponse {
    let sse = Sse::new(aggregate_broadcast_events(db, broadcast_rx))
        .keep_alive(conversation_keep_alive());
    let mut headers = HeaderMap::new();
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    (headers, sse)
}

async fn project_sse_event(
    db: &crate::db::Database,
    audience: SseStreamAudience,
    event: SseEvent,
) -> Event {
    wire_event_to_axum(&project_sse_wire(db, audience, event).await)
}

async fn project_sse_wire(
    db: &crate::db::Database,
    audience: SseStreamAudience,
    event: SseEvent,
) -> SseWireEvent {
    use super::event_presentation::{event_messages, EventPresentations};
    let presentations = match audience {
        SseStreamAudience::Authenticated => {
            EventPresentations::load(db, event_messages(&event)).await
        }
        SseStreamAudience::SharedTranscript => EventPresentations::redacted(),
    };
    SseWireEvent::with_presentations(event, &presentations)
}

#[cfg(test)]
fn sse_event_to_axum(event: SseEvent) -> Event {
    wire_event_to_axum(&event.into())
}

fn wire_event_to_axum(wire: &SseWireEvent) -> Event {
    let event_type = wire.event_type();
    // SseWireEvent derives Serialize over types that themselves derive
    // Serialize (or carry `serde_json::Value`). `to_string` cannot fail
    // at this layer; if it did, we'd want to know loudly.
    let data = serde_json::to_string(wire).expect("SseWireEvent is always serializable");
    Event::default().event(event_type).data(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        ConvMode, Conversation, ErrorKind, Message, MessageContent, MessageType, UsageData,
    };
    use crate::runtime::user_facing_error::UserFacingError;
    use crate::runtime::{ConversationMetadataUpdate, EnrichedConversation};
    use crate::state_machine::state::ConvState;
    use chrono::{TimeZone, Utc};
    use serde_json::{json, Value};

    /// Legacy `json!()` serialization — kept in tests only. The production
    /// path goes through `SseWireEvent`; this function is the gold-standard
    /// reference implementation we compare against for byte-for-byte
    /// parity. Any divergence between this and the typed path is a
    /// regression that would silently break every SSE consumer.
    #[allow(clippy::too_many_lines)]
    fn legacy_sse_event_to_json(event: &SseEvent) -> Value {
        use crate::api::handlers::enrich_message_for_api;

        match event {
            SseEvent::Init {
                sequence_id,
                conversation,
                transcript,
                steering_messages,
                agent_working,
                presentation_mode,
                last_sequence_id,
                stream_incarnation,
                context_window_size,
                project_name,
                pending_anchor_sequence_id,
                pending_events,
                pending_truncated,
                transcript_generation,
                opening,
            } => {
                let transcript_coverage = transcript.coverage();
                let enriched_msgs: Vec<Value> = transcript
                    .messages()
                    .iter()
                    .map(enrich_message_for_api)
                    .collect();
                // Mirror the typed-path conversion: each pending entry is
                // recursively rendered via the legacy JSON producer so this
                // function remains the gold-standard reference for the
                // production wire bytes.
                let pending_json: Vec<Value> = pending_events
                    .iter()
                    .map(legacy_sse_event_to_json)
                    .collect();
                json!({
                    "type": "init",
                    "sequence_id": sequence_id,
                    "conversation": conversation,
                    "transcript_generation": transcript_generation,
                    "transcript_coverage": transcript_coverage,
                    "messages": enriched_msgs,
                    "steering_messages": steering_messages
                        .iter()
                        .map(crate::api::wire::QueuedSteeringMessage::from)
                        .collect::<Vec<_>>(),
                    "agent_working": agent_working,
                    "presentation_mode": presentation_mode,
                    "last_sequence_id": last_sequence_id,
                    "stream_incarnation": stream_incarnation,
                    "context_window_size": context_window_size,
                    "project_name": project_name,
                    "pending_anchor_sequence_id": pending_anchor_sequence_id,
                    "pending_events": pending_json,
                    "pending_truncated": pending_truncated,
                    "opening": opening,
                })
            }
            SseEvent::Message { message, opening } => {
                let sequence_id = message.sequence_id;
                let message_value = enrich_message_for_api(message);
                json!({
                    "type": "message",
                    "sequence_id": sequence_id,
                    "message": message_value,
                    "opening": opening,
                })
            }
            SseEvent::MessageUpdated {
                opening,
                sequence_id,
                message_id,
                transcript_generation,
                display_data,
                content,
                duration_ms,
            } => {
                let mut obj = json!({
                    "type": "message_updated",
                    "opening": opening,
                    "sequence_id": sequence_id,
                    "message_id": message_id,
                    "transcript_generation": transcript_generation,
                    "display_data": display_data,
                    "content": content,
                });
                if let Some(ms) = duration_ms {
                    obj["duration_ms"] = json!(ms);
                }
                obj
            }
            SseEvent::StateChange {
                sequence_id,
                state,
                presentation_mode,
                state_updated_at,
            } => {
                let mut obj = json!({
                    "type": "state_change",
                    "sequence_id": sequence_id,
                    "state": serde_json::to_value(state).unwrap_or(Value::Null),
                    "presentation_mode": presentation_mode,
                    "state_updated_at": state_updated_at,
                });
                if let Some(error_kind) = state.error_kind() {
                    obj["error"] = json!({
                        "kind": error_kind,
                        "can_auto_retry": error_kind.is_generic_auto_retryable(),
                        "can_user_resume": error_kind.is_user_resumable(),
                    });
                }
                obj
            }
            SseEvent::LlmFirstByte {
                sequence_id,
                request_id,
            } => json!({
                "type": "llm_first_byte",
                "sequence_id": sequence_id,
                "request_id": request_id,
            }),
            SseEvent::LlmAttempt {
                sequence_id,
                attempt,
                max_attempts,
                reason,
                backing_off_ms,
                resets_at,
            } => {
                // `resets_at` is omitted from JSON when None (matches the
                // typed wire's `skip_serializing_if = "Option::is_none"`).
                let mut obj = json!({
                    "type": "llm_attempt",
                    "sequence_id": sequence_id,
                    "attempt": attempt,
                    "max_attempts": max_attempts,
                    "reason": reason,
                    "backing_off_ms": backing_off_ms,
                });
                if let Some(ts) = resets_at {
                    obj["resets_at"] = serde_json::to_value(ts).unwrap_or(Value::Null);
                }
                obj
            }
            SseEvent::Token {
                sequence_id,
                text,
                request_id,
            } => json!({
                "type": "token",
                "sequence_id": sequence_id,
                "text": text,
                "request_id": request_id,
            }),
            SseEvent::AgentDone { sequence_id } => json!({
                "type": "agent_done",
                "sequence_id": sequence_id,
            }),
            SseEvent::ConversationBecameTerminal { sequence_id } => json!({
                "type": "conversation_became_terminal",
                "sequence_id": sequence_id,
            }),
            SseEvent::ConversationUpdate {
                sequence_id,
                update,
            } => json!({
                "type": "conversation_update",
                "sequence_id": sequence_id,
                "conversation": update,
            }),
            SseEvent::Error { sequence_id, error } => json!({
                "type": "error",
                "sequence_id": sequence_id,
                "message": error.flat_message(),
                "error": error,
            }),
            SseEvent::ConversationHardDeleted {
                sequence_id,
                conversation_id,
                deleted_conversation_ids,
            } => json!({
                "type": "conversation_hard_deleted",
                "sequence_id": sequence_id,
                "conversation_id": conversation_id,
                "deleted_conversation_ids": deleted_conversation_ids,
            }),
            SseEvent::BrowserSessionState {
                sequence_id,
                active,
            } => json!({
                "type": "browser_session_state",
                "sequence_id": sequence_id,
                "active": active,
            }),
            SseEvent::BashToolProgress {
                sequence_id,
                tool_use_id,
                progress,
            } => json!({
                "type": "bash_tool_progress",
                "sequence_id": sequence_id,
                "tool_use_id": tool_use_id,
                "progress": progress,
            }),
            SseEvent::SteerMessageQueued {
                sequence_id,
                message,
                queue_position,
            } => json!({
                "type": "steer_message_queued",
                "sequence_id": sequence_id,
                "message": crate::api::wire::QueuedSteeringMessage::from(message),
                "queue_position": queue_position,
            }),
            SseEvent::SteerMessageCancelled {
                sequence_id,
                message_id,
            } => json!({
                "type": "steer_message_cancelled",
                "sequence_id": sequence_id,
                "message_id": message_id,
            }),
            SseEvent::RateLimitSnapshot {
                sequence_id,
                snapshot,
            } => json!({
                "type": "rate_limit_snapshot",
                "sequence_id": sequence_id,
                "snapshot": snapshot,
            }),
            SseEvent::WorkScopeUpdate {
                sequence_id,
                inventory,
            } => json!({
                "type": "work_scope_update",
                "sequence_id": sequence_id,
                "inventory": inventory,
            }),
        }
    }

    /// Typed path (production) rendered as a `serde_json::Value` so parity
    /// can be compared against the legacy `json!()` path structurally.
    fn typed_sse_event_to_value(event: &SseEvent) -> Value {
        let wire: SseWireEvent = event.clone().into();
        serde_json::to_value(&wire).expect("SseWireEvent always serializes")
    }

    fn assert_parity(event: &SseEvent) {
        let old = legacy_sse_event_to_json(event);
        let new = typed_sse_event_to_value(event);
        assert_eq!(
            old,
            new,
            "SSE wire parity mismatch between legacy json!() and typed SseWireEvent\n\
             legacy:\n{}\n\
             typed:\n{}",
            serde_json::to_string_pretty(&old).unwrap_or_default(),
            serde_json::to_string_pretty(&new).unwrap_or_default(),
        );
    }

    // ------------------------------------------------------------------
    // Fixture builders
    // ------------------------------------------------------------------

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap()
    }

    fn fixture_conversation() -> Conversation {
        Conversation {
            attached_work_scope_id: Some(
                crate::work_scope::WorkScopeId::parse("test-work").unwrap(),
            ),
            runtime_role: crate::work_scope::RuntimeRole::User,
            id: "conv-1".to_string(),
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::parse(
                    "product-conv-1",
                )
                .unwrap(),
            slug: Some("test-conv".to_string()),
            title: Some("Test Conversation".to_string()),
            cwd: "/tmp/work".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: ts(),
            created_at: ts(),
            updated_at: ts(),
            archived: false,
            transcript_generation: 1,
            model: Some("claude-sonnet-4-5".to_string()),
            effort: None,
            service_tier: phoenix_core::domain::llm_types::ServiceTier::Standard,
            project_id: None,
            conv_mode: ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            },
            desired_base_branch: None,
            message_count: 3,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,

            llm_language: crate::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
        }
    }

    fn fixture_enriched_conversation() -> EnrichedConversation {
        EnrichedConversation {
            inner: crate::runtime::PresentationConversation(fixture_conversation()),
            conv_mode_label: "explore".to_string(),
            branch_name: None,
            worktree_path: None,
            base_branch: None,
            task_title: None,
            shell: Some("/bin/zsh".to_string()),
            home_dir: Some("/home/alice".to_string()),
            seed_parent_slug: None,
            creation_prompt: None,
            creation_error: None,
            parent_conversation_slug: None,
            browser_session_active: false,
            terminal_uses_tmux: false,
            work_scope_key: "conversation:conv-1".to_string(),
            cached_pr: None,
        }
    }

    fn fixture_user_message() -> Message {
        Message {
            origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
            message_id: "msg-user".to_string(),
            conversation_id: "conv-1".to_string(),
            sequence_id: 1,
            message_type: MessageType::User,
            content: MessageContent::user("hello"),
            display_data: None,
            usage_data: None,
            created_at: ts(),
        }
    }

    fn fixture_agent_message_with_bash() -> Message {
        use phoenix_llm::ContentBlock;
        let blocks = vec![
            ContentBlock::Text {
                text: "Running the command".to_string(),
            },
            ContentBlock::ToolUse {
                id: "tool-abc".to_string(),
                name: "bash".to_string(),
                input: json!({"cmd": "cd /tmp && ls"}),
            },
        ];
        Message {
            origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
            message_id: "msg-agent".to_string(),
            conversation_id: "conv-1".to_string(),
            sequence_id: 2,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(blocks),
            display_data: Some(json!({
                "bash": [{ "tool_use_id": "tool-abc", "display": "ls" }]
            })),
            usage_data: Some(UsageData {
                input_tokens: 10,
                output_tokens: 5,
                reasoning_tokens: None,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
            created_at: ts(),
        }
    }

    fn fixture_steer_entry(message_id: &str) -> crate::state_machine::event::SteerEntry {
        crate::state_machine::event::SteerEntry {
            origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
            text: "queued from another client".to_string(),
            llm_text: Some("queued from another client".to_string()),
            images: Vec::new(),
            files: Vec::new(),
            message_id: message_id.to_string(),
            user_agent: Some("test-agent".to_string()),
            skill_invocation: None,
        }
    }

    // ------------------------------------------------------------------
    // Parity tests — one per SseEvent variant
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn captured_notification_shared_init_replay_pending_and_live_never_serialize_goal_facts()
    {
        use crate::coordinator_watch_delivery::captured_notification_fixture::{
            assert_private_absent, captured_notification, GOAL_ID, OBJECTIVE, REASON,
        };
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let (message, source, diagnostic) = captured_notification(&db).await;
        let live_event = SseEvent::Message {
            message: message.clone(),
            opening: None,
        };
        let messages = vec![message.clone()];
        let init = SseEvent::Init {
            sequence_id: 1,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(messages),
            opening: None,
            steering_messages: Vec::new(),
            agent_working: false,
            presentation_mode: "idle".into(),
            last_sequence_id: 1,
            stream_incarnation: "test-stream".into(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id: 0,
            pending_events: vec![live_event.clone()],
            pending_truncated: false,
        };
        for source_removed in [false, true] {
            if source_removed {
                db.delete_conversation(&source).await.unwrap();
            }
            for event in [live_event.clone(), init.clone()] {
                let private =
                    prepare_sse_delivery(&db, event.clone(), SseStreamAudience::Authenticated)
                        .await
                        .unwrap()
                        .unwrap();
                let private =
                    project_sse_wire(&db, SseStreamAudience::Authenticated, private).await;
                let private = serde_json::to_value(private).unwrap();
                for secret in [OBJECTIVE, REASON, GOAL_ID] {
                    assert!(private.to_string().contains(secret));
                }
                let shared = prepare_sse_delivery(&db, event, SseStreamAudience::SharedTranscript)
                    .await
                    .unwrap()
                    .unwrap();
                let shared =
                    project_sse_wire(&db, SseStreamAudience::SharedTranscript, shared).await;
                let wire = serde_json::to_value(shared).unwrap();
                assert_private_absent(&wire);
                let rows = if wire.get("message").is_some() {
                    vec![&wire["message"]]
                } else {
                    vec![&wire["messages"][0], &wire["pending_events"][0]["message"]]
                };
                for row in rows {
                    assert_eq!(row["content"]["text"], diagnostic);
                    assert_eq!(row["event_presentation"]["reason"], "redacted");
                    assert_eq!(
                        row["content"],
                        serde_json::to_value(&message.content).unwrap()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn event_projection_matches_rest_init_pending_live_and_aggregate_and_redacts_share() {
        let (db, messages) = super::super::event_presentation::tests::fixture(1).await;
        let message = messages[0].clone();
        let rest = super::super::event_presentation::enrich_messages(&db, &messages).await;
        let expected = serde_json::to_value(&rest[0]).unwrap();
        let live_event = SseEvent::Message {
            message: message.clone(),
            opening: None,
        };
        let prepared =
            prepare_sse_delivery(&db, live_event.clone(), SseStreamAudience::Authenticated)
                .await
                .unwrap()
                .unwrap();
        let live = project_sse_wire(&db, SseStreamAudience::Authenticated, prepared).await;
        let value = serde_json::to_value(live).unwrap();
        assert_eq!(value["message"], expected);
        assert_eq!(value.get("opening"), Some(&Value::Null));
        let init = SseEvent::Init {
            sequence_id: 1,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(messages),
            opening: None,
            steering_messages: Vec::new(),
            agent_working: false,
            presentation_mode: "idle".into(),
            last_sequence_id: 1,
            stream_incarnation: "test-stream".into(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id: 0,
            pending_events: vec![live_event.clone()],
            pending_truncated: false,
        };
        let projected = project_sse_wire(&db, SseStreamAudience::Authenticated, init.clone()).await;
        let value = serde_json::to_value(projected).unwrap();
        assert_eq!(value["messages"][0], expected);
        assert_eq!(value["pending_events"][0]["message"], expected);
        for transcript in [
            crate::runtime::InitTranscript::Tail(vec![message.clone()]),
            crate::runtime::InitTranscript::Preserve,
        ] {
            let preserve = matches!(&transcript, crate::runtime::InitTranscript::Preserve);
            let mut variant = init.clone();
            if let SseEvent::Init {
                transcript: rows,
                pending_events,
                ..
            } = &mut variant
            {
                *rows = transcript;
                pending_events.push(init.clone());
            }
            let collected = super::super::event_presentation::event_messages(&variant);
            assert_eq!(collected.len(), if preserve { 3 } else { 4 });
            let wire = project_sse_wire(&db, SseStreamAudience::Authenticated, variant).await;
            let value = serde_json::to_value(wire).unwrap();
            assert_eq!(
                value["messages"].as_array().unwrap().len(),
                usize::from(!preserve)
            );
            assert_eq!(value["pending_events"][0]["message"], expected);
            assert_eq!(value["pending_events"][1]["messages"][0], expected);
            assert_eq!(
                value["pending_events"][1]["pending_events"][0]["message"],
                expected
            );
        }
        let prepared = prepare_sse_delivery(&db, init, SseStreamAudience::SharedTranscript)
            .await
            .unwrap()
            .unwrap();
        let shared = project_sse_wire(&db, SseStreamAudience::SharedTranscript, prepared).await;
        let value = serde_json::to_value(shared).unwrap();
        for projected in [
            &value["messages"][0],
            &value["pending_events"][0]["message"],
        ] {
            assert_eq!(projected["content"], expected["content"]);
            assert_eq!(projected["event_presentation"]["reason"], "redacted");
            assert!(projected["event_presentation"]
                .get("display_name")
                .is_none());
        }
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        tx.send(live_event.clone()).unwrap();
        let expected_event =
            project_sse_event(&db, SseStreamAudience::Authenticated, live_event).await;
        let mut stream = Box::pin(aggregate_broadcast_events(db, rx));
        let aggregate_event = stream.next().await.unwrap().unwrap();
        assert_eq!(
            format!("{aggregate_event:?}"),
            format!("{expected_event:?}")
        );
    }

    #[test]
    fn parity_init() {
        let event = SseEvent::Init {
            opening: Some(crate::api::types::ContinuationOpeningView::None),
            sequence_id: 42,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(vec![
                fixture_user_message(),
                fixture_agent_message_with_bash(),
            ]),
            steering_messages: vec![fixture_steer_entry("msg-steer-init")],
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: 42,
            stream_incarnation: "test-stream".to_string(),
            context_window_size: 2048,
            project_name: Some("phoenix".to_string()),
            pending_anchor_sequence_id: 0,
            pending_events: Vec::new(),
            pending_truncated: false,
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(
            typed["steering_messages"][0]["message_id"],
            "msg-steer-init"
        );
        assert_eq!(
            typed["steering_messages"][0]["text"],
            "queued from another client"
        );
        assert!(typed["steering_messages"][0].get("llm_text").is_none());
    }

    /// Init with a populated `ReplayRing` snapshot. Exercises the recursive
    /// `SseWireEvent` serialisation inside `pending_events` and the parity
    /// of `pending_anchor_sequence_id` / `pending_truncated`.
    ///
    /// Every entry's `sequence_id` exceeds the anchor — matches what a real
    /// `ReplayRing` snapshot can ever produce (`sse_wire.allium` invariant
    /// `ReplayRingEntriesAboveAnchor`). A test fixture that violates the
    /// invariant would mask ordering regressions, since the eager-Message
    /// envelope `sequence_id` equals `message.sequence_id` on the wire.
    #[test]
    fn parity_init_with_pending_events() {
        let anchor: i64 = 42;
        // Eager assistant Message reused from another fixture but reseq'd
        // above the anchor — the wire envelope seq is `message.sequence_id`,
        // so the local mutation flows through to the parity comparison.
        let mut eager = fixture_agent_message_with_bash();
        eager.sequence_id = 45;
        let pending = vec![
            SseEvent::Token {
                sequence_id: 43,
                text: "Hel".to_string(),
                request_id: "req-1".to_string(),
            },
            SseEvent::StateChange {
                sequence_id: 44,
                state: ConvState::LlmRequesting { attempt: 1 },
                presentation_mode: "working".to_string(),
                state_updated_at: ts(),
            },
            SseEvent::Message {
                opening: None,
                message: eager,
            },
        ];
        let event = SseEvent::Init {
            opening: Some(crate::api::types::ContinuationOpeningView::None),
            sequence_id: 45,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(vec![fixture_user_message()]),
            steering_messages: Vec::new(),
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: 45,
            stream_incarnation: "test-stream".to_string(),
            context_window_size: 2048,
            project_name: Some("phoenix".to_string()),
            pending_anchor_sequence_id: anchor,
            pending_events: pending,
            pending_truncated: false,
        };
        assert_parity(&event);

        // Belt + braces: assert the typed wire output carries the pending
        // entries with their original `type` discriminators and seqs, and
        // that every entry's seq strictly exceeds the anchor.
        let typed = typed_sse_event_to_value(&event);
        let pending_arr = typed["pending_events"]
            .as_array()
            .expect("pending_events must be an array");
        assert_eq!(pending_arr.len(), 3);
        assert_eq!(pending_arr[0]["type"], "token");
        assert_eq!(pending_arr[0]["sequence_id"], 43);
        assert_eq!(pending_arr[1]["type"], "state_change");
        assert_eq!(pending_arr[1]["sequence_id"], 44);
        assert_eq!(pending_arr[2]["type"], "message");
        assert_eq!(pending_arr[2]["sequence_id"], 45);
        assert_eq!(typed["pending_anchor_sequence_id"], anchor);
        assert_eq!(typed["pending_truncated"], false);
        for entry in pending_arr {
            assert!(
                entry["sequence_id"].as_i64().unwrap() > anchor,
                "every pending entry's seq must exceed the anchor \
                 (sse_wire.allium ReplayRingEntriesAboveAnchor)"
            );
        }
    }

    /// Init with `pending_truncated = true`: per Q3, `pending_events` is
    /// empty by construction (force full resync).
    #[test]
    fn parity_init_truncated() {
        let event = SseEvent::Init {
            opening: Some(crate::api::types::ContinuationOpeningView::None),
            sequence_id: 99,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(vec![fixture_user_message()]),
            steering_messages: Vec::new(),
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: 99,
            stream_incarnation: "test-stream".to_string(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id: 50,
            pending_events: Vec::new(),
            pending_truncated: true,
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["pending_truncated"], true);
        assert!(typed["pending_events"].as_array().unwrap().is_empty());
    }

    #[test]
    fn parity_opening_observation_is_envelope_authority_not_message_content() {
        let opening = super::super::types::ContinuationOpeningView::Accepted {
            edge: super::super::types::ContinuationOpeningEdgeView {
                product_conversation_id: "product".into(), predecessor_transcript_row_id: "parent".into(),
                successor_transcript_row_id: "successor".into(), continuation_message_id: "boundary".into(),
            },
            accepted_message_id: "accepted".into(),
            opening_authority: phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext,
            presentation: super::super::types::ContinuationOpeningPresentationView::Boundary,
        };
        let event = SseEvent::MessageUpdated {
            sequence_id: 5,
            message_id: "accepted".into(),
            display_data: None,
            content: None,
            duration_ms: None,
            transcript_generation: Some(1),
            opening: Some(opening),
        };
        assert_eq!(
            typed_sse_event_to_value(&event),
            legacy_sse_event_to_json(&event)
        );
        assert_eq!(
            typed_sse_event_to_value(&event)["opening"]["status"],
            "accepted"
        );
        assert!(typed_sse_event_to_value(&event)["opening"]
            .get("summary")
            .is_none());
    }

    #[test]
    fn parity_message_user() {
        let event = SseEvent::Message {
            opening: None,
            message: fixture_user_message(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_message_agent_with_bash_display_merge() {
        // This is the key case: agent messages with display_data go through
        // `enrich_message_for_api`, which walks the content blocks and sets
        // a `display` field on matching bash tool_use blocks. The typed
        // path must produce the same merged content.
        let event = SseEvent::Message {
            opening: None,
            message: fixture_agent_message_with_bash(),
        };
        assert_parity(&event);

        // Belt + braces: assert the `display` field is actually present on
        // the merged tool_use block in the typed output.
        let typed = typed_sse_event_to_value(&event);
        let content = &typed["message"]["content"];
        assert!(
            content.is_array(),
            "content must be an array for agent messages"
        );
        let tool_use = content
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b.get("type") == Some(&json!("tool_use")))
            .expect("missing tool_use block");
        assert_eq!(tool_use.get("display"), Some(&json!("ls")));
    }

    #[test]
    fn parity_message_updated_with_display_data() {
        let event = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 7,
            message_id: "msg-abc".to_string(),
            transcript_generation: Some(3),
            display_data: Some(json!({ "type": "subagent_summary", "results": [] })),
            content: None,
            duration_ms: None,
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_message_updated_with_content() {
        use phoenix_llm::ContentBlock;
        let event = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 9,
            message_id: "msg-def".to_string(),
            transcript_generation: Some(4),
            display_data: None,
            content: Some(MessageContent::Agent(vec![ContentBlock::Text {
                text: "updated".to_string(),
            }])),
            duration_ms: None,
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_message_updated_both_nulls() {
        let event = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 11,
            message_id: "msg-xyz".to_string(),
            transcript_generation: Some(5),
            display_data: None,
            content: None,
            duration_ms: None,
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_message_updated_with_duration_ms() {
        // Typed `duration_ms` must appear in the serialized output.
        let event = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 12,
            message_id: "msg-tool-result".to_string(),
            transcript_generation: Some(6),
            display_data: None,
            content: None,
            duration_ms: Some(1234),
        };
        assert_parity(&event);
        // Belt + braces: assert the field is actually present in the typed output.
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(
            typed.get("duration_ms"),
            Some(&json!(1234)),
            "duration_ms must be present on the wire"
        );
        assert_eq!(
            typed.get("transcript_generation"),
            Some(&json!(6)),
            "transcript_generation must be present on the wire"
        );
    }

    #[test]
    fn parity_state_change() {
        let event = SseEvent::StateChange {
            sequence_id: 13,
            state: ConvState::Idle,
            presentation_mode: "idle".to_string(),
            state_updated_at: ts(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_invalid_request_exposes_manual_recovery() {
        let state = serde_json::from_str(
            r#"{"type":"error","message":"Bad request (400): The access_programs parameter is not enabled for this organization.","error_kind":"invalid_request"}"#,
        )
        .unwrap();
        let event = SseEvent::StateChange {
            sequence_id: 13,
            state,
            presentation_mode: "error".to_string(),
            state_updated_at: ts(),
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["error"]["kind"], "invalid_request");
        assert_eq!(typed["error"]["can_auto_retry"], false);
        assert_eq!(typed["error"]["can_user_resume"], true);
    }

    #[test]
    fn parity_terminal_overload_does_not_expose_generic_auto_retry() {
        let event = SseEvent::StateChange {
            sequence_id: 14,
            state: ConvState::Error {
                message: "capacity retry exhausted".to_string(),
                error_kind: ErrorKind::ServerOverloaded,
                resets_at: None,
            },
            presentation_mode: "error".to_string(),
            state_updated_at: ts(),
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["error"]["kind"], "server_overloaded");
        assert_eq!(typed["error"]["can_auto_retry"], false);
        assert_eq!(typed["error"]["can_user_resume"], true);
    }

    #[test]
    fn parity_state_change_llm_requesting() {
        let event = SseEvent::StateChange {
            sequence_id: 14,
            state: ConvState::LlmRequesting { attempt: 1 },
            presentation_mode: "working".to_string(),
            state_updated_at: ts(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_token() {
        let event = SseEvent::Token {
            sequence_id: 15,
            text: "Hel".to_string(),
            request_id: "req-42".to_string(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_llm_first_byte() {
        let event = SseEvent::LlmFirstByte {
            sequence_id: 21,
            request_id: "req-42".to_string(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_llm_attempt_with_resets_at() {
        let event = SseEvent::LlmAttempt {
            sequence_id: 30,
            attempt: 2,
            max_attempts: 3,
            reason: phoenix_llm::LlmAttemptReason::RateLimit,
            backing_off_ms: 2000,
            resets_at: Some(ts()),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_llm_attempt_without_resets_at() {
        // `resets_at: None` MUST be omitted from the JSON (not emitted
        // as `null`) so the wire shape matches `#[serde(skip_serializing_if)]`
        // and the client reads `undefined` rather than null.
        let event = SseEvent::LlmAttempt {
            sequence_id: 31,
            attempt: 1,
            max_attempts: 3,
            reason: phoenix_llm::LlmAttemptReason::Network,
            backing_off_ms: 1000,
            resets_at: None,
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_agent_done() {
        let event = SseEvent::AgentDone { sequence_id: 16 };
        assert_parity(&event);
    }

    #[test]
    fn parity_conversation_became_terminal() {
        let event = SseEvent::ConversationBecameTerminal { sequence_id: 17 };
        assert_parity(&event);
    }

    #[test]
    fn enriched_conversation_omits_internal_product_identity() {
        let value = serde_json::to_value(fixture_enriched_conversation()).unwrap();
        assert!(value.get("product_conversation_id").is_none());
    }

    #[test]
    fn parity_conversation_update() {
        let event = SseEvent::ConversationUpdate {
            sequence_id: 18,
            update: ConversationMetadataUpdate {
                slug: None,
                title: None,
                cwd: Some("/new/cwd".to_string()),
                project_id: None,
                project_name: None,
                updated_at: None,
                branch_name: None,
                worktree_path: None,
                conv_mode_label: Some("work".to_string()),
                base_branch: None,
                task_title: None,
                work_scope_key: None,
                model: None,
                archived: None,
            },
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_error_retryable() {
        let event = SseEvent::Error {
            sequence_id: 19,
            error: UserFacingError::retryable("Rate limited", "Try again shortly."),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_error_internal() {
        let event = SseEvent::Error {
            sequence_id: 20,
            error: UserFacingError::internal(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_conversation_hard_deleted() {
        let event = SseEvent::ConversationHardDeleted {
            sequence_id: 21,
            conversation_id: "product-1".to_string(),
            deleted_conversation_ids: vec!["conv-1".to_string(), "conv-2".to_string()],
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_browser_session_state_active() {
        let event = SseEvent::BrowserSessionState {
            sequence_id: 22,
            active: true,
        };
        assert_parity(&event);
        // Belt + braces: assert the typed wire output carries `active: true`
        // under the `browser_session_state` discriminator.
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["type"], "browser_session_state");
        assert_eq!(typed["active"], true);
        assert_eq!(typed["sequence_id"], 22);
    }

    #[test]
    fn parity_browser_session_state_inactive() {
        let event = SseEvent::BrowserSessionState {
            sequence_id: 23,
            active: false,
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["active"], false);
    }

    #[test]
    fn parity_bash_tool_progress() {
        use phoenix_core::domain::bash_progress::{BashProgressLine, BashToolProgress};

        let event = SseEvent::BashToolProgress {
            sequence_id: 24,
            tool_use_id: "tool-bash".to_string(),
            progress: BashToolProgress {
                handle: "b-1".to_string(),
                start_offset: 3,
                end_offset: 5,
                truncated_before: true,
                lines: vec![BashProgressLine {
                    offset: 4,
                    text: "building".to_string(),
                }],
                partial: Some("still running".to_string()),
            },
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["type"], "bash_tool_progress");
        assert_eq!(typed["tool_use_id"], "tool-bash");
        assert_eq!(typed["progress"]["partial"], "still running");
    }

    #[test]
    fn parity_work_scope_update() {
        use phoenix_core::domain::work_scope_inventory::{
            BashHandleInventory, BashHandleState, WorkScopeInventory,
        };
        let event = SseEvent::WorkScopeUpdate {
            sequence_id: 31,
            inventory: WorkScopeInventory {
                scope_key: "conversation:conv-1".to_string(),
                bash: vec![BashHandleInventory {
                    handle_id: "b-1".to_string(),
                    label: Some("dev".to_string()),
                    cmd: "npm run dev".to_string(),
                    state: BashHandleState::Running,
                    pid: Some(1234),
                    pgid: Some(4321),
                    started_at: ts(),
                    duration_ms: None,
                    exit_code: None,
                    signal_number: None,
                    output_bytes: 42,
                    health: None,
                }],
                tmux: None,
                browser: None,
                health_sampled_at: None,
                health: None,
            },
        };
        assert_parity(&event);
        let typed = typed_sse_event_to_value(&event);
        assert_eq!(typed["type"], "work_scope_update");
        assert_eq!(typed["sequence_id"], 31);
        assert_eq!(typed["inventory"]["scope_key"], "conversation:conv-1");
        assert_eq!(typed["inventory"]["bash"][0]["handle_id"], "b-1");
    }

    #[test]
    fn parity_steer_message_queued() {
        let event = SseEvent::SteerMessageQueued {
            sequence_id: 22,
            message: fixture_steer_entry("msg-steer-1"),
            queue_position: 0,
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_steer_message_cancelled() {
        let event = SseEvent::SteerMessageCancelled {
            sequence_id: 23,
            message_id: "msg-steer-1".to_string(),
        };
        assert_parity(&event);
    }

    #[test]
    fn shared_audience_filters_live_steering_mutations() {
        let queued = SseEvent::SteerMessageQueued {
            sequence_id: 22,
            message: fixture_steer_entry("msg-steer-1"),
            queue_position: 0,
        };
        let cancelled = SseEvent::SteerMessageCancelled {
            sequence_id: 23,
            message_id: "msg-steer-1".to_string(),
        };

        assert!(SseStreamAudience::Authenticated
            .prepare(queued.clone())
            .is_some());
        assert!(SseStreamAudience::SharedTranscript
            .prepare(queued)
            .is_none());
        assert!(SseStreamAudience::SharedTranscript
            .prepare(cancelled)
            .is_none());
        assert!(SseStreamAudience::SharedTranscript
            .prepare(SseEvent::AgentDone { sequence_id: 24 })
            .is_some());
    }

    #[test]
    fn shared_audience_filters_steering_from_init_and_replay() {
        let init = SseEvent::Init {
            opening: Some(crate::api::types::ContinuationOpeningView::None),
            sequence_id: 24,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(Vec::new()),
            steering_messages: vec![fixture_steer_entry("durable-steer")],
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: 24,
            stream_incarnation: "test-stream".to_string(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id: 21,
            pending_events: vec![
                SseEvent::SteerMessageQueued {
                    sequence_id: 22,
                    message: fixture_steer_entry("replayed-steer"),
                    queue_position: 0,
                },
                SseEvent::AgentDone { sequence_id: 24 },
            ],
            pending_truncated: false,
        };

        let filtered = SseStreamAudience::SharedTranscript
            .prepare(init)
            .expect("shared init remains visible");
        let SseEvent::Init {
            steering_messages,
            pending_events,
            ..
        } = filtered
        else {
            panic!("expected init");
        };

        assert!(steering_messages.is_empty());
        assert_eq!(pending_events.len(), 1);
        assert!(matches!(pending_events[0], SseEvent::AgentDone { .. }));
    }

    // ------------------------------------------------------------------
    // Backwards-compat sanity: the axum Event is still constructed with
    // the correct `event:` label for every variant.
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Phase 2 end-to-end: SseBroadcaster.snapshot_pending() → SseEvent::Init
    // → SseWireEvent → JSON. Verifies init carries the in-flight ring
    // contents in seq order with the right anchor — the core promise of
    // `sse_wire.allium` StreamOpened + InitSnapshotMirrorsRing.
    // ------------------------------------------------------------------

    #[test]
    fn init_carries_pending_events_from_broadcaster_ring() {
        use crate::runtime::SseBroadcaster;

        // Fresh broadcaster, no persisted Messages yet — anchor stays at the
        // initial_last_seq we seeded.
        let initial_last_seq: i64 = 7;
        let broadcaster = SseBroadcaster::new(64, initial_last_seq);
        // A live subscriber so the channel send paths in send_seq don't
        // short-circuit (the ring append happens first either way, but
        // matching real handler shape avoids spurious Err returns).
        let _rx = broadcaster.subscribe();

        // Three ephemeral broadcasts — no PersistedMessage between them, so
        // they accumulate in the ring.
        let _ = broadcaster.send_seq(|seq| SseEvent::Token {
            sequence_id: seq,
            text: "Hel".to_string(),
            request_id: "req-init".to_string(),
        });
        let _ = broadcaster.send_seq(|seq| SseEvent::StateChange {
            sequence_id: seq,
            state: ConvState::LlmRequesting { attempt: 1 },
            presentation_mode: "working".to_string(),
            state_updated_at: ts(),
        });
        let _ = broadcaster.send_seq(|seq| SseEvent::Token {
            sequence_id: seq,
            text: "lo".to_string(),
            request_id: "req-init".to_string(),
        });

        let (pending_anchor_sequence_id, pending_truncated, highest_pending_seq, pending_events) =
            broadcaster.snapshot_pending();

        // Acceptance: anchor equals initial_last_seq (no persisted Message
        // has bumped it).
        assert_eq!(pending_anchor_sequence_id, initial_last_seq);
        assert!(!pending_truncated);
        assert_eq!(pending_events.len(), 3);
        // highest_pending_seq tracks the last entry's seq.
        assert_eq!(highest_pending_seq, initial_last_seq + 3);

        // Matches the production handler: init_seq is bounded by the
        // snapshot's highest seq, not the live broadcaster counter, so
        // any post-snapshot broadcast still passes the client's
        // `applyIfNewer` guard on its live delivery.
        let init_seq = std::cmp::max(initial_last_seq, highest_pending_seq);
        let init = SseEvent::Init {
            opening: Some(crate::api::types::ContinuationOpeningView::None),
            sequence_id: init_seq,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(Vec::new()),
            steering_messages: Vec::new(),
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: init_seq,
            stream_incarnation: "test-stream".to_string(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id,
            pending_events,
            pending_truncated,
        };

        // Parity holds end-to-end (legacy json! and typed path agree).
        assert_parity(&init);

        let typed = typed_sse_event_to_value(&init);
        assert_eq!(typed["type"], "init");
        assert_eq!(typed["pending_anchor_sequence_id"], initial_last_seq);
        assert_eq!(typed["pending_truncated"], false);

        let arr = typed["pending_events"]
            .as_array()
            .expect("pending_events is an array");
        assert_eq!(arr.len(), 3);

        // Entries appear in strictly increasing sequence_id order — every
        // entry above the anchor.
        let seqs: Vec<i64> = arr
            .iter()
            .map(|e| e["sequence_id"].as_i64().expect("seq present"))
            .collect();
        assert_eq!(
            seqs,
            vec![
                initial_last_seq + 1,
                initial_last_seq + 2,
                initial_last_seq + 3
            ]
        );
        for s in &seqs {
            assert!(
                *s > pending_anchor_sequence_id,
                "every pending seq must exceed the anchor"
            );
        }

        // Event-type discriminators survive the round-trip.
        assert_eq!(arr[0]["type"], "token");
        assert_eq!(arr[0]["text"], "Hel");
        assert_eq!(arr[1]["type"], "state_change");
        assert_eq!(arr[2]["type"], "token");
        assert_eq!(arr[2]["text"], "lo");
    }

    /// After a persisted Message broadcast, the ring resets and a fresh
    /// init carries empty `pending_events` with the anchor advanced.
    #[test]
    fn init_pending_is_empty_after_persisted_message() {
        use crate::runtime::SseBroadcaster;

        let broadcaster = SseBroadcaster::new(64, 0);
        let _rx = broadcaster.subscribe();

        // Some ephemeral activity.
        let _ = broadcaster.send_seq(|seq| SseEvent::Token {
            sequence_id: seq,
            text: "x".to_string(),
            request_id: "r".to_string(),
        });
        // A persisted Message reaches the broadcaster — anchor advances,
        // ring clears.
        let msg = {
            use crate::db::{Message, MessageContent, MessageType};
            use chrono::Utc;
            Message {
                origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
                message_id: "m1".to_string(),
                conversation_id: "c".to_string(),
                sequence_id: 5,
                message_type: MessageType::Agent,
                content: MessageContent::agent(vec![phoenix_llm::ContentBlock::text("hi")]),
                display_data: None,
                usage_data: None,
                created_at: Utc::now(),
            }
        };
        let _ = broadcaster.send_persisted_message(msg);

        let (anchor, truncated, highest, events) = broadcaster.snapshot_pending();
        assert_eq!(anchor, 5);
        assert!(!truncated);
        assert_eq!(highest, 5, "empty post-reset ring reports highest = anchor");
        assert!(events.is_empty());

        let init_seq = std::cmp::max(0_i64, highest);
        let init = SseEvent::Init {
            opening: Some(crate::api::types::ContinuationOpeningView::None),
            sequence_id: init_seq,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            transcript: crate::runtime::InitTranscript::Complete(Vec::new()),
            steering_messages: Vec::new(),
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: init_seq,
            stream_incarnation: "test-stream".to_string(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id: anchor,
            pending_events: events,
            pending_truncated: truncated,
        };
        let typed = typed_sse_event_to_value(&init);
        assert_eq!(typed["pending_anchor_sequence_id"], 5);
        assert_eq!(typed["pending_truncated"], false);
        assert!(typed["pending_events"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn aggregate_broadcast_lag_terminates_stream() {
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let event = || SseEvent::ConversationHardDeleted {
            sequence_id: 1,
            conversation_id: "aggregate".to_string(),
            deleted_conversation_ids: Vec::new(),
        };
        tx.send(event()).unwrap();
        tx.send(event()).unwrap();

        let mut stream = Box::pin(aggregate_broadcast_events(
            crate::db::Database::open_in_memory().await.unwrap(),
            rx,
        ));
        assert!(stream.next().await.is_none());
    }

    #[test]
    fn conversation_keepalive_is_typed_ping_with_nonempty_data() {
        let keep_alive = conversation_keep_alive();
        let encoded = format!("{keep_alive:?}");

        assert_eq!(CONVERSATION_KEEPALIVE_INTERVAL, Duration::from_secs(15));
        assert!(
            encoded.contains(r"event: ping\ndata: ping\n\n"),
            "expected a client-observable typed ping keepalive: {encoded}"
        );
    }

    fn update_test_init(pending_events: Vec<SseEvent>) -> SseEvent {
        SseEvent::Init {
            opening: Some(super::super::types::ContinuationOpeningView::None),
            sequence_id: 24,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 2,
            transcript: crate::runtime::InitTranscript::Complete(Vec::new()),
            steering_messages: vec![],
            agent_working: false,
            presentation_mode: "idle".into(),
            last_sequence_id: 24,
            stream_incarnation: "test-stream".into(),
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id: 20,
            pending_events,
            pending_truncated: false,
        }
    }

    #[test]
    fn shared_audience_redacts_pending_keys_in_live_and_replayed_observations() {
        let pending = super::super::types::ContinuationOpeningView::Pending {
            edge: super::super::types::ContinuationOpeningEdgeView {
                product_conversation_id: "product".into(),
                predecessor_transcript_row_id: "parent".into(),
                successor_transcript_row_id: "child".into(),
                continuation_message_id: "boundary".into(),
            },
            client_turn_key: "private-unpublished-key".into(),
            opening_authority: phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext,
        };
        let event = SseEvent::MessageUpdated {
            opening: Some(pending.clone()),
            sequence_id: 22,
            message_id: "opening-key".into(),
            transcript_generation: None,
            display_data: None,
            content: None,
            duration_ms: None,
        };
        for mut carrier in [event.clone(), update_test_init(vec![event])] {
            if let SseEvent::Init { opening, .. } = &mut carrier {
                *opening = Some(pending.clone());
            }
            let authenticated: SseWireEvent = SseStreamAudience::Authenticated
                .prepare(carrier.clone())
                .unwrap()
                .into();
            assert!(serde_json::to_string(&authenticated)
                .unwrap()
                .contains("private-unpublished-key"));
            let shared: SseWireEvent = SseStreamAudience::SharedTranscript
                .prepare(carrier)
                .unwrap()
                .into();
            assert!(!serde_json::to_string(&shared)
                .unwrap()
                .contains("private-unpublished-key"));
        }
    }

    fn decode_update_test_chunk(chunk: &str) -> (i64, Value) {
        let payload = chunk
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("event has JSON data");
        let payload: Value = serde_json::from_str(payload).unwrap();
        let id = payload["sequence_id"]
            .as_i64()
            .expect("sequenced event has a payload cursor");
        (id, payload)
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn delayed_update_reconciles_committed_content_without_poisoning_reconnect() {
        use phoenix_core::domain::db_schema::ContinuationContent;
        use phoenix_db::{ContinueOutcome, NewContinuationDispatchIntent};
        use phoenix_workflow::ClientTurnKey;
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("update-root", "update-root", "/tmp", true, None, None)
            .await
            .unwrap();
        let restored = MessageContent::Continuation(ContinuationContent {
            summary: "context".into(),
        });
        db.add_message("boundary", &root.id, &restored, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "context".into(),
            },
        )
        .await
        .unwrap();
        let (outcome, _) = db
            .continue_conversation_with_intent(
                &root.id,
                NewContinuationDispatchIntent::generated_predecessor_context(
                    ClientTurnKey::try_from("opening-key").unwrap(),
                    "context".into(),
                ),
            )
            .await
            .unwrap();
        let ContinueOutcome::Created(successor) = outcome else {
            panic!("expected successor");
        };
        db.add_message("opening-key", &successor.id, &restored, None, None)
            .await
            .unwrap();
        let edited = MessageContent::Continuation(ContinuationContent {
            summary: "edit A".into(),
        });
        let mut delayed = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 22,
            message_id: "opening-key".into(),
            transcript_generation: Some(1),
            display_data: Some(json!({"stale-display": true})),
            content: Some(edited.clone()),
            duration_ms: None,
        };
        let ordinary = MessageContent::Continuation(ContinuationContent {
            summary: "owed ordinary edit".into(),
        });
        db.add_message("ordinary-key", &successor.id, &ordinary, None, None)
            .await
            .unwrap();
        for (generation, content) in [(1, &edited), (3, &restored)] {
            let mut tx = db.pool().begin().await.unwrap();
            sqlx::query("UPDATE messages SET content = ?1 WHERE message_id = 'opening-key'")
                .bind(serde_json::to_string(&content.to_stored_json()).unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("UPDATE conversations SET transcript_generation = ?1 WHERE id = ?2")
                .bind(generation)
                .bind(&successor.id)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            if generation == 1 {
                let committed = db.get_message_by_id("opening-key").await.unwrap();
                let committed_generation = db
                    .get_conversation(&successor.id)
                    .await
                    .unwrap()
                    .transcript_generation;
                if let SseEvent::MessageUpdated {
                    content,
                    transcript_generation,
                    ..
                } = &mut delayed
                {
                    *content = Some(committed.content);
                    *transcript_generation = Some(committed_generation);
                }
            }
        }
        for audience in [
            SseStreamAudience::Authenticated,
            SseStreamAudience::SharedTranscript,
        ] {
            let broadcaster = crate::runtime::SseBroadcaster::new(16, 20);
            let live_rx = broadcaster.subscribe();
            let live_cursor = broadcaster.current_seq();
            broadcaster
                .send_seq(|sequence_id| {
                    let mut event = delayed.clone();
                    if let SseEvent::MessageUpdated {
                        sequence_id: seq, ..
                    } = &mut event
                    {
                        *seq = sequence_id;
                    }
                    event
                })
                .unwrap();
            broadcaster
                .send_seq(|sequence_id| SseEvent::MessageUpdated {
                    opening: None,
                    sequence_id,
                    message_id: "ordinary-key".into(),
                    transcript_generation: Some(2),
                    display_data: Some(json!({"owed": true})),
                    content: Some(ordinary.clone()),
                    duration_ms: None,
                })
                .unwrap();
            broadcaster
                .send_seq(|sequence_id| SseEvent::Token {
                    sequence_id,
                    text: "owed-token".into(),
                    request_id: "update-test".into(),
                })
                .unwrap();

            // Use the same durable read/fence as the init handler, not an empty fixture snapshot.
            let conversation = db.get_conversation(&successor.id).await.unwrap();
            let messages = db.get_messages(&successor.id).await.unwrap();
            let opening = db
                .read_continuation_opening_at_snapshot(
                    &successor.id,
                    conversation.transcript_generation,
                    messages.iter().map(|message| message.sequence_id).max(),
                )
                .await
                .unwrap();
            let make_init = |cursor, pending_events| {
                let mut init = update_test_init(pending_events);
                if let SseEvent::Init {
                    conversation: view,
                    transcript,
                    opening: observation,
                    transcript_generation,
                    sequence_id,
                    last_sequence_id,
                    stream_incarnation,
                    ..
                } = &mut init
                {
                    view.inner = crate::runtime::PresentationConversation(conversation.clone());
                    *transcript = crate::runtime::InitTranscript::Complete(messages.clone());
                    *observation = Some(opening.clone().into());
                    *transcript_generation = conversation.transcript_generation;
                    *sequence_id = cursor;
                    *last_sequence_id = cursor;
                    *stream_incarnation = broadcaster.stream_incarnation().into();
                }
                init
            };
            let response = sse_stream(
                db.clone(),
                successor.id.clone(),
                make_init(live_cursor, vec![]),
                live_rx,
                None,
                audience,
            )
            .into_response();
            let mut body = response.into_body().into_data_stream();
            let mut applied_cursor = live_cursor;
            for (expected, expected_cursor) in [
                ("event: init", 20),
                ("restored-opening", 21),
                ("owed ordinary edit", 22),
                ("owed-token", 23),
            ] {
                let chunk = tokio::time::timeout(Duration::from_secs(1), body.next())
                    .await
                    .expect("stream makes progress")
                    .expect("stream must not close")
                    .unwrap();
                let chunk = String::from_utf8(chunk.to_vec()).unwrap();
                let (cursor, payload) = decode_update_test_chunk(&chunk);
                assert_eq!(cursor, expected_cursor);
                if expected == "event: init" {
                    assert!(chunk.contains(expected), "{chunk}");
                    assert_eq!(payload["last_sequence_id"], applied_cursor);
                } else {
                    assert_eq!(cursor, applied_cursor + 1, "event must be applicable");
                    applied_cursor = cursor;
                    if expected != "restored-opening" {
                        assert!(chunk.contains(expected), "{chunk}");
                    }
                }
                assert!(!chunk.contains("stale-display"));
                assert!(
                    !chunk.contains("edit A"),
                    "old content must not borrow current authority"
                );
                if expected == "restored-opening" {
                    assert!(chunk.contains("boundary"));
                    assert!(chunk.contains("context"));
                    assert_eq!(payload["transcript_generation"], 3);
                }
            }
            drop(body);
            // The ring still holds the old event. Two reconnects must both reach init and live traffic.
            for reconnect in 0..2 {
                let rx = broadcaster.subscribe();
                let (anchor, truncated, highest, pending) = broadcaster.snapshot_pending();
                assert!(!truncated);
                assert_eq!(
                    pending.len(),
                    3 + reconnect,
                    "no owed event is removed from the shared ring"
                );
                let mut init = make_init(highest, pending);
                if let SseEvent::Init {
                    pending_anchor_sequence_id,
                    sequence_id,
                    last_sequence_id,
                    ..
                } = &mut init
                {
                    *pending_anchor_sequence_id = anchor;
                    *sequence_id = highest;
                    *last_sequence_id = highest;
                }
                let response =
                    sse_stream(db.clone(), successor.id.clone(), init, rx, None, audience)
                        .into_response();
                let mut body = response.into_body().into_data_stream();
                let chunk = tokio::time::timeout(Duration::from_secs(1), body.next())
                    .await
                    .unwrap()
                    .expect("reconnect must deliver current init")
                    .unwrap();
                let chunk = String::from_utf8(chunk.to_vec()).unwrap();
                assert!(chunk.contains("event: init"));
                let (cursor, payload) = decode_update_test_chunk(&chunk);
                assert_eq!(cursor, highest);
                assert_eq!(payload["last_sequence_id"], highest);
                let mut replay_cursor = anchor;
                for event in payload["pending_events"].as_array().unwrap() {
                    let seq = event["sequence_id"].as_i64().unwrap();
                    assert_eq!(seq, replay_cursor + 1, "replay event must be applicable");
                    replay_cursor = seq;
                }
                assert_eq!(replay_cursor, highest);
                assert_eq!(payload["pending_events"][0]["transcript_generation"], 3);
                assert_eq!(payload["pending_events"][1]["transcript_generation"], 2);
                assert_eq!(
                    payload["pending_events"][1]["content"],
                    ordinary.to_stored_json()
                );
                assert!(chunk.contains("context") && chunk.contains("boundary"));
                assert!(chunk.contains("owed-token"));
                assert!(!chunk.contains("stale-display"));
                assert!(!chunk.contains("edit A"));
                broadcaster
                    .send_seq(|sequence_id| SseEvent::Token {
                        sequence_id,
                        text: "still-live".into(),
                        request_id: "update-test".into(),
                    })
                    .unwrap();
                let chunk = tokio::time::timeout(Duration::from_secs(1), body.next())
                    .await
                    .unwrap()
                    .expect("reconnected stream remains open")
                    .unwrap();
                let chunk = String::from_utf8(chunk.to_vec()).unwrap();
                let (cursor, payload) = decode_update_test_chunk(&chunk);
                assert_eq!(
                    cursor,
                    highest + 1,
                    "post-reconnect token must be applicable"
                );
                assert_eq!(payload["text"], "still-live");
            }
            for generation in [None, Some(3), Some(4)] {
                let mut wrong = delayed.clone();
                if let SseEvent::MessageUpdated {
                    transcript_generation,
                    ..
                } = &mut wrong
                {
                    *transcript_generation = generation;
                }
                assert!(matches!(
                    prepare_sse_delivery(&db, wrong.clone(), audience).await,
                    Err(crate::db::DbError::TranscriptSnapshotChanged)
                ));
                let (_tx, rx) = tokio::sync::broadcast::channel(8);
                let response = sse_stream(
                    db.clone(),
                    successor.id.clone(),
                    make_init(broadcaster.current_seq(), vec![wrong]),
                    rx,
                    None,
                    audience,
                )
                .into_response();
                let bytes = tokio::time::timeout(
                    Duration::from_secs(1),
                    axum::body::to_bytes(response.into_body(), usize::MAX),
                )
                .await
                .expect("invalid evidence still closes instead of skipping it")
                .unwrap();
                assert!(bytes.is_empty());
            }
        }
        let latest = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 23,
            message_id: "opening-key".into(),
            transcript_generation: Some(3),
            display_data: None,
            content: Some(restored),
            duration_ms: None,
        };
        let event = prepare_sse_delivery(&db, latest, SseStreamAudience::Authenticated)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            SseEvent::MessageUpdated {
                opening: Some(super::super::types::ContinuationOpeningView::Accepted {
                    presentation:
                        super::super::types::ContinuationOpeningPresentationView::Boundary,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn axum_event_label_for_message_updated() {
        // Regression on the label the client registers via
        // `addEventListener`.
        let event = SseEvent::MessageUpdated {
            opening: None,
            sequence_id: 42,
            message_id: "msg-abc".to_string(),
            transcript_generation: Some(9),
            display_data: Some(json!({ "type": "subagent_summary", "results": [] })),
            content: None,
            duration_ms: None,
        };
        let axum_event = sse_event_to_axum(event);
        let dbg = format!("{axum_event:?}");
        assert!(
            dbg.contains("message_updated"),
            "expected event label: {dbg}"
        );
        assert!(dbg.contains("msg-abc"), "expected id in payload: {dbg}");
    }
}
