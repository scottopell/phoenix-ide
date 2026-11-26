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
    Initial {
        target: ProductConversationId,
    },
    Observe {
        target: ProductConversationId,
        attempt_id: CloseAttemptId,
        run_ordinal: CloseRunOrdinal,
    },
    ExplicitRetry {
        target: ProductConversationId,
        attempt_id: CloseAttemptId,
        failed_run_ordinal: CloseRunOrdinal,
    },
}

impl NativeCloseIntent {
    #[must_use]
    pub fn target(&self) -> &ProductConversationId {
        match self {
            Self::Initial { target }
            | Self::Observe { target, .. }
            | Self::ExplicitRetry { target, .. } => target,
        }
    }

    /// The canonical wrapped input emitted for new native Close occurrences.
    #[must_use]
    pub fn source_input(&self) -> serde_json::Value {
        serde_json::json!({"request": self.legacy_source_input()})
    }

    /// Matches only a complete canonical input or a complete historical flat input.
    #[must_use]
    pub fn matches_source_input(&self, input: &serde_json::Value) -> bool {
        input == &self.source_input() || input == &self.legacy_source_input()
    }

    fn legacy_source_input(&self) -> serde_json::Value {
        match self {
            Self::Initial { target } => serde_json::json!({
                "action": "initial", "product_conversation_id": target.as_str()
            }),
            Self::Observe {
                target,
                attempt_id,
                run_ordinal,
            } => serde_json::json!({
                "action": "observe", "product_conversation_id": target.as_str(),
                "attempt_id": attempt_id.as_str(), "run_ordinal": run_ordinal.get()
            }),
            Self::ExplicitRetry {
                target,
                attempt_id,
                failed_run_ordinal,
            } => serde_json::json!({
                "action": "explicit_retry", "product_conversation_id": target.as_str(),
                "attempt_id": attempt_id.as_str(), "failed_run_ordinal": failed_run_ordinal.get()
            }),
        }
    }

