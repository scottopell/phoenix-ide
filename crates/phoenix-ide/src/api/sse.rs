//! Server-Sent Events support
//!
//! REQ-API-005: Real-time Streaming
//!
//! The serialization boundary lives in [`super::wire::SseWireEvent`]: every
//! broadcast [`SseEvent`] is `From`-converted into the typed wire enum and
//! then through `serde_json::to_string`. See `super::wire` for the rationale
//! and for the ts-rs-driven TS codegen that downstream clients consume.

use super::types::WakeStatusSnapshot;
use super::wire::SseWireEvent;
use crate::runtime::SseEvent;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::StreamExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventVisibility {
    Authenticated,
    PublicShare,
}

impl EventVisibility {
    fn permits(self, event: &SseEvent) -> bool {
        self == Self::Authenticated || !event.is_private()
    }

    fn filter_init(self, mut event: SseEvent) -> SseEvent {
        if let SseEvent::Init { pending_events, .. } = &mut event {
            pending_events.retain(|pending| self.permits(pending));
        }
        event
    }
}

/// Stream `init_event` followed by broadcast events to an SSE client.
///
/// On `broadcast::error::RecvError::Lagged` — the client fell far enough behind
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
    conv_id: String,
    init_event: SseEvent,
    visibility: EventVisibility,
    wake_status: Option<WakeStatusSnapshot>,
    db: crate::db::Database,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
) -> impl IntoResponse {
    let init_event = visibility.filter_init(init_event);
    let init = futures::stream::iter(
        std::iter::once(Ok::<Event, Infallible>(sse_event_to_axum(init_event)))
            .chain(wake_status.clone().map(wake_status_to_axum).map(Ok)),
    );

    let combined = init.chain(live_event_stream(
        conv_id,
        visibility,
        wake_status,
        db,
        broadcast_rx,
    ));

    // Typed `ping` event with non-empty data so the browser's EventSource
    // observes it via an explicit listener and the heartbeat watchdog can
    // bump `lastSseEventAt` (specs/working-phase-visibility/ REQ-WPV-004).
    // The previous `.text("ping")` form emitted an SSE comment line which
    // EventSource does NOT surface, leaving the watchdog blind to keep-
    // alives. axum's `Event::data` drops empty-data events so the payload
    // MUST be non-empty; the listener bumps lastSseEventAt and discards
    // the body.
    let sse = Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .event(Event::default().event("ping").data("ping")),
    );

    let mut headers = HeaderMap::new();
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    (headers, sse)
}

#[allow(clippy::too_many_lines)]
fn live_event_stream(
    conv_id: String,
    visibility: EventVisibility,
    wake_status: Option<WakeStatusSnapshot>,
    db: crate::db::Database,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    let wake_poll_due_at = tokio::time::Instant::now() + Duration::from_secs(1);
    futures::stream::unfold(
        (
            db,
            conv_id,
            visibility,
            wake_status,
            broadcast_rx,
            wake_poll_due_at,
            0_u8,
            None,
        ),
        |(
            db,
            conversation_id,
            visibility,
            mut previous,
            mut broadcast_rx,
            mut wake_poll_due_at,
            mut broadcast_batch,
            pending_broadcast,
        )| async move {
            if let Some(event) = pending_broadcast {
                return Some((
                    Ok(sse_event_to_axum(event)),
                    (
                        db,
                        conversation_id,
                        visibility,
                        previous,
                        broadcast_rx,
                        wake_poll_due_at,
                        broadcast_batch,
                        None,
                    ),
                ));
            }

            loop {
                if previous.is_some()
                    && broadcast_batch >= 32
                    && tokio::time::Instant::now() >= wake_poll_due_at
                {
                    // Observe closure/lag before starting the DB read. A ready event is
                    // retained so the wake poll cannot lose or delay the broadcast by a
                    // full interval.
                    let pending_broadcast = match broadcast_rx.try_recv() {
                        Ok(event) if !visibility.permits(&event) => None,
                        Ok(event) => Some(event),
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => None,
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return None,
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                            log_lag(&conversation_id, n);
                            return None;
                        }
                    };
                    broadcast_batch = 0;
                    wake_poll_due_at += Duration::from_secs(1);
                    let next = match WakeStatusSnapshot::load(&db, &conversation_id).await {
                        Ok(next) => next,
                        Err(error) => {
                            tracing::warn!(%conversation_id, %error, "failed to refresh wake SSE status");
                            continue;
                        }
                    };
                    if previous.as_ref() != Some(&next) {
                        previous = Some(next.clone());
                        return Some((
                            Ok(wake_status_to_axum(next)),
                            (
                                db,
                                conversation_id,
                                visibility,
                                previous,
                                broadcast_rx,
                                wake_poll_due_at,
                                broadcast_batch,
                                pending_broadcast,
                            ),
                        ));
                    }
                    if let Some(event) = pending_broadcast {
                        return Some((
                            Ok(sse_event_to_axum(event)),
                            (
                                db,
                                conversation_id,
                                visibility,
                                previous,
                                broadcast_rx,
                                wake_poll_due_at,
                                broadcast_batch,
                                None,
                            ),
                        ));
                    }
                }

                if previous.is_none() {
                    return match broadcast_rx.recv().await {
                        Ok(event) if !visibility.permits(&event) => continue,
                        Ok(event) => Some((
                            Ok(sse_event_to_axum(event)),
                            (
                                db,
                                conversation_id,
                                visibility,
                                previous,
                                broadcast_rx,
                                wake_poll_due_at,
                                broadcast_batch,
                                None,
                            ),
                        )),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            log_lag(&conversation_id, n);
                            None
                        }
                    };
                }

                tokio::select! {
                    biased;
                    received = broadcast_rx.recv() => match received {
                        Ok(event) if !visibility.permits(&event) => {}
                        Ok(event) => {
                            broadcast_batch = broadcast_batch.saturating_add(1);
                            return Some((Ok(sse_event_to_axum(event)), (db, conversation_id, visibility, previous, broadcast_rx, wake_poll_due_at, broadcast_batch, None)));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            log_lag(&conversation_id, n);
                            return None;
                        }
                    },
                    () = tokio::time::sleep_until(wake_poll_due_at) => {
                        broadcast_batch = 32;
                    }
                }
            }
        },
    )
}

