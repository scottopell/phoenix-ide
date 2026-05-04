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
use serde::{Deserialize, Serialize};
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

// ---------------------------------------------------------------------------
// Bash and tmux tool response wire types (task 02697).
//
// These structs are the typed contract for what `BashTool` and `TmuxTool`
// emit on the wire as `tool_result` content (`ToolOutput.output` /
// `ToolOutput.display_data`). They are NOT directly transported as
// `SseWireEvent` variants — bash/tmux results travel inside an enriched
// message's `content` / `display_data` payload, which is carried as
// `serde_json::Value` here (see "Deliberately opaque fields" at the top
// of this module).
//
// What ts_rs codegen + the valibot satisfies-bound buys us: a Rust-side
// change to a response field surfaces as a TS type change in
// `ui/src/generated/`, which the UI's valibot schemas in
// `ui/src/sseSchemas.ts` must validate against — drift between the
// emitted JSON and the runtime validator becomes a compile error rather
// than a production runtime surprise.
//
// Wire shape MUST remain byte-for-byte compatible with the JSON the
// `BashTool` / `TmuxTool` operations produced before this typing pass —
// the existing 02694 / 02695 integration tests are the ground truth.
// ---------------------------------------------------------------------------

/// Bash response shape, tagged by `status`. Encompasses every successful
/// (non-error) shape emitted by [`crate::tools::BashTool`] across spawn /
/// peek / wait / kill operations (REQ-BASH-002, REQ-BASH-003, REQ-BASH-006).
///
/// Variant correspondence:
///
/// | `status`              | When                                                              |
/// |-----------------------|-------------------------------------------------------------------|
/// | `running`             | peek on a live handle (no kill in flight)                         |
/// | `still_running`       | spawn-window elapsed; wait re-timeout                             |
/// | `kill_pending_kernel` | kill response window expired without exit                          |
/// | `tombstoned`          | peek/wait/kill served from a tombstone                            |
/// | `exited`              | spawn observed exit within `wait_seconds`                         |
/// | `killed`              | spawn observed signal-termination within `wait_seconds`           |
/// | `waiter_panicked`     | exit observer fired while state was still `Live` (waiter panic)   |
///
/// Field availability per status is intentionally non-uniform — it
/// matches what the tool actually emits (and what tests assert against).
/// `serde(tag = "status")` produces a flat object with `status` as the
/// discriminator.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum BashResponse {
    /// Live handle observed via peek (no kill in flight).
    Running(BashRunningPayload),
    /// Spawn / wait window elapsed without exit; same handle is returned.
    StillRunning(BashStillRunningPayload),
    /// Kill response timer expired; signal sent but process not yet exited.
    KillPendingKernel(BashKillPendingKernelPayload),
    /// Handle is terminal; served from tombstone (peek / wait / kill).
    Tombstoned(BashTombstonedPayload),
    /// Spawn-path: process exited normally inside the wait window.
    Exited(BashSpawnTombstonePayload),
    /// Spawn-path: process was signal-terminated inside the wait window.
    Killed(BashSpawnTombstonePayload),
    /// Waiter task panicked; surface as a structured response so callers
    /// don't hang. Only fields needed for diagnosis are emitted.
    WaiterPanicked(BashWaiterPanickedPayload),
}

/// Common ring-buffer view returned alongside any handle response
/// (REQ-BASH-004).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashRingWindow {
    pub start_offset: u64,
    pub end_offset: u64,
    pub truncated_before: bool,
    pub lines: Vec<BashRingLine>,
}

/// Single ring line; `bytes` is the line contents as a (lossy) UTF-8
/// string, matching what the JSON wire emits today.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashRingLine {
    pub offset: u64,
    pub bytes: String,
}

/// `running` response payload. Spec REQ-BASH-003.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashRunningPayload {
    pub handle: String,
    pub cmd: String,
    #[serde(flatten)]
    pub window: BashRingWindow,
    /// Set when a kill has been issued and is in flight against this
    /// otherwise-still-live handle (`kill_pending_kernel` is reached only
    /// after the response window expires; until then `running` carries
    /// the in-flight kill metadata).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    /// Display label per REQ-BASH-015 (peek/wait/kill operations).
    pub display: String,
    /// Optional kill-response top-level field (kill response only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deprecation_notice: Option<String>,
}

/// `still_running` response payload. Distinguished from `running` by the
/// `waited_ms` field and the absence of a `display` label (spawn / wait
/// re-timeout responses don't synthesize a label — REQ-BASH-002 /
/// REQ-BASH-015).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashStillRunningPayload {
    pub handle: String,
    pub cmd: String,
    pub waited_ms: u64,
    #[serde(flatten)]
    pub window: BashRingWindow,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deprecation_notice: Option<String>,
}

