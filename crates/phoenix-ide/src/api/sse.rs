//! Server-Sent Events support
//!
//! REQ-API-005: Real-time Streaming
//!
//! The serialization boundary lives in [`super::wire::SseWireEvent`]: every
//! broadcast [`SseEvent`] is `From`-converted into the typed wire enum and
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
    conv_id: String,
    init_event: SseEvent,
    broadcast_rx: tokio::sync::broadcast::Receiver<SseEvent>,
) -> impl IntoResponse {
    let init =
        futures::stream::once(
            async move { Ok::<Event, Infallible>(sse_event_to_axum(init_event)) },
        );

    let broadcasts = BroadcastStream::new(broadcast_rx)
        .take_while(move |result| match result {
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(
                    conv_id = %conv_id,
                    lagged_by = n,
                    "SSE broadcast lagged; closing stream so client reconnects and resyncs"
                );
                false
            }
            _ => true,
        })
        .filter_map(|result| match result {
            Ok(event) => Some(Ok(sse_event_to_axum(event))),
            Err(_) => None, // Lagged already closed the stream above
        });

    let combined = init.chain(broadcasts);

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

fn sse_event_to_axum(event: SseEvent) -> Event {
    let wire: SseWireEvent = event.into();
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
    use crate::runtime::{ConversationMetadataUpdate, EnrichedConversation, SseBreadcrumb};
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
                breadcrumbs,
                project_name,
                pending_anchor_sequence_id,
                pending_events,
                pending_truncated,
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
                    "messages": enriched_msgs,
                    "agent_working": agent_working,
                    "presentation_mode": presentation_mode,
                    "last_sequence_id": last_sequence_id,
                    "context_window_size": context_window_size,
                    "breadcrumbs": breadcrumbs,
                    "project_name": project_name,
                    "pending_anchor_sequence_id": pending_anchor_sequence_id,
                    "pending_events": pending_json,
                    "pending_truncated": pending_truncated,
                })
            }
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
            model: Some("claude-sonnet-4-5".to_string()),
            project_id: None,
            conv_mode: ConvMode::Explore {
                worktree_path: None,
            },
            desired_base_branch: None,
            message_count: 3,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            steering_queue: vec![],
            llm_language: crate::llm_language::LlmLanguage::default(),
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
            parent_conversation_slug: None,
            browser_session_active: false,
            terminal_uses_tmux: false,
            work_scope_key: "conversation:conv-1".to_string(),
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
        use crate::llm::ContentBlock;
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

    fn fixture_breadcrumbs() -> Vec<SseBreadcrumb> {
        vec![SseBreadcrumb {
            crumb_type: "user".to_string(),
            label: "first message".to_string(),
            tool_id: None,
            sequence_id: Some(1),
            preview: None,
        }]
    }

    // ------------------------------------------------------------------
    // Parity tests — one per SseEvent variant
    // ------------------------------------------------------------------

    #[test]
    fn parity_init() {
        let event = SseEvent::Init {
            sequence_id: 42,
            conversation: Box::new(fixture_enriched_conversation()),
            messages: vec![fixture_user_message(), fixture_agent_message_with_bash()],
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: 42,
            context_window_size: 2048,
            breadcrumbs: fixture_breadcrumbs(),
            project_name: Some("phoenix".to_string()),
            pending_anchor_sequence_id: 0,
            pending_events: Vec::new(),
            pending_truncated: false,
        };
        assert_parity(&event);
    }

    /// Init with a populated `ReplayRing` snapshot. Exercises the recursive
    /// SseWireEvent serialisation inside `pending_events` and the parity
    /// of `pending_anchor_sequence_id` / `pending_truncated`.
    ///
    /// Every entry's `sequence_id` exceeds the anchor — matches what a real
    /// `ReplayRing` snapshot can ever produce (sse_wire.allium invariant
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
            messages: vec![fixture_user_message()],
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: 45,
            context_window_size: 2048,
            breadcrumbs: fixture_breadcrumbs(),
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
            messages: vec![fixture_user_message()],
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: 99,
            context_window_size: 0,
            breadcrumbs: Vec::new(),
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
        use crate::llm::ContentBlock;
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
            reason: crate::llm::LlmAttemptReason::RateLimit,
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
            reason: crate::llm::LlmAttemptReason::Network,
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
                cwd: Some("/new/cwd".to_string()),
                branch_name: None,
                worktree_path: None,
                conv_mode_label: Some("work".to_string()),
                base_branch: None,
                task_title: None,
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
            messages: Vec::new(),
            agent_working: true,
            presentation_mode: "working".to_string(),
            last_sequence_id: init_seq,
            context_window_size: 0,
            breadcrumbs: Vec::new(),
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
    /// init carries empty pending_events with the anchor advanced.
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
                content: MessageContent::agent(vec![crate::llm::ContentBlock::text("hi")]),
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
            messages: Vec::new(),
            agent_working: false,
            presentation_mode: "idle".to_string(),
            last_sequence_id: init_seq,
            context_window_size: 0,
            breadcrumbs: Vec::new(),
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