fn log_lag(conversation_id: &str, lagged_by: u64) {
    tracing::warn!(
        conv_id = %conversation_id,
        lagged_by,
        "SSE broadcast lagged; closing stream so client reconnects and resyncs"
    );
}

fn wake_status_to_axum(snapshot: WakeStatusSnapshot) -> Event {
    wire_event_to_axum(&SseWireEvent::WakeStatusUpdate { snapshot })
}

fn sse_event_to_axum(event: SseEvent) -> Event {
    wire_event_to_axum(&event.into())
}

fn wire_event_to_axum(wire: &SseWireEvent) -> Event {
    let event_type = wire.event_type();
    // SseWireEvent derives Serialize over types that themselves derive
    // Serialize (or carry `serde_json::Value`). `to_string` cannot fail
    // at this layer; if it did, we'd want to know loudly.
    let data = serde_json::to_string(&wire).expect("SseWireEvent is always serializable");
    Event::default().event(event_type).data(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ConvMode, Conversation, Message, MessageContent, MessageType, UsageData};
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
                messages,
                agent_working,
                presentation_mode,
                last_sequence_id,
                context_window_size,
                project_name,
                pending_anchor_sequence_id,
                pending_events,
                pending_truncated,
                transcript_generation,
            } => {
                let enriched_msgs: Vec<Value> =
                    messages.iter().map(enrich_message_for_api).collect();
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
                    "messages": enriched_msgs,
                    "agent_working": agent_working,
                    "presentation_mode": presentation_mode,
                    "last_sequence_id": last_sequence_id,
                    "context_window_size": context_window_size,
                    "project_name": project_name,
                    "pending_anchor_sequence_id": pending_anchor_sequence_id,
                    "pending_events": pending_json,
                    "pending_truncated": pending_truncated,
                })
            }
            SseEvent::WakeContractRegistered {
                sequence_id,
                registration,
            } => json!({
                "type": "wake_contract_registered",
                "sequence_id": sequence_id,
                "registration": registration,
            }),
            SseEvent::Message { message } => {
                let sequence_id = message.sequence_id;
                let message_value = enrich_message_for_api(message);
                json!({
                    "type": "message",
                    "sequence_id": sequence_id,
                    "message": message_value,
                })
            }
            SseEvent::MessageUpdated {
                sequence_id,
                message_id,
                display_data,
                content,
                duration_ms,
            } => {
                let mut obj = json!({
                    "type": "message_updated",
                    "sequence_id": sequence_id,
                    "message_id": message_id,
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
                        "can_auto_retry": error_kind.is_auto_retryable(),
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
            } => json!({
                "type": "conversation_hard_deleted",
                "sequence_id": sequence_id,
                "conversation_id": conversation_id,
            }),
            SseEvent::BrowserSessionState {
                sequence_id,
                active,
            } => json!({
                "type": "browser_session_state",
                "sequence_id": sequence_id,
                "active": active,
            }),
            SseEvent::SteerMessageQueued {
                sequence_id,
                message_id,
                queue_position,
            } => json!({
                "type": "steer_message_queued",
                "sequence_id": sequence_id,
                "message_id": message_id,
                "queue_position": queue_position,
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
            id: "conv-1".to_string(),
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
            chain_name: None,
            llm_language: crate::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
        }
    }

    fn fixture_enriched_conversation() -> EnrichedConversation {
        EnrichedConversation {
            inner: fixture_conversation(),
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
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
            created_at: ts(),
        }
    }

    // ------------------------------------------------------------------
    // Parity tests — one per SseEvent variant
    // ------------------------------------------------------------------

    #[test]
    fn parity_init() {
        let event = SseEvent::Init {
            sequence_id: 42,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            messages: vec![fixture_user_message(), fixture_agent_message_with_bash()],
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: 42,
            context_window_size: 2048,
            project_name: Some("phoenix".to_string()),
            pending_anchor_sequence_id: 0,
            pending_events: Vec::new(),
            pending_truncated: false,
        };
        assert_parity(&event);
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
            SseEvent::Message { message: eager },
        ];
        let event = SseEvent::Init {
            sequence_id: 45,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            messages: vec![fixture_user_message()],
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: 45,
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
            sequence_id: 99,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            messages: vec![fixture_user_message()],
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: 99,
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
    fn parity_wake_contract_registered() {
        assert_parity(&SseEvent::WakeContractRegistered {
            sequence_id: 9,
            registration: phoenix_core::domain::wake_contracts::WakeContractRegistered {
                conversation_id: "conv-1".to_string(),
                contract_id: "wake-1".to_string(),
                handle: phoenix_core::domain::wake_contracts::WakeRegisteredHandle::TmuxWindow {
                    id: "window-1".to_string(),
                },
                expires_at: ts(),
                registering_tool_use_id: Some("tool-1".to_string()),
            },
        });
    }

    #[test]
    fn wake_status_update_is_a_typed_full_snapshot() {
        use crate::api::types::{
            WakeCause, WakeContractHandle, WakeContractStatus, WakeForgottenReason, WakeStatus,
        };

        let snapshot = WakeStatusSnapshot {
            conversation_id: "conv-1".to_string(),
            pending_count: 1,
            soonest_expiry: Some(ts()),
            lifecycle_blocked: true,
            contracts: vec![WakeContractStatus {
                id: "wake-1".to_string(),
                handle: WakeContractHandle::TmuxWindow {
                    id: "window-1".to_string(),
                },
                registered_at: ts(),
                expires_at: ts(),
                status: WakeStatus::Forgotten,
                cause: Some(WakeCause::Forgotten),
                forgotten_reason: Some(WakeForgottenReason::HandleMissing),
            }],
        };
        let wire = SseWireEvent::WakeStatusUpdate { snapshot };
        let value = serde_json::to_value(&wire).expect("wake status serializes");

        assert_eq!(wire.event_type(), "wake_status_update");
        assert_eq!(value["type"], "wake_status_update");
        assert_eq!(value["snapshot"]["conversation_id"], "conv-1");
        assert_eq!(value["snapshot"]["pending_count"], 1);
        assert_eq!(
            value["snapshot"]["contracts"][0]["handle"]["kind"],
            "tmux_window"
        );
        assert_eq!(
            value["snapshot"]["contracts"][0]["handle"]["id"],
            "window-1"
        );
        assert_eq!(value["snapshot"]["contracts"][0]["cause"], "forgotten");
        assert_eq!(
            value["snapshot"]["contracts"][0]["forgotten_reason"],
            "handle_missing"
        );
    }

    async fn live_stream_fixture(
        capacity: usize,
    ) -> (
        crate::db::Database,
        tokio::sync::broadcast::Sender<SseEvent>,
        tokio::sync::broadcast::Receiver<SseEvent>,
        WakeStatusSnapshot,
    ) {
        let db = crate::db::Database::open_in_memory()
            .await
            .expect("database");
        db.create_conversation("live-sse", "live-sse", "/tmp", true, None, None)
            .await
            .expect("conversation");
        let snapshot = WakeStatusSnapshot::load(&db, "live-sse")
            .await
            .expect("initial wake status");
        let (tx, rx) = tokio::sync::broadcast::channel(capacity);
        (db, tx, rx, snapshot)
    }

    #[tokio::test]
    async fn public_share_live_stream_never_polls_or_emits_wake_status() {
        use chrono::Duration as ChronoDuration;
        use phoenix_core::domain::wake_contracts::{
            WakeContract, WakeContractHandle, WakeContractStatus,
        };

        let (db, tx, rx, _snapshot) = live_stream_fixture(4).await;
        let registered_at = Utc::now();
        db.insert_wake_contract(&WakeContract {
            id: "private-contract-id".to_string(),
            current_conversation_id: "live-sse".to_string(),
            registration_work_scope: crate::work_scope::WorkScope::Conversation(
                "live-sse".to_string(),
            ),
            handle: WakeContractHandle::Bash {
                handle_id: "private-handle-id".to_string(),
            },
            registering_tool_use_id: None,
            registered_at,
            expires_at: registered_at + ChronoDuration::seconds(60),
            status: WakeContractStatus::Pending,
            terminal_cause: None,
            forgotten_reason: None,
            terminal_payload: None,
            resolved_at: None,
        })
        .await
        .expect("wake contract");

        tokio::time::pause();
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::PublicShare,
            None,
            db,
            rx,
        );
        futures::pin_mut!(stream);
        tokio::time::advance(Duration::from_secs(5)).await;
        tx.send(SseEvent::Token {
            sequence_id: 1,
            text: "public".to_string(),
            request_id: "request".to_string(),
        })
        .expect("broadcast");

        let rendered = format!("{:?}", stream.next().await.expect("event").unwrap());
        assert!(rendered.contains("public"), "{rendered}");
        assert!(!rendered.contains("wake_status_update"), "{rendered}");
        assert!(!rendered.contains("private-contract-id"), "{rendered}");
        assert!(!rendered.contains("private-handle-id"), "{rendered}");
    }

    fn wake_registration_event() -> SseEvent {
        SseEvent::WakeContractRegistered {
            sequence_id: 7,
            registration: phoenix_core::domain::wake_contracts::WakeContractRegistered {
                conversation_id: "live-sse".to_string(),
                contract_id: "private-contract-id".to_string(),
                handle: phoenix_core::domain::wake_contracts::WakeRegisteredHandle::Bash {
                    id: "private-handle-id".to_string(),
                },
                expires_at: ts(),
                registering_tool_use_id: Some("private-tool-use-id".to_string()),
            },
        }
    }

    fn replay_init(visibility: EventVisibility) -> Value {
        use crate::runtime::SseBroadcaster;

        let broadcaster = SseBroadcaster::new(8, 0);
        let _rx = broadcaster.subscribe();
        let _ = broadcaster.send_seq(|sequence_id| {
            let mut event = wake_registration_event();
            if let SseEvent::WakeContractRegistered {
                sequence_id: event_sequence_id,
                ..
            } = &mut event
            {
                *event_sequence_id = sequence_id;
            }
            event
        });
        let _ = broadcaster.send_seq(|sequence_id| SseEvent::Token {
            sequence_id,
            text: "public-pending-token".to_string(),
            request_id: "public-request-id".to_string(),
        });
        let (pending_anchor_sequence_id, pending_truncated, highest, pending_events) =
            broadcaster.snapshot_pending();
        let init = SseEvent::Init {
            sequence_id: highest,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            messages: Vec::new(),
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: highest,
            context_window_size: 0,
            project_name: None,
            pending_anchor_sequence_id,
            pending_events,
            pending_truncated,
        };
        typed_sse_event_to_value(&visibility.filter_init(init))
    }

    #[test]
    fn public_share_init_filters_private_replay_events() {
        let raw = replay_init(EventVisibility::PublicShare);
        let rendered = serde_json::to_string(&raw).expect("raw shared init");
        let pending = raw["pending_events"].as_array().expect("pending events");

        assert_eq!(pending.len(), 1, "{rendered}");
        assert_eq!(pending[0]["type"], "token");
        assert_eq!(pending[0]["text"], "public-pending-token");
        for private_value in [
            "wake_contract_registered",
            "private-contract-id",
            "private-handle-id",
            "private-tool-use-id",
            &ts().to_rfc3339(),
        ] {
            assert!(!rendered.contains(private_value), "{rendered}");
        }
    }

    #[test]
    fn authenticated_init_retains_private_replay_events() {
        let raw = replay_init(EventVisibility::Authenticated);
        let rendered = serde_json::to_string(&raw).expect("raw authenticated init");
        let pending = raw["pending_events"].as_array().expect("pending events");

        assert_eq!(pending.len(), 2, "{rendered}");
        assert_eq!(pending[0]["type"], "wake_contract_registered");
        assert!(rendered.contains("private-contract-id"), "{rendered}");
        assert!(rendered.contains("private-handle-id"), "{rendered}");
        assert_eq!(pending[1]["type"], "token");
    }

    #[tokio::test]
    async fn authenticated_live_stream_receives_wake_registration_edge() {
        let (db, tx, rx, snapshot) = live_stream_fixture(4).await;
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);
        tx.send(wake_registration_event()).expect("broadcast");
        let rendered = format!("{:?}", stream.next().await.expect("event").unwrap());
        assert!(rendered.contains("wake_contract_registered"), "{rendered}");
        assert!(rendered.contains("private-contract-id"), "{rendered}");
        assert!(rendered.contains("private-handle-id"), "{rendered}");
    }

    #[tokio::test]
    async fn public_live_stream_filters_wake_registration_edge() {
        let (db, tx, rx, _snapshot) = live_stream_fixture(4).await;
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::PublicShare,
            None,
            db,
            rx,
        );
        futures::pin_mut!(stream);
        tx.send(wake_registration_event())
            .expect("private broadcast");
        tx.send(SseEvent::Token {
            sequence_id: 8,
            text: "public-after-private".to_string(),
            request_id: "request".to_string(),
        })
        .expect("public broadcast");
        let rendered = format!("{:?}", stream.next().await.expect("event").unwrap());
        assert!(rendered.contains("public-after-private"), "{rendered}");
        assert!(!rendered.contains("private-contract-id"), "{rendered}");
    }

    #[tokio::test]
    async fn fast_broadcasts_before_poll_deadline_have_no_forced_wait() {
        let (db, tx, rx, snapshot) = live_stream_fixture(128).await;
        tokio::time::pause();
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);

        for sequence_id in 1..=100 {
            tx.send(SseEvent::Token {
                sequence_id,
                text: format!("event-{sequence_id}"),
                request_id: "request".to_string(),
            })
            .expect("broadcast");
        }
        let before_drain = tokio::time::Instant::now();
        for sequence_id in 1..=100 {
            let rendered = format!("{:?}", stream.next().await.expect("event").unwrap());
            assert!(
                rendered.contains(&format!("event-{sequence_id}")),
                "{rendered}"
            );
        }
        assert_eq!(tokio::time::Instant::now(), before_drain);
    }

    #[tokio::test]
    async fn live_stream_emits_changed_wake_snapshot() {
        use chrono::Duration as ChronoDuration;
        use phoenix_core::domain::wake_contracts::{
            WakeContract, WakeContractHandle, WakeContractStatus,
        };

        let (db, _tx, rx, snapshot) = live_stream_fixture(4).await;
        let registered_at = Utc::now();
        db.insert_wake_contract(&WakeContract {
            id: "wake-live".to_string(),
            current_conversation_id: "live-sse".to_string(),
            registration_work_scope: crate::work_scope::WorkScope::Conversation(
                "live-sse".to_string(),
            ),
            handle: WakeContractHandle::Bash {
                handle_id: "bash-live".to_string(),
            },
            registering_tool_use_id: Some("tool-live".to_string()),
            registered_at,
            expires_at: registered_at + ChronoDuration::seconds(60),
            status: WakeContractStatus::Pending,
            terminal_cause: None,
            forgotten_reason: None,
            terminal_payload: None,
            resolved_at: None,
        })
        .await
        .expect("wake contract");

        tokio::time::pause();
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);
        tokio::time::advance(Duration::from_secs(1)).await;
        let event = stream.next().await.expect("changed wake event").unwrap();
        let rendered = format!("{event:?}");
        assert!(rendered.contains("wake_status_update"), "{rendered}");
        assert!(rendered.contains("wake-live"), "{rendered}");
    }

    #[tokio::test]
    async fn ready_broadcasts_do_not_starve_changed_wake_snapshot() {
        use chrono::Duration as ChronoDuration;
        use phoenix_core::domain::wake_contracts::{
            WakeContract, WakeContractHandle, WakeContractStatus,
        };

        let (db, tx, rx, snapshot) = live_stream_fixture(256).await;
        let registered_at = Utc::now();
        db.insert_wake_contract(&WakeContract {
            id: "wake-fair".to_string(),
            current_conversation_id: "live-sse".to_string(),
            registration_work_scope: crate::work_scope::WorkScope::Conversation(
                "live-sse".to_string(),
            ),
            handle: WakeContractHandle::Bash {
                handle_id: "bash-fair".to_string(),
            },
            registering_tool_use_id: None,
            registered_at,
            expires_at: registered_at + ChronoDuration::seconds(60),
            status: WakeContractStatus::Pending,
            terminal_cause: None,
            forgotten_reason: None,
            terminal_payload: None,
            resolved_at: None,
        })
        .await
        .expect("wake contract");

        tokio::time::pause();
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);

        tx.send(SseEvent::Token {
            sequence_id: 1,
            text: "busy".to_string(),
            request_id: "request".to_string(),
        })
        .expect("initial broadcast");
        let initial = stream.next().await.expect("initial live event").unwrap();
        assert!(!format!("{initial:?}").contains("wake_status_update"));

        tokio::time::advance(Duration::from_secs(1)).await;
        for sequence_id in 2..=128 {
            tx.send(SseEvent::Token {
                sequence_id,
                text: "busy".to_string(),
                request_id: "request".to_string(),
            })
            .expect("broadcast");
            let event = stream.next().await.expect("live event").unwrap();
            if format!("{event:?}").contains("wake_status_update") {
                return;
            }
        }
        panic!("ready broadcasts starved the overdue wake snapshot");
    }

    #[tokio::test]
    async fn broadcast_close_ends_live_stream_despite_wake_ticker() {
        let (db, tx, rx, snapshot) = live_stream_fixture(4).await;
        tokio::time::pause();
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);
        drop(tx);
        tokio::time::advance(Duration::from_secs(2)).await;

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn broadcast_lag_ends_entire_live_stream_for_reconnect() {
        let (db, tx, rx, snapshot) = live_stream_fixture(1).await;
        tx.send(SseEvent::Token {
            sequence_id: 1,
            text: "overwritten".to_string(),
            request_id: "request".to_string(),
        })
        .expect("first broadcast");
        tx.send(SseEvent::Token {
            sequence_id: 2,
            text: "latest".to_string(),
            request_id: "request".to_string(),
        })
        .expect("second broadcast");
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn live_stream_passes_through_ordinary_broadcasts() {
        let (db, tx, rx, snapshot) = live_stream_fixture(4).await;
        let stream = live_event_stream(
            "live-sse".to_string(),
            EventVisibility::Authenticated,
            Some(snapshot),
            db,
            rx,
        );
        futures::pin_mut!(stream);
        tx.send(SseEvent::Token {
            sequence_id: 7,
            text: "hello".to_string(),
            request_id: "request".to_string(),
        })
        .expect("broadcast");

        let event = stream.next().await.expect("broadcast event").unwrap();
        let rendered = format!("{event:?}");
        assert!(rendered.contains("token"), "{rendered}");
        assert!(rendered.contains("hello"), "{rendered}");
    }

    #[test]
    fn parity_message_user() {
        let event = SseEvent::Message {
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
            sequence_id: 7,
            message_id: "msg-abc".to_string(),
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
            sequence_id: 9,
            message_id: "msg-def".to_string(),
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
            sequence_id: 11,
            message_id: "msg-xyz".to_string(),
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
            sequence_id: 12,
            message_id: "msg-tool-result".to_string(),
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
            conversation_id: "conv-1".to_string(),
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
                }],
                tmux: None,
                browser: None,
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
            message_id: "msg-steer-1".to_string(),
            queue_position: 0,
        };
        assert_parity(&event);
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
            sequence_id: init_seq,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            messages: Vec::new(),
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: init_seq,
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
            sequence_id: init_seq,
            conversation: Box::new(fixture_enriched_conversation()),
            transcript_generation: 1,
            messages: Vec::new(),
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: init_seq,
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

    #[test]
    fn axum_event_label_for_message_updated() {
        // Regression on the label the client registers via
        // `addEventListener`.
        let event = SseEvent::MessageUpdated {
            sequence_id: 42,
            message_id: "msg-abc".to_string(),
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