/// `kill_pending_kernel` response payload (REQ-BASH-003). The kill
/// response timer expired before the kernel delivered the exit; the
/// process is still alive and the handle stays subscribable.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashKillPendingKernelPayload {
    pub handle: String,
    pub cmd: String,
    #[serde(flatten)]
    pub window: BashRingWindow,
    pub kill_signal_sent: String,
    pub kill_attempted_at: String,
    pub display: String,
    /// Echoes the signal sent on this kill call (`TERM` / `KILL`).
    pub signal_sent: String,
}

/// `tombstoned` response payload (REQ-BASH-006). Served on peek/wait/kill
/// for handles that have reached a terminal state.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashTombstonedPayload {
    pub handle: String,
    pub cmd: String,
    pub final_cause: String,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_number: Option<i32>,
    pub duration_ms: u64,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    #[serde(flatten)]
    pub window: BashRingWindow,
    pub display: String,
    /// Echo of the kill signal on the `kill` operation (None on peek/wait
    /// of an already-terminal handle).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deprecation_notice: Option<String>,
}

/// Spawn-path tombstone response (status `exited` or `killed`). Differs
/// from [`BashTombstonedPayload`] by the absence of the synthesized
/// `display` label — spawn responses carry the original `cmd` instead.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashSpawnTombstonePayload {
    pub handle: String,
    pub cmd: String,
    pub final_cause: String,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_number: Option<i32>,
    pub duration_ms: u64,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    #[serde(flatten)]
    pub window: BashRingWindow,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deprecation_notice: Option<String>,
}

/// `waiter_panicked` response. Surface for the rare case the bash
/// waiter task panicked; carries enough info for the agent to abandon
/// the handle.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashWaiterPanickedPayload {
    pub handle: String,
    pub cmd: String,
    pub error_message: String,
}

/// Bash error envelope (REQ-BASH-008). All tool-level failures share
/// the `error` discriminator + an `error_message`; structured fields
/// vary by error id.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "error", rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum BashErrorResponse {
    HandleNotFound {
        error_message: String,
        handle_id: String,
        hint: String,
    },
    HandleCapReached {
        error_message: String,
        cap: usize,
        live_handles: Vec<BashLiveHandleSummary>,
        hint: String,
    },
    WaitSecondsOutOfRange {
        error_message: String,
        provided: i64,
        max_wait_seconds: u64,
    },
    PeekArgsMutuallyExclusive {
        error_message: String,
    },
    CommandSafetyRejected {
        error_message: String,
        reason: String,
    },
    SpawnFailed {
        error_message: String,
    },
    /// Either zero/multiple of `cmd|peek|wait|kill`, or `mode`+`wait_seconds`,
    /// or a malformed input (REQ-BASH-010). The dispatch flattens the
    /// dual-pass case's `mode` / `wait_seconds` fields onto the top level
    /// of the JSON envelope (see `BashError::into_tool_output`); they are
    /// not modelled as typed fields here because the set of extras is
    /// open (driven by the conflict shape) and giving them `ts_rs` typing
    /// would lock the wire shape against fields the runtime adds.
    MutuallyExclusiveModes {
        error_message: String,
        conflicting_args: Vec<String>,
        recommended_action: String,
    },
}

/// One entry of the live-handle snapshot returned with `handle_cap_reached`
/// (REQ-BASH-005).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BashLiveHandleSummary {
    pub handle: String,
    pub cmd: String,
    pub age_seconds: u64,
    /// Always `"running"` today; reserved for future state-aware listings.
    pub status: String,
}

/// Tmux tool successful response (REQ-TMUX-012). The shape differs
/// deliberately from [`BashResponse`] — tmux surfaces stdout / stderr
/// separately because tmux subcommands emit structured CLI output where
/// the distinction matters (see `specs/tmux-integration/design.md`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TmuxToolResponse {
    /// `ok` (subprocess exited normally), `timed_out` (Phoenix-side
    /// `wait_seconds` expired), or `cancelled` (turn cancellation token).
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

/// Tmux tool error envelope. Stable error ids: `invalid_input`,
/// `wait_seconds_out_of_range`, `tmux_binary_unavailable`,
/// `tmux_server_unavailable`, `tmux_spawn_failed`, `tmux_wait_failed`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TmuxErrorResponse {
    pub error: String,
    pub message: String,
}

#[cfg(test)]
mod bash_tmux_wire_tests {
    use super::*;

