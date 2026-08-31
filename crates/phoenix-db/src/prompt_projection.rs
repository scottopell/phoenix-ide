//! Typed transfer boundary for provider-bound durable prompt projections.

use phoenix_core::domain::db_schema::Message;

use crate::{DbError, DbResult};

/// Generation fence for one durable prompt transcript projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptTranscriptGeneration(i64);

impl PromptTranscriptGeneration {
    /// Construct a generation read from the authoritative conversations row.
    ///
    /// # Errors
    ///
    /// Returns an error when the persisted generation is not positive.
    pub fn from_persisted(value: i64) -> DbResult<Self> {
        if value < 1 {
            return Err(DbError::Serialization(format!(
                "prompt transcript generation must be positive, got {value}"
            )));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

/// Greatest persisted message sequence observed by a prompt projection.
/// Sequence gaps are legal; only strict increase matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PersistedMessageSequence(i64);

impl PersistedMessageSequence {
    /// Construct a cursor read from authoritative persisted rows.
    ///
    /// # Errors
    ///
    /// Returns an error when the persisted message sequence is negative.
    pub fn from_persisted(value: i64) -> DbResult<Self> {
        if value < 0 {
            return Err(DbError::Serialization(format!(
                "persisted message cursor must be non-negative, got {value}"
            )));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

/// Prompt projection boundary within one transcript generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptProjectionPosition {
    #[default]
    Empty,
    At(PersistedMessageSequence),
}

/// Tail boundary that cannot be separated from its transcript generation fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationFencedPromptPosition {
    generation: PromptTranscriptGeneration,
    position: PromptProjectionPosition,
}

impl GenerationFencedPromptPosition {
    #[must_use]
    pub const fn new(
        generation: PromptTranscriptGeneration,
        position: PromptProjectionPosition,
    ) -> Self {
        Self {
            generation,
            position,
        }
    }

    #[must_use]
    pub const fn generation(self) -> PromptTranscriptGeneration {
        self.generation
    }

    #[must_use]
    pub const fn position(self) -> PromptProjectionPosition {
        self.position
    }
}

/// Fully hydrated, transactionally consistent durable prompt transcript.
#[derive(Debug, Clone)]
pub struct HydratedPromptSnapshot {
    generation: PromptTranscriptGeneration,
    position: PromptProjectionPosition,
    messages: Vec<Message>,
}

impl HydratedPromptSnapshot {
    /// Validated constructor retained for alternate `MessageStore` implementations.
    ///
    /// # Errors
    ///
    /// Returns an error when message ownership or ordering contradicts the
    /// requested conversation snapshot.
    pub fn try_new(
        conversation_id: &str,
        generation: PromptTranscriptGeneration,
        messages: Vec<Message>,
    ) -> DbResult<Self> {
        let position = validate_prompt_rows(conversation_id, None, &messages)?;
        Ok(Self {
            generation,
            position,
            messages,
        })
    }

    #[must_use]
    pub const fn generation(&self) -> PromptTranscriptGeneration {
        self.generation
    }

    #[must_use]
    pub const fn position(&self) -> PromptProjectionPosition {
        self.position
    }

    #[must_use]
    pub const fn fenced_position(&self) -> GenerationFencedPromptPosition {
        GenerationFencedPromptPosition::new(self.generation, self.position)
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        PromptTranscriptGeneration,
        PromptProjectionPosition,
        Vec<Message>,
    ) {
        (self.generation, self.position, self.messages)
    }
}

/// Generation-fenced, fully hydrated durable transcript tail.
#[derive(Debug, Clone)]
pub enum HydratedPromptTail {
    Current(HydratedPromptTailRows),
    Invalidated(PromptTranscriptGeneration),
}

/// Validated tail rows. Private fields prevent alternate stores from forging a
/// contradictory position or bypassing conversation/ordering checks.
#[derive(Debug, Clone)]
pub struct HydratedPromptTailRows {
    position: PromptProjectionPosition,
    messages: Vec<Message>,
}

impl HydratedPromptTailRows {
    #[must_use]
    pub const fn position(&self) -> PromptProjectionPosition {
        self.position
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn into_parts(self) -> (PromptProjectionPosition, Vec<Message>) {
        (self.position, self.messages)
    }
}

impl HydratedPromptTail {
    /// Construct and validate a current tail from an alternate store.
    ///
    /// # Errors
    ///
    /// Returns an error when tail ownership or ordering does not strictly
    /// advance beyond `after`.
    pub fn try_current(
        conversation_id: &str,
        after: PromptProjectionPosition,
        messages: Vec<Message>,
    ) -> DbResult<Self> {
        let position = validate_prompt_rows(conversation_id, Some(after), &messages)?;
        Ok(Self::Current(HydratedPromptTailRows { position, messages }))
    }

    #[must_use]
    pub const fn invalidated(current_generation: PromptTranscriptGeneration) -> Self {
        Self::Invalidated(current_generation)
    }
}

pub(crate) fn validate_prompt_rows(
    conversation_id: &str,
    after: Option<PromptProjectionPosition>,
    messages: &[Message],
) -> DbResult<PromptProjectionPosition> {
    let mut position = after.unwrap_or(PromptProjectionPosition::Empty);
    for message in messages {
        if message.conversation_id != conversation_id {
            return Err(DbError::Serialization(format!(
                "prompt transcript row {} belongs to {}, expected {conversation_id}",
                message.message_id, message.conversation_id
            )));
        }
        let sequence = PersistedMessageSequence::from_persisted(message.sequence_id)?;
        match position {
            PromptProjectionPosition::At(cursor) if message.sequence_id <= cursor.value() => {
                return Err(DbError::Serialization(format!(
                    "prompt transcript sequence {} is not greater than cursor {}",
                    message.sequence_id,
                    cursor.value()
                )));
            }
            PromptProjectionPosition::Empty | PromptProjectionPosition::At(_) => {}
        }
        position = PromptProjectionPosition::At(sequence);
    }
    Ok(position)
}
