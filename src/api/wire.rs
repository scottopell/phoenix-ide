//! SSE wire format — typed serialization boundary.
//!
//! This module is the single Rust-side source of truth for the shape of SSE
//! events on the wire. `SseWireEvent` (internally tagged by `type`) replaces
//! the hand-rolled `json!()` macros that used to live in [`super::sse`]: every
//! broadcast [`crate::runtime::SseEvent`] is `From`-converted into an
//! `SseWireEvent`, then `serde_json::to_string`'d into the SSE `data:` line.
//!
//! The typed path doubles as the codegen source: `#[derive(ts_rs::TS)]` +
//! `#[ts(export)]` emits `ui/src/generated/sse.ts` during `cargo test` (see
//! `export_sse_types`). The generated file is checked into git and CI fails
//! if it drifts from the Rust types (`./dev.py check` runs
//! `git diff --exit-code ui/src/generated/`). That closes the loop: the
//! Rust type, the TS type, and the runtime valibot schema (typed against the
//! generated TS in `ui/src/sseSchemas.ts`) cannot disagree without tripping
//! a compile error or a CI gate — see task 02677.
//!
//! ### Deliberately opaque fields
//!
//! A few fields are carried as `serde_json::Value` and surface as `unknown`
//! on the TS side rather than being unfolded into generated types:
//!
//! - `EnrichedMessage.content` — the `MessageContent` union is large,
//!   already treated as `v.unknown()` on the client (see
//!   `ui/src/sseSchemas.ts`), and structurally unfolding it here would
//!   duplicate the existing hand-authored `MessageContent` TS type. The UI
//!   pattern-matches on `message_type` + structural access and casts as
//!   needed.
//! - `EnrichedMessage.display_data` — free-form UI hinting payload that
//!   varies by tool.
//! - `EnrichedConversation` (as referenced from `SseWireEvent::Init`) —
//!   the full conversation shape is hand-authored in `ui/src/api.ts` as
//!   `Conversation`; the generated wire types reference it as `unknown` to
//!   avoid duplicating a large record here. Only the two load-bearing
//!   envelope fields (`sequence_id`, `last_sequence_id`) need the codegen
//!   guarantee.
//! - `SseWireEvent::StateChange.state` — `ConvState` is a deeply-nested
//!   discriminated union. The UI routes it through `parseConversationState`
//!   which performs its own validation; duplicating the union in ts-rs
//!   would undo the "single source of truth" win and pull in many
//!   transitive types.
//! - `SseWireEvent::ConversationUpdate.conversation` — the reducer merges
//!   it shallowly onto `Conversation`; forward-compat dominates over
//!   enforcement.
//!
//! These are marked with `#[ts(type = "unknown")]` so the emitted TS
//! matches the wire reality and matches what the valibot schemas already
//! declare.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

use crate::chain_runtime::ChainSseEvent;
use crate::db::{Message, MessageType, UsageData};
use crate::runtime::{
    user_facing_error::UserFacingError, ConversationMetadataUpdate, EnrichedConversation,
    SseBreadcrumb, SseEvent,
};

/// A message enriched for API output: bash `tool_use` blocks have their
/// `display` field merged into `content`. This is what `EnrichedMessage`
/// carries on the wire; `crate::db::Message` (the DB record) is the input.
///
/// The transformation is implemented by [`enrich_content`] below, which
/// walks the `content` JSON and merges `display_data.bash[*].display` into
/// matching `tool_use` blocks. The semantics match the old
/// `enrich_message_for_api(&Message) -> Value` helper byte-for-byte.
///
/// `content` and `display_data` stay as `serde_json::Value` — see the module
/// docs for the rationale.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct EnrichedMessage {
    pub message_id: String,
    pub conversation_id: String,
    pub sequence_id: i64,
    pub message_type: MessageType,
    #[ts(type = "unknown")]
    pub content: Value,
    #[ts(type = "unknown | null")]
    pub display_data: Option<Value>,
    pub usage_data: Option<UsageData>,
    pub created_at: DateTime<Utc>,
}

