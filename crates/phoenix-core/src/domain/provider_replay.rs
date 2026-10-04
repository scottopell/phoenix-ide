//! Provider replay data model for active private response sets.
//!
//! These types capture the private (non-public) blocks that Anthropic embeds in
//! streaming responses — extended thinking blocks and redacted-thinking blocks —
//! so they can be replayed verbatim on the next turn. The model requires the
//! exact blocks it emitted; replaying a summarised or absent version breaks the
//! chain-of-thought context.
//!
//! Serde discipline (all types):
//! - `#[serde(deny_unknown_fields)]` — forward-incompatible JSON is a hard error
//!   rather than silent data loss.
//! - No `#[serde(default)]` — every field must be present in the JSON.
//! - No `#[serde(skip_serializing_if = …)]` — absent fields are data loss.
//! - No `#[serde(flatten)]` — structural ambiguity is forbidden.
//! - No `serde_json::Value` — opaque payloads are not allowed at any nesting depth.

use super::llm_types::ContentBlock;
use serde::{Deserialize, Serialize};

/// The index of a block within the Anthropic response's `content` array.
///
/// Anthropic requires positioned replay: each private block carries the 0-based
/// offset at which it appeared in the original response. Using a newtype instead
/// of a bare `usize` makes confusing the ordinal with an unrelated integer a
/// compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields, transparent)]
pub struct ContentIndex(pub usize);

/// A single private block from an Anthropic streaming response.
///
/// Anthropic emits two kinds of private content:
/// - `Thinking` — an extended chain-of-thought block with a cryptographic
///   signature that the API validates on replay.
/// - `RedactedThinking` — an opaque base64 blob; the model decides what to
///   redact from the visible thinking trace.
///
/// The `index` field is the position of the block in the response's `content`
/// array. Blocks are ordered by `index` within a [`AnthropicResponseSet`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnthropicPrivateBlock {
    /// An extended-thinking block. The `signature` is a provider-signed MAC;
    /// replaying a block with a tampered or absent signature causes the API to
    /// reject the request.
    Thinking {
        /// 0-based position in the response's `content` array.
        index: ContentIndex,
        /// The chain-of-thought text produced by the model.
        thinking: String,
        /// The cryptographic signature Anthropic appended to validate replay.
        signature: String,
    },
    /// An opaque redacted-thinking block. The `data` field is a base64-encoded
    /// blob; its internals are not defined by the public API.
    RedactedThinking {
        /// 0-based position in the response's `content` array.
        index: ContentIndex,
        /// The opaque redacted-thinking payload.
        data: String,
    },
}

impl AnthropicPrivateBlock {
    /// Return the position of this block within its response's `content` array.
    #[must_use]
    pub fn index(&self) -> ContentIndex {
        match self {
            Self::Thinking { index, .. } | Self::RedactedThinking { index, .. } => *index,
        }
    }
}

/// The identity of an Anthropic API response.
///
/// Both fields come directly from the Anthropic API wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicResponseIdentity {
    /// The `id` field from the Anthropic API response (e.g.
    /// `"msg_01XFDUDYJgAACzvnptvVoYEL"`).
    pub response_id: String,
    /// The model identifier reported by the API (e.g.
    /// `"claude-opus-4-5-20251101"`).
    pub model: String,
}

/// One active Anthropic response set: the response identity plus every private
/// block from that response.
///
/// A response set corresponds to a single Anthropic API response. When a
/// conversation spans multiple turns, each turn contributes at most one set;
/// only the sets for *active* (not-yet-cleared) turns are retained.
///
/// # Constructor
///
/// Use [`AnthropicResponseSet::new`]; it enforces the `private_blocks` ordering
/// invariant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicResponseSet {
    /// The response identity.
    pub identity: AnthropicResponseIdentity,
    /// Phoenix request/message identity assigned after the response is admitted.
    pub owner_message_id: String,
    /// Exact public content persisted/broadcast for this assistant
    /// response. Private ordinals are validated against this sequence.
    pub public_content: Vec<ContentBlock>,
    /// All private blocks from this response, in ascending `index` order.
    pub private_blocks: Vec<AnthropicPrivateBlock>,
}

