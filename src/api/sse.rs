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
        .take_while(move |result| {
            if let Err(BroadcastStreamRecvError::Lagged(n)) = result {
                tracing::warn!(
                    conv_id = %conv_id,
                    lagged_by = n,
                    "SSE broadcast lagged; closing stream so client reconnects and resyncs"
                );
                false
            } else {
                true
            }
        })
        .filter_map(|result| match result {
            Ok(event) => Some(Ok(sse_event_to_axum(event))),
            Err(_) => None, // Lagged already closed the stream above
        });

    let combined = init.chain(broadcasts);

    let sse = Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
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
                display_state,
                last_sequence_id,
                context_window_size,
                breadcrumbs,
                commits_behind,
                commits_ahead,
                project_name,
            } => {
                let enriched_msgs: Vec<Value> =
                    messages.iter().map(enrich_message_for_api).collect();
                json!({
                    "type": "init",
                    "sequence_id": sequence_id,
                    "conversation": conversation,
                    "messages": enriched_msgs,
                    "agent_working": agent_working,
                    "display_state": display_state,
                    "last_sequence_id": last_sequence_id,
                    "context_window_size": context_window_size,
                    "breadcrumbs": breadcrumbs,
                    "commits_behind": commits_behind,
                    "commits_ahead": commits_ahead,
                    "project_name": project_name,
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
                display_state,
            } => json!({
                "type": "state_change",
                "sequence_id": sequence_id,
                "state": serde_json::to_value(state).unwrap_or(Value::Null),
                "display_state": display_state,
            }),
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
            conv_mode: ConvMode::Explore,
            desired_base_branch: None,
            message_count: 3,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
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
            display_state: "idle".to_string(),
            last_sequence_id: 42,
            context_window_size: 2048,
            breadcrumbs: fixture_breadcrumbs(),
            commits_behind: 0,
            commits_ahead: 3,
            project_name: Some("phoenix".to_string()),
        };
        assert_parity(&event);
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
            display_state: "idle".to_string(),
        };
        assert_parity(&event);
    }

    #[test]
    fn parity_state_change_llm_requesting() {
        let event = SseEvent::StateChange {
            sequence_id: 14,
            state: ConvState::LlmRequesting { attempt: 1 },
            display_state: "working".to_string(),
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
                commits_behind: Some(0),
                commits_ahead: Some(2),
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

    // ------------------------------------------------------------------
    // Backwards-compat sanity: the axum Event is still constructed with
    // the correct `event:` label for every variant.
    // ------------------------------------------------------------------

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
