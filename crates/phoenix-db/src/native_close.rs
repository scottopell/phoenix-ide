//! Typed native Close request and canonical source/intent correlation.

use std::fmt::Write as _;

use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use sha2::{Digest, Sha256};
use uuid::Uuid;

// A fixed namespace and framing version keep this request identity separate from other UUID-v5 uses.
const REQUEST_NAMESPACE: Uuid = Uuid::from_bytes([
    0x7c, 0x4c, 0x67, 0x9a, 0xf0, 0x76, 0x46, 0x3f, 0xa5, 0x53, 0xe2, 0x6b, 0x74, 0x0b, 0x7f, 0x13,
]);
const REQUEST_DOMAIN: &[u8] = b"phoenix.native_close.request.v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"phoenix.native_close.intent.v1\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeCloseIntent {
    Initial { target: ProductConversationId },
    Observe {
        target: ProductConversationId,
        attempt_id: CloseAttemptId,
        run_ordinal: CloseRunOrdinal,
    },
}

impl NativeCloseIntent {
    #[must_use]
    pub fn target(&self) -> &ProductConversationId {
        match self {
            Self::Initial { target } | Self::Observe { target, .. } => target,
        }
    }

    /// The exact source occurrence matched by both the actor and durable verifier.
    #[must_use]
    pub fn source_input(&self) -> serde_json::Value {
        match self {
            Self::Initial { target } => serde_json::json!({
                "action": "initial", "product_conversation_id": target.as_str()
            }),
            Self::Observe { target, attempt_id, run_ordinal } => serde_json::json!({
                "action": "observe", "product_conversation_id": target.as_str(),
                "attempt_id": attempt_id.as_str(), "run_ordinal": run_ordinal.get()
            }),
        }
    }

    fn receipt_intent(&self) -> &'static str {
        match self {
            Self::Initial { .. } => "initial",
            Self::Observe { .. } => "observe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NativeCloseCommandError {
    #[error("native Close source transcript, message and tool-use IDs must be nonempty")]
    EmptySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseCommand {
    source_transcript_id: String,
    source: SourceToolCall,
    intent: NativeCloseIntent,
    request_id: Uuid,
    fingerprint: String,
}

impl NativeCloseCommand {
    /// # Errors
    /// Rejects empty source identity fields. Source correlation is not live authority;
    /// the consumer must validate current source and target eligibility separately.
    pub fn new(
        source_transcript_id: String,
        source: SourceToolCall,
        intent: NativeCloseIntent,
    ) -> Result<Self, NativeCloseCommandError> {
        if source_transcript_id.trim().is_empty()
            || source.message_id.trim().is_empty()
            || source.tool_use_id.trim().is_empty()
        {
            return Err(NativeCloseCommandError::EmptySource);
        }

        let mut name = REQUEST_DOMAIN.to_vec();
        append_field(&mut name, source_transcript_id.as_bytes());
        append_field(&mut name, source.message_id.as_bytes());
        append_field(&mut name, source.tool_use_id.as_bytes());
        let request_id = Uuid::new_v5(&REQUEST_NAMESPACE, &name);

        let mut canonical = FINGERPRINT_DOMAIN.to_vec();
        match &intent {
            NativeCloseIntent::Initial { target } => {
                append_field(&mut canonical, b"initial");
                append_field(&mut canonical, target.as_str().as_bytes());
            }
            NativeCloseIntent::Observe { target, attempt_id, run_ordinal } => {
                append_field(&mut canonical, b"observe");
                append_field(&mut canonical, target.as_str().as_bytes());
                append_field(&mut canonical, attempt_id.as_str().as_bytes());
                append_field(&mut canonical, &run_ordinal.get().to_be_bytes());
            }
        }
        let fingerprint = Sha256::digest(&canonical).iter().fold(
            String::with_capacity(64),
            |mut output, byte| {
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            },
        );

        Ok(Self {
            source_transcript_id,
            source,
            intent,
            request_id,
            fingerprint,
        })
    }

    /// Historical source correlation only; this does not confer live authority.
    #[must_use]
    pub fn source_transcript_id(&self) -> &str {
        &self.source_transcript_id
    }

    /// Historical source correlation only; this does not confer live authority.
    #[must_use]
    pub fn source(&self) -> &SourceToolCall {
        &self.source
    }

    #[must_use]
    pub fn intent(&self) -> &NativeCloseIntent {
        &self.intent
    }

    #[must_use]
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

mod initial;
pub use initial::{NativeCloseInitialResult, NativeCloseReceipt, NativeCloseRunRef};
mod observe;
mod receipts;
pub use observe::{NativeCloseObservation, NativeCloseObserveReceipt, NativeCloseObserveResult};

#[cfg(test)]
mod tests;