/// Error returned by [`AnthropicResponseSet::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnthropicResponseSetError {
    /// The response identity's `response_id` field is empty.
    EmptyResponseId,
    /// The response identity's `model` field is empty.
    EmptyModel,
    /// A private block index cannot be reconstructed against public content.
    BlockIndexOutOfRange(ContentIndex),
    /// A thinking block has no provider signature.
    MissingThinkingSignature(ContentIndex),
    /// Two or more private blocks share the same `index`.
    DuplicateBlockIndex(ContentIndex),
    /// The private blocks were supplied in non-ascending `index` order.
    BlocksOutOfOrder {
        /// The offending block's index (smaller than the preceding one).
        index: ContentIndex,
    },
}

impl std::fmt::Display for AnthropicResponseSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyResponseId => f.write_str("response_id must not be empty"),
            Self::EmptyModel => f.write_str("model must not be empty"),
            Self::BlockIndexOutOfRange(index) => write!(
                f,
                "private block index {} is outside reconstructed response",
                index.0
            ),
            Self::MissingThinkingSignature(index) => {
                write!(f, "thinking block at index {} has no signature", index.0)
            }
            Self::DuplicateBlockIndex(idx) => {
                write!(f, "duplicate block index {}", idx.0)
            }
            Self::BlocksOutOfOrder { index } => {
                write!(f, "blocks out of order at index {}", index.0)
            }
        }
    }
}

impl std::error::Error for AnthropicResponseSetError {}

impl AnthropicResponseSet {
    /// Construct a validated [`AnthropicResponseSet`].
    ///
    /// # Errors
    ///
    /// - [`AnthropicResponseSetError::EmptyResponseId`] — `identity.response_id` is empty.
    /// - [`AnthropicResponseSetError::EmptyModel`] — `identity.model` is empty.
    /// - [`AnthropicResponseSetError::DuplicateBlockIndex`] — two blocks share an index.
    /// - [`AnthropicResponseSetError::BlocksOutOfOrder`] — blocks not in ascending index order.
    pub fn new(
        identity: AnthropicResponseIdentity,
        private_blocks: Vec<AnthropicPrivateBlock>,
    ) -> Result<Self, AnthropicResponseSetError> {
        Self::with_public_content(identity, Vec::new(), private_blocks)
    }

    /// Construct a response set with the exact public projection used to
    /// validate/rebuild provider block ordinals.
    ///
    /// # Errors
    /// Returns [`AnthropicResponseSetError`] when identity, signature, ordinal,
    /// or ordering invariants do not hold.
    pub fn with_public_content(
        identity: AnthropicResponseIdentity,
        public_content: Vec<ContentBlock>,
        private_blocks: Vec<AnthropicPrivateBlock>,
    ) -> Result<Self, AnthropicResponseSetError> {
        if identity.response_id.is_empty() {
            return Err(AnthropicResponseSetError::EmptyResponseId);
        }
        if identity.model.is_empty() {
            return Err(AnthropicResponseSetError::EmptyModel);
        }
        // Validate ascending, duplicate-free indices.
        let mut prev: Option<ContentIndex> = None;
        for block in &private_blocks {
            let idx = block.index();
            if matches!(block, AnthropicPrivateBlock::Thinking { signature, .. } if signature.is_empty())
            {
                return Err(AnthropicResponseSetError::MissingThinkingSignature(idx));
            }
            let inserted_before = prev.map_or(0, |_| {
                private_blocks
                    .iter()
                    .take_while(|candidate| candidate.index() < idx)
                    .count()
            });
            if !public_content.is_empty() && idx.0 > public_content.len() + inserted_before {
                return Err(AnthropicResponseSetError::BlockIndexOutOfRange(idx));
            }
            if let Some(p) = prev {
                if idx == p {
                    return Err(AnthropicResponseSetError::DuplicateBlockIndex(idx));
                }
                if idx < p {
                    return Err(AnthropicResponseSetError::BlocksOutOfOrder { index: idx });
                }
            }
            prev = Some(idx);
        }
        Ok(Self {
            identity,
            owner_message_id: String::new(),
            public_content,
            private_blocks,
        })
    }

    /// Bind the provider response to Phoenix's durable request/message identity.
    #[must_use]
    pub fn with_owner_message_id(mut self, owner_message_id: String) -> Self {
        self.owner_message_id = owner_message_id;
        self
    }

    /// Re-run semantic invariants after decoding persisted replay state.
    ///
    /// # Errors
    /// Returns [`AnthropicResponseSetError`] when persisted identity, signature,
    /// ordinal, or ordering data is invalid.
    pub fn validate(&self) -> Result<(), AnthropicResponseSetError> {
        Self::with_public_content(
            self.identity.clone(),
            self.public_content.clone(),
            self.private_blocks.clone(),
        )?;
        Ok(())
    }
}