impl From<&Message> for EnrichedMessage {
    fn from(msg: &Message) -> Self {
        let content = enrich_content(msg);
        Self {
            message_id: msg.message_id.clone(),
            conversation_id: msg.conversation_id.clone(),
            sequence_id: msg.sequence_id,
            message_type: msg.message_type,
            content,
            display_data: msg.display_data.clone(),
            usage_data: msg.usage_data.clone(),
            created_at: msg.created_at,
        }
    }
}

impl From<Message> for EnrichedMessage {
    fn from(msg: Message) -> Self {
        Self::from(&msg)
    }
}

/// Serialize `msg.content` and, for agent messages, merge
/// `msg.display_data.bash[*].display` into matching `tool_use` blocks.
///
/// Behaviour matches the legacy `enrich_message_for_api` helper exactly:
/// the serialized JSON of the `Message` is produced via the existing
/// `Serialize` impl, then the `content` sub-tree is mutated in place. Callers
/// that only need the enriched content (as opposed to the whole message)
/// get it without the surrounding envelope fields.
fn enrich_content(msg: &Message) -> Value {
    let full = serde_json::to_value(msg).unwrap_or(Value::Null);
    let mut content = full.get("content").cloned().unwrap_or(Value::Null);

    if msg.message_type != MessageType::Agent {
        return content;
    }

    let Some(display_data) = &msg.display_data else {
        return content;
    };

    merge_bash_displays_into_content(&mut content, display_data);
    content
}