    fn receipt_intent(&self) -> &'static str {
        match self {
            Self::Initial { .. } => "initial",
            Self::Observe { .. } => "observe",
            Self::ExplicitRetry { .. } => "explicit_retry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NativeCloseCommandError {
    #[error("native Close source transcript, message and tool-use IDs must be nonempty")]
    EmptySource,
    #[error("native Close source input is not an exact supported canonical shape")]
    InvalidSourceInput,
    #[error("native Close retained fingerprint does not match a supported wire command")]
    InvalidRetainedFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeCloseSourceShape {
    Wrapped,
    LegacyFlat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseCommand {
    source_transcript_id: String,
    source: SourceToolCall,
    intent: NativeCloseIntent,
    source_shape: NativeCloseSourceShape,
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
        Self::new_with_shape(
            source_transcript_id,
            source,
            intent,
            NativeCloseSourceShape::Wrapped,
        )
    }

    /// Binds authority to exactly the canonical wrapped or historical flat input supplied.
    ///
    /// # Errors
    /// Rejects unsupported inputs or empty source identity fields.
    pub fn new_with_source_input(
        source_transcript_id: String,
        source: SourceToolCall,
        intent: NativeCloseIntent,
        input: &serde_json::Value,
    ) -> Result<Self, NativeCloseCommandError> {
        let shape = if input == &intent.source_input() {
            NativeCloseSourceShape::Wrapped
        } else if input == &intent.legacy_source_input() {
            NativeCloseSourceShape::LegacyFlat
        } else {
            return Err(NativeCloseCommandError::InvalidSourceInput);
        };
        Self::new_with_shape(source_transcript_id, source, intent, shape)
    }

    /// Reconstructs the original wire shape using retained authority, never current state.
    ///
    /// # Errors
    /// Rejects unknown fingerprints or empty source identity fields.
    pub fn new_from_retained_fingerprint(
        source_transcript_id: &str,
        source: &SourceToolCall,
        intent: &NativeCloseIntent,
        fingerprint: &str,
    ) -> Result<Self, NativeCloseCommandError> {
        for shape in [
            NativeCloseSourceShape::Wrapped,
            NativeCloseSourceShape::LegacyFlat,
        ] {
            let command = Self::new_with_shape(
                source_transcript_id.to_owned(),
                source.clone(),
                intent.clone(),
                shape,
            )?;
            if command.fingerprint() == fingerprint {
                return Ok(command);
            }
        }
        Err(NativeCloseCommandError::InvalidRetainedFingerprint)
    }

    fn new_with_shape(
        source_transcript_id: String,
        source: SourceToolCall,
        intent: NativeCloseIntent,
        source_shape: NativeCloseSourceShape,
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
        // Flat commands retain the exact historical v1 bytes. Wrapped commands
        // are domain-separated so the saved fingerprint also binds the wire shape.
        if source_shape == NativeCloseSourceShape::Wrapped {
            append_field(&mut canonical, b"wire.request.wrapped.v1");
        }
        match &intent {
            NativeCloseIntent::Initial { target } => {
                append_field(&mut canonical, b"initial");
                append_field(&mut canonical, target.as_str().as_bytes());
            }
            NativeCloseIntent::Observe {
                target,
                attempt_id,
                run_ordinal,
            } => {
                append_field(&mut canonical, b"observe");
                append_field(&mut canonical, target.as_str().as_bytes());
                append_field(&mut canonical, attempt_id.as_str().as_bytes());
                append_field(&mut canonical, &run_ordinal.get().to_be_bytes());
            }
            NativeCloseIntent::ExplicitRetry {
                target,
                attempt_id,
                failed_run_ordinal,
            } => {
                append_field(&mut canonical, b"explicit_retry");
                append_field(&mut canonical, target.as_str().as_bytes());
                append_field(&mut canonical, attempt_id.as_str().as_bytes());
                append_field(&mut canonical, &failed_run_ordinal.get().to_be_bytes());
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
            source_shape,
            request_id,
            fingerprint,
        })
    }

    #[must_use]
    pub fn source_input(&self) -> serde_json::Value {
        match self.source_shape {
            NativeCloseSourceShape::Wrapped => self.intent.source_input(),
            NativeCloseSourceShape::LegacyFlat => self.intent.legacy_source_input(),
        }
    }

    #[must_use]
    pub fn matches_source_input(&self, input: &serde_json::Value) -> bool {
        input == &self.source_input()
    }

    /// # Errors
    /// Refuses unless both retained occurrences match this command's single wire shape.
    pub fn verify_executing_source(
        &self,
        state: &phoenix_core::domain::sm_state::ConvState,
        source: &SourceToolCall,
    ) -> crate::DbResult<()> {
        crate::executing_source::require_executing_source(
            state,
            source,
            "close_conversation",
            &self.source_input(),
        )
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

mod captured_loss_retry;
pub use captured_loss_retry::{NativeCloseCapturedLossRetryContext, NativeCloseRetryOriginKind};
mod captured_loss_admission;
pub use captured_loss_admission::NativeCloseCapturedLossRetryAssessment;

#[cfg(test)]
mod completion;
#[cfg(test)]
pub(crate) use completion::NativeCloseCompletionResult;

mod initial;
mod initial_exit;
pub use initial::{
    NativeCloseInitialEntryRelease, NativeCloseInitialExecution,
    NativeCloseInitialFirstEffectError, NativeCloseInitialProcessAuthorization,
    NativeCloseInitialResourceEntryDecision, NativeCloseInitialResourceEntryResult,
    NativeCloseInitialResult, NativeCloseInitialSettlement, NativeCloseReceipt, NativeCloseRunRef,
};
mod observe;
mod receipts;
mod retry;
mod retry_execution;
mod source_fence;
pub use observe::{NativeCloseObservation, NativeCloseObserveReceipt, NativeCloseObserveResult};
pub use retry::{NativeCloseRetryResult, NativeCloseRetrySourceAssessment};
pub use retry_execution::{NativeCloseRetryExecutionContext, NativeCloseRetryPreAdmissionContext};
pub(crate) use source_fence::{
    acknowledge_initial_result_tx, fence_native_retry_sources_tx, interrupt_native_retry_tx,
    recover_native_retry_sources_tx, NativeInitialSourceFence, NativeRetrySourceFence,
};

#[cfg(test)]
pub(crate) use source_fence::{fence_pre_completion_native_sources_tx, NativeSourceLossCause};

#[cfg(test)]
mod tests;