/// Replay payload for all active Anthropic response sets in a conversation.
///
/// Earned blob: this aggregate is always read and written as one unit; it is
/// never addressed field-wise by SQL. The `response_sets` field is ordered by
/// appearance in the conversation transcript — the oldest turn's set first.
///
/// # Constructor
///
/// Use [`AnthropicReplayPayload::new`]; it validates the invariant that no two
/// sets share the same `response_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicReplayPayload {
    /// All active response sets, in transcript order (oldest first).
    pub response_sets: Vec<AnthropicResponseSet>,
}

/// Error returned by [`AnthropicReplayPayload::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnthropicReplayPayloadError {
    /// A decoded response set violates semantic replay invariants.
    InvalidResponseSet(String),
    /// Two or more response sets share the same `response_id`.
    DuplicateResponseId(String),
}

impl std::fmt::Display for AnthropicReplayPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidResponseSet(message) => f.write_str(message),
            Self::DuplicateResponseId(id) => {
                write!(f, "duplicate response_id in payload: {id}")
            }
        }
    }
}

impl std::error::Error for AnthropicReplayPayloadError {}

impl AnthropicReplayPayload {
    /// Construct a validated [`AnthropicReplayPayload`].
    ///
    /// # Errors
    ///
    /// [`AnthropicReplayPayloadError::DuplicateResponseId`] — two sets share the
    /// same `response_id`.
    pub fn new(
        response_sets: Vec<AnthropicResponseSet>,
    ) -> Result<Self, AnthropicReplayPayloadError> {
        let mut seen = std::collections::HashSet::new();
        for set in &response_sets {
            let id = &set.identity.response_id;
            if !seen.insert(id.as_str()) {
                return Err(AnthropicReplayPayloadError::DuplicateResponseId(id.clone()));
            }
        }
        for set in &response_sets {
            set.validate().map_err(|error| {
                AnthropicReplayPayloadError::InvalidResponseSet(error.to_string())
            })?;
        }
        Ok(Self { response_sets })
    }
}

/// Replay disposition applied by an authoritative turn-settlement transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderReplaySettlement {
    Preserve,
    Clear { conversation_id: String },
}

impl ProviderReplaySettlement {
    #[must_use]
    pub fn for_conversation_state(
        conversation_id: &str,
        state: &super::sm_state::ConvState,
    ) -> Self {
        if matches!(
            state,
            super::sm_state::ConvState::Error { .. }
                | super::sm_state::ConvState::RecoverableContinuationFailure { .. }
                | super::sm_state::ConvState::AwaitingRecovery { .. }
        ) {
            Self::Preserve
        } else {
            Self::Clear {
                conversation_id: conversation_id.to_string(),
            }
        }
    }
}

/// Private replay update returned beside public provider content. Runtime
/// admission decides whether this may mutate durable replay state.
#[derive(Debug, Clone, PartialEq)]
pub enum AnthropicReplayUpdate {
    Append(AnthropicResponseSet),
    Clear,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(response_id: &str, model: &str) -> AnthropicResponseIdentity {
        AnthropicResponseIdentity {
            response_id: response_id.into(),
            model: model.into(),
        }
    }

    fn thinking_block(index: usize) -> AnthropicPrivateBlock {
        AnthropicPrivateBlock::Thinking {
            index: ContentIndex(index),
            thinking: format!("thought at {index}"),
            signature: format!("sig-{index}"),
        }
    }

    fn redacted_block(index: usize) -> AnthropicPrivateBlock {
        AnthropicPrivateBlock::RedactedThinking {
            index: ContentIndex(index),
            data: format!("blob-{index}"),
        }
    }

    // ── AnthropicResponseSet::new ────────────────────────────────────────────

    #[test]
    fn set_new_ok_empty_blocks() {
        let set = AnthropicResponseSet::new(identity("msg-1", "claude-opus"), vec![]).unwrap();
        assert!(set.private_blocks.is_empty());
    }

    #[test]
    fn set_new_ok_ascending_blocks() {
        let set = AnthropicResponseSet::new(
            identity("msg-1", "claude-opus"),
            vec![thinking_block(0), redacted_block(2), thinking_block(4)],
        )
        .unwrap();
        assert_eq!(set.private_blocks.len(), 3);
    }

    #[test]
    fn set_new_rejects_empty_response_id() {
        let err = AnthropicResponseSet::new(identity("", "claude-opus"), vec![]).unwrap_err();
        assert_eq!(err, AnthropicResponseSetError::EmptyResponseId);
    }

