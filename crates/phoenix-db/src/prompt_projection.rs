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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
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

/// Fully hydrated, transactionally consistent durable prompt transcript.
#[derive(Debug, Clone)]
pub struct HydratedPromptSnapshot {
    generation: PromptTranscriptGeneration,
    cursor: PersistedMessageSequence,
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
        let cursor = validate_prompt_rows(conversation_id, None, &messages)?;
        Ok(Self {
            generation,
            cursor,
            messages,
        })
    }

    #[must_use]
    pub const fn generation(&self) -> PromptTranscriptGeneration {
        self.generation
    }

    #[must_use]
    pub const fn cursor(&self) -> PersistedMessageSequence {
        self.cursor
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
        PersistedMessageSequence,
        Vec<Message>,
    ) {
        (self.generation, self.cursor, self.messages)
    }
}

/// Generation-fenced, fully hydrated durable transcript tail.
#[derive(Debug, Clone)]
pub enum HydratedPromptTail {
    Current(HydratedPromptTailRows),
    Invalidated(PromptTranscriptGeneration),
}

/// Validated tail rows. Private fields prevent alternate stores from forging a
/// contradictory cursor or bypassing conversation/ordering checks.
#[derive(Debug, Clone)]
pub struct HydratedPromptTailRows {
    cursor: PersistedMessageSequence,
    messages: Vec<Message>,
}

impl HydratedPromptTailRows {
    #[must_use]
    pub const fn cursor(&self) -> PersistedMessageSequence {
        self.cursor
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn into_parts(self) -> (PersistedMessageSequence, Vec<Message>) {
        (self.cursor, self.messages)
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
        after: PersistedMessageSequence,
        messages: Vec<Message>,
    ) -> DbResult<Self> {
        let cursor = validate_prompt_rows(conversation_id, Some(after), &messages)?;
        Ok(Self::Current(HydratedPromptTailRows { cursor, messages }))
    }

    #[must_use]
    pub const fn invalidated(current_generation: PromptTranscriptGeneration) -> Self {
        Self::Invalidated(current_generation)
    }
}

pub(crate) fn validate_prompt_rows(
    conversation_id: &str,
    after: Option<PersistedMessageSequence>,
    messages: &[Message],
) -> DbResult<PersistedMessageSequence> {
    let mut cursor = after.unwrap_or_default();
    for message in messages {
        if message.conversation_id != conversation_id {
            return Err(DbError::Serialization(format!(
                "prompt transcript row {} belongs to {}, expected {conversation_id}",
                message.message_id, message.conversation_id
            )));
        }
        if message.sequence_id <= cursor.value() {
            return Err(DbError::Serialization(format!(
                "prompt transcript sequence {} is not greater than cursor {}",
                message.sequence_id,
                cursor.value()
            )));
        }
        cursor = PersistedMessageSequence::from_persisted(message.sequence_id)?;
    }
    Ok(cursor)
}