/// `display_data` shape: `{ "bash": [{ "tool_use_id": "...", "display": "..." }] }`.
/// Mutates `content` to set `display` on matching bash `tool_use` blocks.
fn merge_bash_displays_into_content(content: &mut Value, display_data: &Value) {
    use std::collections::HashMap;

    let bash_displays: HashMap<String, String> = display_data
        .get("bash")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let id = item.get("tool_use_id")?.as_str()?;
                    let display = item.get("display")?.as_str()?;
                    Some((id.to_string(), display.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    if bash_displays.is_empty() {
        return;
    }

    let Some(blocks) = content.as_array_mut() else {
        return;
    };

    for block in blocks.iter_mut() {
        let is_bash_tool_use = block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && block.get("name").and_then(|n| n.as_str()) == Some("bash");
        if !is_bash_tool_use {
            continue;
        }
        let Some(id) = block.get("id").and_then(|i| i.as_str()).map(String::from) else {
            continue;
        };
        if let Some(display) = bash_displays.get(&id) {
            if let Some(obj) = block.as_object_mut() {
                obj.insert("display".to_string(), Value::String(display.clone()));
            }
        }
    }
}

/// Wire-format `SseEvent`. Single source of truth for what each variant looks
/// like on the `data:` line of an SSE frame. Every broadcast-side
/// [`SseEvent`] goes through `From<SseEvent>` into `SseWireEvent` and then
/// through `serde_json::to_string`.
///
/// `#[serde(tag = "type", rename_all = "snake_case")]` puts the discriminant
/// on the wire as the `type` field — matches the old `json!()` shape and what
/// the TS schemas + `EventSource.addEventListener(eventType, ...)` calls
/// consume.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum SseWireEvent {
    /// Full state snapshot at connect / reconnect.
    Init {
        sequence_id: i64,
        /// Hand-authored TS type `Conversation` in `ui/src/api.ts` is the
        /// consumer; we pass `unknown` through codegen so the generated file
        /// doesn't duplicate the large conversation record. Boxed to keep
        /// `SseWireEvent`'s enum discriminant small (matches the upstream
        /// `SseEvent::Init.conversation: Box<_>` indirection).
        #[ts(type = "unknown")]
        conversation: Box<EnrichedConversation>,
        /// `EnrichedMessage` is exported as its own generated type for
        /// callers that want the Rust-derived shape, but the init payload
        /// carries it as `unknown[]` so the UI's hand-authored `Message`
        /// type (`ui/src/api.ts`) — slightly narrower in a few places —
        /// doesn't structurally clash with the codegen output. The
        /// valibot schema validates each element against `MessageSchema`
        /// and transforms to `Message` at that boundary.
        #[ts(type = "Array<unknown>")]
        messages: Vec<EnrichedMessage>,
        agent_working: bool,
        display_state: String,
        last_sequence_id: i64,
        context_window_size: u64,
        breadcrumbs: Vec<SseBreadcrumb>,
        commits_behind: u32,
        commits_ahead: u32,
        project_name: Option<String>,
    },
    /// A newly-persisted message joins the conversation. The envelope
    /// `sequence_id` equals `message.sequence_id` by construction.
    Message {
        sequence_id: i64,
        /// See the note on `Init.messages` — the message payload is
        /// validated against `MessageSchema` and transformed to the UI's
        /// `Message` type at the valibot boundary.
        #[ts(type = "unknown")]
        message: EnrichedMessage,
    },
    /// In-place mutation of an existing message's mutable fields.
    MessageUpdated {
        sequence_id: i64,
        message_id: String,
        #[ts(type = "unknown | null")]
        display_data: Option<Value>,
        #[ts(type = "unknown | null")]
        content: Option<Value>,
        /// Tool-execution duration in milliseconds. Present only when the
        /// `MessageUpdated` event is emitted for a tool-result message;
        /// absent (`undefined` on the TS side) for all other update paths.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        duration_ms: Option<u64>,
    },
    /// Conversation phase transition. `state` is opaque here — the UI has
    /// its own tagged-union validator (`parseConversationState`).
    StateChange {
        sequence_id: i64,
        #[ts(type = "unknown")]
        state: Value,
        display_state: String,
    },
    /// Ephemeral streaming token (LLM delta).
    Token {
        sequence_id: i64,
        text: String,
        request_id: String,
    },
    /// Agent reached an idle state and is no longer working.
    AgentDone { sequence_id: i64 },
    /// Conversation hit a terminal state — the terminal subsystem uses this
    /// to tear down PTYs.
    ConversationBecameTerminal { sequence_id: i64 },
    /// Partial conversation metadata update.
    ConversationUpdate {
        sequence_id: i64,
        #[ts(type = "unknown")]
        conversation: ConversationMetadataUpdate,
    },
    /// User-facing error. Carries both a flattened `message` (what the
    /// existing toast renders) and the typed `error` payload for
    /// kind-aware affordances.
    Error {
        sequence_id: i64,
        message: String,
        /// Generated as `unknown` — the existing UI reads only the flat
        /// `message` field. Kind-aware consumers can narrow against
        /// `UserFacingError` (also exported by ts-rs for future use).
        #[ts(type = "unknown")]
        error: UserFacingError,
    },
    /// REQ-BED-032 step 6: a conversation has just been hard-deleted (its
    /// row is gone from `SQLite`, all per-conversation resources cleaned
    /// up). UI consumers refresh sidebar / navigation in response. Emitted
    /// once per hard-delete, after every cascade step.
    ConversationHardDeleted {
        sequence_id: i64,
        conversation_id: String,
    },
}

impl SseWireEvent {
    /// SSE `event:` label for this variant — matches the tag used by
    /// `EventSource.addEventListener` on the client.
    pub fn event_type(&self) -> &'static str {
        match self {
            SseWireEvent::Init { .. } => "init",
            SseWireEvent::Message { .. } => "message",
            SseWireEvent::MessageUpdated { .. } => "message_updated",
            SseWireEvent::StateChange { .. } => "state_change",
            SseWireEvent::Token { .. } => "token",
            SseWireEvent::AgentDone { .. } => "agent_done",
            SseWireEvent::ConversationBecameTerminal { .. } => "conversation_became_terminal",
            SseWireEvent::ConversationUpdate { .. } => "conversation_update",
            SseWireEvent::Error { .. } => "error",
            SseWireEvent::ConversationHardDeleted { .. } => "conversation_hard_deleted",
        }
    }
}