    #[test]
    fn set_new_rejects_empty_model() {
        let err = AnthropicResponseSet::new(identity("msg-1", ""), vec![]).unwrap_err();
        assert_eq!(err, AnthropicResponseSetError::EmptyModel);
    }

    #[test]
    fn set_new_rejects_ordinal_impossible_at_reconstruction_step() {
        let result = AnthropicResponseSet::with_public_content(
            identity("resp", "claude-opus-5-5"),
            vec![ContentBlock::text("public")],
            vec![AnthropicPrivateBlock::Thinking {
                index: ContentIndex(2),
                thinking: String::new(),
                signature: "sig".into(),
            }],
        );
        assert_eq!(
            result.unwrap_err(),
            AnthropicResponseSetError::BlockIndexOutOfRange(ContentIndex(2))
        );
    }

    #[test]
    fn set_new_rejects_duplicate_index() {
        let err = AnthropicResponseSet::new(
            identity("msg-1", "claude-opus"),
            vec![thinking_block(1), redacted_block(1)],
        )
        .unwrap_err();
        assert_eq!(
            err,
            AnthropicResponseSetError::DuplicateBlockIndex(ContentIndex(1))
        );
    }

    #[test]
    fn set_new_rejects_out_of_order_blocks() {
        let err = AnthropicResponseSet::new(
            identity("msg-1", "claude-opus"),
            vec![thinking_block(3), thinking_block(1)],
        )
        .unwrap_err();
        assert_eq!(
            err,
            AnthropicResponseSetError::BlocksOutOfOrder {
                index: ContentIndex(1)
            }
        );
    }

    // ── AnthropicReplayPayload::new ──────────────────────────────────────────

    #[test]
    fn payload_new_ok_empty() {
        let p = AnthropicReplayPayload::new(vec![]).unwrap();
        assert!(p.response_sets.is_empty());
    }

    #[test]
    fn payload_new_ok_multiple_sets() {
        let s1 = AnthropicResponseSet::new(identity("msg-1", "claude-opus"), vec![]).unwrap();
        let s2 = AnthropicResponseSet::new(identity("msg-2", "claude-opus"), vec![]).unwrap();
        let p = AnthropicReplayPayload::new(vec![s1, s2]).unwrap();
        assert_eq!(p.response_sets.len(), 2);
    }

    #[test]
    fn payload_new_rejects_duplicate_response_id() {
        let s1 = AnthropicResponseSet::new(identity("msg-dup", "claude-opus"), vec![]).unwrap();
        let s2 = AnthropicResponseSet::new(identity("msg-dup", "claude-opus"), vec![]).unwrap();
        let err = AnthropicReplayPayload::new(vec![s1, s2]).unwrap_err();
        assert_eq!(
            err,
            AnthropicReplayPayloadError::DuplicateResponseId("msg-dup".into())
        );
    }

    // ── Serde round-trip ─────────────────────────────────────────────────────

    #[test]
    fn payload_round_trips_through_json() {
        let payload = AnthropicReplayPayload::new(vec![
            AnthropicResponseSet::new(
                identity("msg-abc", "claude-opus-4-5"),
                vec![thinking_block(0), redacted_block(2)],
            )
            .unwrap(),
            AnthropicResponseSet::new(identity("msg-def", "claude-opus-4-5"), vec![]).unwrap(),
        ])
        .unwrap();
        let json = serde_json::to_string(&payload).unwrap();
        let decoded: AnthropicReplayPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, decoded);
    }

    #[test]
    fn deny_unknown_fields_rejects_extra_top_level_key() {
        let json = r#"{"response_sets":[], "extra":"field"}"#;
        assert!(serde_json::from_str::<AnthropicReplayPayload>(json).is_err());
    }

    #[test]
    fn deny_unknown_fields_rejects_extra_block_key() {
        let json = r#"{"response_sets":[{"identity":{"response_id":"x","model":"m"},"private_blocks":[{"type":"thinking","index":0,"thinking":"t","signature":"s","extra":"bad"}]}]}"#;
        assert!(serde_json::from_str::<AnthropicReplayPayload>(json).is_err());
    }

    #[test]
    fn missing_required_field_is_an_error() {
        // `thinking` field missing from thinking block
        let json = r#"{"response_sets":[{"identity":{"response_id":"x","model":"m"},"private_blocks":[{"type":"thinking","index":0,"signature":"s"}]}]}"#;
        assert!(serde_json::from_str::<AnthropicReplayPayload>(json).is_err());
    }
}