    #[test]
    fn bash_running_serializes_with_status_tag() {
        let resp = BashResponse::Running(BashRunningPayload {
            handle: "b-1".into(),
            cmd: "ls".into(),
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 1,
                truncated_before: false,
                lines: vec![BashRingLine {
                    offset: 0,
                    bytes: "hello".into(),
                }],
            },
            kill_signal_sent: None,
            kill_attempted_at: None,
            display: "peek b-1".into(),
            signal_sent: None,
            deprecation_notice: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "running");
        assert_eq!(v["handle"], "b-1");
        assert_eq!(v["cmd"], "ls");
        assert_eq!(v["display"], "peek b-1");
        assert_eq!(v["start_offset"], 0);
        assert_eq!(v["end_offset"], 1);
        assert_eq!(v["truncated_before"], false);
        assert_eq!(v["lines"][0]["offset"], 0);
        assert_eq!(v["lines"][0]["bytes"], "hello");
    }

    #[test]
    fn bash_tombstoned_carries_final_cause_and_signal_number() {
        let resp = BashResponse::Tombstoned(BashTombstonedPayload {
            handle: "b-2".into(),
            cmd: "sleep 1".into(),
            final_cause: "killed".into(),
            exit_code: None,
            signal_number: Some(15),
            duration_ms: 1000,
            finished_at: "1700000000".into(),
            kill_signal_sent: Some("TERM".into()),
            kill_attempted_at: Some("1700000000".into()),
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 0,
                truncated_before: false,
                lines: vec![],
            },
            display: "kill b-2 (TERM)".into(),
            signal_sent: Some("TERM".into()),
            deprecation_notice: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "tombstoned");
        assert_eq!(v["final_cause"], "killed");
        assert_eq!(v["signal_number"], 15);
        assert_eq!(v["signal_sent"], "TERM");
    }

    #[test]
    fn bash_spawn_exited_status_is_exited_not_tombstoned() {
        // REQ-BASH-002: spawn responses use `exited` / `killed` directly.
        let resp = BashResponse::Exited(BashSpawnTombstonePayload {
            handle: "b-3".into(),
            cmd: "echo hi".into(),
            final_cause: "exited".into(),
            exit_code: Some(0),
            signal_number: None,
            duration_ms: 5,
            finished_at: "1700000000".into(),
            kill_signal_sent: None,
            kill_attempted_at: None,
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 1,
                truncated_before: false,
                lines: vec![BashRingLine {
                    offset: 0,
                    bytes: "hi".into(),
                }],
            },
            deprecation_notice: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "exited");
        // No display field on the spawn-tombstone shape.
        assert!(v.get("display").is_none());
    }

    #[test]
    fn bash_error_handle_cap_reached_includes_live_handles() {
        let resp = BashErrorResponse::HandleCapReached {
            error_message: "this conversation has reached the cap of 8 live bash handles".into(),
            cap: 8,
            live_handles: vec![BashLiveHandleSummary {
                handle: "b-1".into(),
                cmd: "cargo test".into(),
                age_seconds: 1820,
                status: "running".into(),
            }],
            hint: "kill or wait".into(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"], "handle_cap_reached");
        assert_eq!(v["cap"], 8);
        assert_eq!(v["live_handles"][0]["handle"], "b-1");
        assert_eq!(v["live_handles"][0]["status"], "running");
    }

    #[test]
    fn bash_error_mutually_exclusive_modes_serializes_with_error_tag() {
        let resp = BashErrorResponse::MutuallyExclusiveModes {
            error_message: "the deprecated 'mode' parameter cannot be used with 'wait_seconds'; \
                            pass wait_seconds alone"
                .into(),
            conflicting_args: vec!["mode".into(), "wait_seconds".into()],
            recommended_action: "remove the deprecated 'mode' parameter; pass 'wait_seconds' alone"
                .into(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"], "mutually_exclusive_modes");
        assert!(v["conflicting_args"].is_array());
        // Extras (`mode`, `wait_seconds`) are merged in by
        // `BashError::into_tool_output`, not by the typed struct itself.
    }

    #[test]
    fn tmux_response_carries_separate_stdout_and_stderr() {
        let resp = TmuxToolResponse {
            status: "ok".into(),
            exit_code: Some(0),
            duration_ms: 12,
            stdout: "main: 1 windows".into(),
            stderr: String::new(),
            truncated: false,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["stdout"], "main: 1 windows");
        assert_eq!(v["stderr"], "");
        assert_eq!(v["truncated"], false);
        assert_eq!(v["exit_code"], 0);
    }
}