impl From<SseEvent> for SseWireEvent {
    fn from(event: SseEvent) -> Self {
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
            } => SseWireEvent::Init {
                sequence_id,
                conversation,
                messages: messages.iter().map(EnrichedMessage::from).collect(),
                agent_working,
                display_state,
                last_sequence_id,
                context_window_size,
                breadcrumbs,
                commits_behind,
                commits_ahead,
                project_name,
            },
            SseEvent::Message { message } => {
                // The envelope `sequence_id` equals `message.sequence_id` —
                // this is what the client already expects (see
                // `ui/src/sseSchemas.ts` `SseMessageDataSchema`).
                let sequence_id = message.sequence_id;
                SseWireEvent::Message {
                    sequence_id,
                    message: EnrichedMessage::from(message),
                }
            }
            SseEvent::MessageUpdated {
                sequence_id,
                message_id,
                display_data,
                content,
                duration_ms,
            } => SseWireEvent::MessageUpdated {
                sequence_id,
                message_id,
                display_data,
                // `content` is `Option<MessageContent>` at the runtime layer
                // and serializes to the same JSON shape as a Message's
                // `content` field; pass through as `Value` here.
                content: content.map(|c| serde_json::to_value(&c).unwrap_or(Value::Null)),
                duration_ms,
            },
            SseEvent::StateChange {
                sequence_id,
                state,
                display_state,
            } => SseWireEvent::StateChange {
                sequence_id,
                state: serde_json::to_value(&state).unwrap_or(Value::Null),
                display_state,
            },
            SseEvent::Token {
                sequence_id,
                text,
                request_id,
            } => SseWireEvent::Token {
                sequence_id,
                text,
                request_id,
            },
            SseEvent::AgentDone { sequence_id } => SseWireEvent::AgentDone { sequence_id },
            SseEvent::ConversationBecameTerminal { sequence_id } => {
                SseWireEvent::ConversationBecameTerminal { sequence_id }
            }
            SseEvent::ConversationUpdate {
                sequence_id,
                update,
            } => SseWireEvent::ConversationUpdate {
                sequence_id,
                conversation: update,
            },
            SseEvent::Error { sequence_id, error } => {
                // Flat `message` (for the existing toast) + typed `error`
                // (task 24682) — wire shape unchanged.
                let message = error.flat_message();
                SseWireEvent::Error {
                    sequence_id,
                    message,
                    error,
                }
            }
            SseEvent::ConversationHardDeleted {
                sequence_id,
                conversation_id,
            } => SseWireEvent::ConversationHardDeleted {
                sequence_id,
                conversation_id,
            },
        }
    }
}

/// Wire-format chain Q&A events (Phoenix Chains v1, REQ-CHN-004).
///
/// Distinct from [`SseWireEvent`] because chain broadcasters carry their
/// own demux discriminator (`chain_qa_id`) rather than a per-conversation
/// monotonic `sequence_id`. Each variant maps 1:1 to a
/// [`ChainSseEvent`] case; the conversion lives in `From<ChainSseEvent>`
/// below. The SSE `event:` label is the variant's `snake_case` tag.
#[allow(dead_code, clippy::enum_variant_names)] // Phase 4 wires API handlers; ChainQa* prefix mirrors the wire tag domain.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum ChainSseWireEvent {
    /// Streaming token chunk for an in-flight Q&A. Subscribers filter on
    /// `chain_qa_id` to demultiplex concurrent questions on the same chain
    /// (REQ-CHN-006: a sibling tab's question must not render into mine).
    ChainQaToken { chain_qa_id: String, delta: String },
    /// Stream completed cleanly. `full_answer` matches what was just
    /// persisted to `chain_qa.answer`; subsequent reads via
    /// `list_chain_qa` would return the same string.
    ChainQaCompleted {
        chain_qa_id: String,
        full_answer: String,
    },
    /// Stream ended in error. `partial_answer` carries whatever tokens
    /// streamed before the failure (may be `None` when no token was emitted
    /// before the error).
    ChainQaFailed {
        chain_qa_id: String,
        error: String,
        partial_answer: Option<String>,
    },
}

