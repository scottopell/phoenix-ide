use std::fmt;

use serde::{Deserialize, Serialize};

use super::close::TranscriptConversationId;
use super::instance_identity::InstanceId;
use super::product_conversation::ProductConversationId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct MessageId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("message id cannot be empty")]
pub struct MessageIdError;

impl MessageId {
    /// # Errors
    /// Returns [`MessageIdError`] when the supplied identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, MessageIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(MessageIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for MessageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum ConversationReferenceTarget {
    ProductConversation {
        product_conversation_id: ProductConversationId,
    },
    Transcript {
        transcript_id: TranscriptConversationId,
    },
    Message {
        transcript_id: TranscriptConversationId,
        message_id: MessageId,
    },
    Coordinator,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct QualifiedConversationReference {
    pub instance_id: InstanceId,
    pub target: ConversationReferenceTarget,
}

impl QualifiedConversationReference {
    #[must_use]
    pub fn new(instance_id: InstanceId, target: ConversationReferenceTarget) -> Self {
        Self {
            instance_id,
            target,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use serde_json::json;

    use super::{
        ConversationReferenceTarget, MessageId, QualifiedConversationReference,
        TranscriptConversationId,
    };
    use crate::domain::instance_identity::InstanceId;
    use crate::domain::product_conversation::ProductConversationId;

    fn instance(value: &str) -> InstanceId {
        InstanceId::from_str(value).unwrap()
    }

    fn round_trip(target: ConversationReferenceTarget) {
        let reference = QualifiedConversationReference::new(
            instance("a48ea5c2-37cc-42d5-a1d7-fac1d7765d4e"),
            target,
        );
        let encoded = serde_json::to_value(&reference).unwrap();
        assert_eq!(
            serde_json::from_value::<QualifiedConversationReference>(encoded).unwrap(),
            reference
        );
    }

    #[test]
    fn every_target_round_trips_without_losing_identity() {
        round_trip(ConversationReferenceTarget::ProductConversation {
            product_conversation_id: ProductConversationId::parse("product:opaque").unwrap(),
        });
        round_trip(ConversationReferenceTarget::Transcript {
            transcript_id: TranscriptConversationId::parse("transcript:opaque").unwrap(),
        });
        round_trip(ConversationReferenceTarget::Message {
            transcript_id: TranscriptConversationId::parse("transcript:opaque").unwrap(),
            message_id: MessageId::parse("message:opaque").unwrap(),
        });
        round_trip(ConversationReferenceTarget::Coordinator);
    }

    #[test]
    fn same_local_target_on_two_instances_remains_distinct() {
        let target = ConversationReferenceTarget::Transcript {
            transcript_id: TranscriptConversationId::parse("same-local-id").unwrap(),
        };
        let left = QualifiedConversationReference::new(
            instance("a48ea5c2-37cc-42d5-a1d7-fac1d7765d4e"),
            target.clone(),
        );
        let right = QualifiedConversationReference::new(
            instance("77a8586e-dd2d-4275-98cf-3602e858e12e"),
            target,
        );
        assert_ne!(left, right);
    }

    #[test]
    fn message_identity_requires_its_transcript() {
        let missing_transcript = json!({
            "instance_id": "a48ea5c2-37cc-42d5-a1d7-fac1d7765d4e",
            "target": {"kind": "message", "message_id": "message-only"}
        });
        assert!(
            serde_json::from_value::<QualifiedConversationReference>(missing_transcript).is_err()
        );
    }

    #[test]
    fn instance_is_required_and_unknown_fields_are_rejected() {
        let missing_instance = json!({
            "target": {"kind": "coordinator"}
        });
        assert!(
            serde_json::from_value::<QualifiedConversationReference>(missing_instance).is_err()
        );

        let unknown_field = json!({
            "instance_id": "a48ea5c2-37cc-42d5-a1d7-fac1d7765d4e",
            "target": {"kind": "coordinator"},
            "href": "https://example.invalid/global"
        });
        assert!(serde_json::from_value::<QualifiedConversationReference>(unknown_field).is_err());
    }

    #[test]
    fn empty_message_id_is_rejected() {
        assert!(MessageId::parse("  ").is_err());
        assert!(serde_json::from_value::<MessageId>(json!("\t")).is_err());
    }
}
