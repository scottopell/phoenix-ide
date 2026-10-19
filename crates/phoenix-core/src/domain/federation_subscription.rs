use serde::{Deserialize, Serialize};

use super::{
    close::TranscriptConversationId, instance_identity::InstanceId,
    product_conversation::ProductConversationId,
};

/// A sequence position, not an event identity. Zero precedes all future facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct SubscriptionCursor(i64);
impl SubscriptionCursor {
    pub const START: Self = Self(0);
    /// The committed sequence position represented by this cursor.
    #[must_use]
    pub fn get(self) -> i64 {
        self.0
    }
}
impl TryFrom<i64> for SubscriptionCursor {
    type Error = &'static str;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            Err("negative subscription cursor")
        } else {
            Ok(Self(value))
        }
    }
}
impl From<SubscriptionCursor> for i64 {
    fn from(value: SubscriptionCursor) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFactOutcome {
    Completed,
    Failed,
    Cancelled,
}
impl TerminalFactOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}
impl TryFrom<&str> for TerminalFactOutcome {
    type Error = &'static str;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err("not a terminal fact"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionReference {
    pub instance_id: InstanceId,
    pub product_conversation_id: ProductConversationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubscriptionCurrentState {
    Nonterminal,
    Terminal { outcome: TerminalFactOutcome },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionSnapshot {
    pub transcript_id: TranscriptConversationId,
    pub state: SubscriptionCurrentState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedTerminalFact {
    pub event_id: uuid::Uuid,
    pub sequence: SubscriptionCursor,
    pub source_transcript_id: TranscriptConversationId,
    pub outcome: TerminalFactOutcome,
    pub occurred_at_us: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionGap {
    pub requested_after: SubscriptionCursor,
    pub resume_after: SubscriptionCursor,
    pub snapshot: SubscriptionSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubscriptionDelivery {
    Fact { fact: FederatedTerminalFact },
    Gap { gap: SubscriptionGap },
}
impl SubscriptionDelivery {
    /// The exclusive starting cursor and inclusive ending cursor of this delivery.
    ///
    /// # Errors
    /// Returns an error for zero-sequence facts or empty/reversed delivery ranges.
    pub fn bounds(&self) -> Result<(SubscriptionCursor, SubscriptionCursor), &'static str> {
        let (after, through) = match self {
            Self::Fact { fact } => (
                fact.sequence
                    .get()
                    .checked_sub(1)
                    .ok_or("zero fact sequence")?,
                fact.sequence.get(),
            ),
            Self::Gap { gap } => (gap.requested_after.get(), gap.resume_after.get()),
        };
        if through <= after {
            return Err("empty delivery range");
        }
        Ok((after.try_into()?, through.try_into()?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubscriptionAuthorization {
    Authorized { authorization_id: uuid::Uuid },
    Rejected { reason: SubscriptionEnd },
    Refresh,
}

impl SubscriptionCurrentState {
    /// Classify terminal execution state without interpreting task or approval state.
    #[must_use]
    pub fn from_projection(
        state: &super::sm_state::ConvState,
        classified: Option<TerminalFactOutcome>,
    ) -> Self {
        use super::sm_state::ConvState;
        let outcome = match state {
            ConvState::Error { .. }
            | ConvState::CreationFailed { .. }
            | ConvState::RecoverableContinuationFailure { .. }
            | ConvState::Failed { .. }
            | ConvState::ContextExhausted { .. } => Some(TerminalFactOutcome::Failed),
            ConvState::CreationCancelled { .. } => Some(TerminalFactOutcome::Cancelled),
            ConvState::Completed { .. } => Some(TerminalFactOutcome::Completed),
            ConvState::Idle | ConvState::Terminal => classified,
            ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ServerOverloadRetrying { .. }
            | ConvState::Provisioning { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::HandedOff { .. } => None,
        };
        outcome.map_or(Self::Nonterminal, |outcome| Self::Terminal { outcome })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionEnd {
    Unsubscribed,
    Revoked,
    SourceClosed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubscriptionReplay {
    Facts { events: Vec<FederatedTerminalFact> },
    Gap { gap: SubscriptionGap },
    Ended { reason: SubscriptionEnd },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Active,
    Unsubscribed,
    Revoked,
    SourceClosed,
}
impl SubscriptionState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Unsubscribed => "unsubscribed",
            Self::Revoked => "revoked",
            Self::SourceClosed => "source_closed",
        }
    }
}
impl TryFrom<&str> for SubscriptionState {
    type Error = &'static str;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "active" => Ok(Self::Active),
            "unsubscribed" => Ok(Self::Unsubscribed),
            "revoked" => Ok(Self::Revoked),
            "source_closed" => Ok(Self::SourceClosed),
            _ => Err("invalid subscription state"),
        }
    }
}

pub const REPLAY_BATCH: usize = 16;
pub const MAX_UNACKNOWLEDGED: i64 = 1024;
pub const RETENTION_US: i64 = 30 * 24 * 60 * 60 * 1_000_000;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_terminal_wire_and_cursor_domains() {
        assert!(serde_json::from_str::<SubscriptionCursor>("-1").is_err());
        for invalid in [
            "awaiting_user_response",
            "cleanup_failed",
            "token",
            "awaiting_task_approval",
        ] {
            assert!(
                serde_json::from_value::<TerminalFactOutcome>(serde_json::json!(invalid)).is_err()
            );
        }
        assert!(serde_json::from_value::<SubscriptionReplay>(
            serde_json::json!({"kind":"facts","events":[],"token":"no"})
        )
        .is_err());
    }
}