impl ChainSseWireEvent {
    /// SSE `event:` label for this variant.
    #[allow(dead_code)] // Phase 4 wires API handlers that consume this on the wire.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::ChainQaToken { .. } => "chain_qa_token",
            Self::ChainQaCompleted { .. } => "chain_qa_completed",
            Self::ChainQaFailed { .. } => "chain_qa_failed",
        }
    }
}

impl From<ChainSseEvent> for ChainSseWireEvent {
    fn from(event: ChainSseEvent) -> Self {
        match event {
            ChainSseEvent::Token { chain_qa_id, delta } => {
                Self::ChainQaToken { chain_qa_id, delta }
            }
            ChainSseEvent::Completed {
                chain_qa_id,
                full_answer,
            } => Self::ChainQaCompleted {
                chain_qa_id,
                full_answer,
            },
            ChainSseEvent::Failed {
                chain_qa_id,
                error,
                partial_answer,
            } => Self::ChainQaFailed {
                chain_qa_id,
                error,
                partial_answer,
            },
        }
    }
}

// Codegen note: types annotated with `#[ts(export)]` above are emitted to
// `ui/src/generated/` automatically whenever `cargo test` is run — no
// explicit test is needed (ts-rs v12 has built-in test-time export
// plumbing). `./dev.py check` runs `cargo test` followed by
// `git diff --exit-code ui/src/generated/` so a developer who edits a
// Rust type here without running tests will see the check fail.

#[cfg(test)]
mod chain_wire_tests {
    use super::*;

    /// Wire round-trip parity for `ChainQaToken`: the typed wire variant
    /// serializes to the JSON shape the UI's valibot schema will validate
    /// against (`type: "chain_qa_token"`, `snake_case` fields).
    #[test]
    fn chain_qa_token_serializes_with_expected_tag_and_fields() {
        let wire: ChainSseWireEvent = ChainSseEvent::Token {
            chain_qa_id: "qa-1".to_string(),
            delta: "Hello".to_string(),
        }
        .into();
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["type"], "chain_qa_token");
        assert_eq!(json["chain_qa_id"], "qa-1");
        assert_eq!(json["delta"], "Hello");
        assert_eq!(wire.event_type(), "chain_qa_token");
    }

    #[test]
    fn chain_qa_completed_carries_full_answer() {
        let wire: ChainSseWireEvent = ChainSseEvent::Completed {
            chain_qa_id: "qa-2".to_string(),
            full_answer: "the assembled answer".to_string(),
        }
        .into();
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["type"], "chain_qa_completed");
        assert_eq!(json["chain_qa_id"], "qa-2");
        assert_eq!(json["full_answer"], "the assembled answer");
        assert_eq!(wire.event_type(), "chain_qa_completed");
    }

    #[test]
    fn chain_qa_failed_carries_error_and_partial() {
        let wire: ChainSseWireEvent = ChainSseEvent::Failed {
            chain_qa_id: "qa-3".to_string(),
            error: "boom".to_string(),
            partial_answer: Some("hel".to_string()),
        }
        .into();
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["type"], "chain_qa_failed");
        assert_eq!(json["chain_qa_id"], "qa-3");
        assert_eq!(json["error"], "boom");
        assert_eq!(json["partial_answer"], "hel");
        assert_eq!(wire.event_type(), "chain_qa_failed");
    }

    #[test]
    fn chain_qa_failed_with_null_partial_serializes_as_null() {
        let wire: ChainSseWireEvent = ChainSseEvent::Failed {
            chain_qa_id: "qa-4".to_string(),
            error: "nope".to_string(),
            partial_answer: None,
        }
        .into();
        let json = serde_json::to_value(&wire).unwrap();
        assert!(json["partial_answer"].is_null());
    }
}
