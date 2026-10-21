use crate::{
    AcceptanceProfile, CodecRef, DeliveryItem, DeliveryPayload, ExternalAcceptanceDisabled,
    ProfileRef, RuntimeAcceptanceEnabled, SupportedCodecRegistry, WorkflowProfile,
};
use serde::{Deserialize, Serialize};

pub const PROFILE_ID: &str = "direct_turn";
pub const PROTOCOL_VERSION: u32 = 1;
pub const SNAPSHOT_CODEC_FAMILY: &str = "direct_turn.snapshot";
pub const EVENT_CODEC_FAMILY: &str = "direct_turn.event";
pub const INTENT_CODEC_FAMILY: &str = "direct_turn.intent";
pub const PREPARED_PAYLOAD_CODEC_FAMILY: &str = "direct_turn.prepared_payload";
pub const RECEIPT_CODEC_FAMILY: &str = "direct_turn.receipt";
pub const RECEIPT_EVENT_CODEC_FAMILY: &str = "direct_turn.receipt_event";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTurnProfile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnSnapshot {
    pub turn_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectTurnEvent {
    Accepted,
    Delivered(DirectTurnReceiptEvent),
    Terminal(DirectTurnTerminalEvent),
    Rearmed {
        turn_id: u64,
        generation: u64,
    },
    RuntimeReconstructed {
        turn_id: u64,
        previous_generation: u64,
        generation: u64,
        reconstruction_id: String,
        request_id: String,
        state_updated_at: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnTerminalEvent {
    pub terminal: DirectTurnTerminalKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectTurnTerminalKind {
    Completed,
    Cancelled,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTurnIntent {
    pub turn_id: u64,
    pub conversation_id: String,
    pub client_turn_key: String,
    pub prepared_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CanonicalTurnReceipt {
    Message(DirectTurnReceipt),
    GoalStart(GoalStartTurnReceipt),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalStartTurnReceipt {
    pub turn_id: u64,
    pub goal_start_receipt_id: crate::GoalStartReceiptId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectTurnReceipt {
    pub turn_id: u64,
    pub canonical_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectTurnReceiptEvent {
    Materialized { canonical_message_id: String },
    GoalStartMaterialized { receipt: crate::GoalStartReceiptId },
}

impl WorkflowProfile for DirectTurnProfile {
    type Snapshot = DirectTurnSnapshot;
    type RuntimeAcceptance = RuntimeAcceptanceEnabled;
    type ExternalAcceptance = ExternalAcceptanceDisabled;
    type Event = DirectTurnEvent;
    type Intent = RuntimeTurnIntent;
    type Observation = ();
    type Receipt = CanonicalTurnReceipt;
    type ReceiptReducerEvent = DirectTurnReceiptEvent;
    type BarrierEvent = ();
    type ManualPayload = ();

    fn runtime_start_allowed(_: &Self::Snapshot) -> bool {
        true
    }

    fn receipt_requires_runtime_acceptance(_: &Self::ReceiptReducerEvent) -> bool {
        true
    }

    fn decision_handles_delivery(item: &DeliveryItem<Self>, event: &Self::Event) -> bool {
        Self::decision_handles_runtime_acceptance(item, event)
    }

    fn decision_handles_runtime_acceptance(item: &DeliveryItem<Self>, event: &Self::Event) -> bool {
        matches!(
            (&item.payload, event),
            (DeliveryPayload::Receipt(delivery), DirectTurnEvent::Delivered(accepted))
                if delivery == accepted
        )
    }

    fn decision_handles_runtime_suppression(_: &DeliveryItem<Self>, _: &Self::Event) -> bool {
        false
    }
}

#[must_use]
pub fn profile() -> ProfileRef {
    ProfileRef {
        profile_kind: PROFILE_ID.to_string(),
        profile_version: PROTOCOL_VERSION,
    }
}

fn supported_codecs() -> SupportedCodecRegistry {
    SupportedCodecRegistry::new([
        snapshot_codec(),
        event_codec(),
        intent_codec(),
        prepared_payload_codec(),
        receipt_codec(),
        receipt_event_codec(),
    ])
    .unwrap_or_else(|| unreachable!("static direct-turn codec registry is non-empty and valid"))
}

#[must_use]
pub fn acceptance_profile(
) -> AcceptanceProfile<RuntimeAcceptanceEnabled, ExternalAcceptanceDisabled> {
    AcceptanceProfile::new(profile(), supported_codecs())
}

#[must_use]
pub fn snapshot_codec() -> CodecRef {
    CodecRef {
        family: SNAPSHOT_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn event_codec() -> CodecRef {
    CodecRef {
        family: EVENT_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn intent_codec() -> CodecRef {
    CodecRef {
        family: INTENT_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn prepared_payload_codec() -> CodecRef {
    CodecRef {
        family: PREPARED_PAYLOAD_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn receipt_codec() -> CodecRef {
    CodecRef {
        family: RECEIPT_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn receipt_event_codec() -> CodecRef {
    CodecRef {
        family: RECEIPT_EVENT_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_goal_start_workflow_receipt_is_typed_and_has_no_message_identity() {
        let receipt = CanonicalTurnReceipt::GoalStart(GoalStartTurnReceipt {
            turn_id: 9,
            goal_start_receipt_id: crate::GoalStartReceiptId::new(4).unwrap(),
        });
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"turn_id":9,"goal_start_receipt_id":4})
        );
        assert_eq!(
            serde_json::from_value::<CanonicalTurnReceipt>(value).unwrap(),
            receipt
        );
        let event = DirectTurnReceiptEvent::GoalStartMaterialized {
            receipt: crate::GoalStartReceiptId::new(4).unwrap(),
        };
        assert_eq!(serde_json::from_value::<DirectTurnReceiptEvent>(
            serde_json::to_value(&event).unwrap(),
        ).unwrap(), event);
    }

    #[test]
    fn conversation_goal_start_workflow_receipt_rejects_ambiguous_or_invalid_input() {
        for value in [
            serde_json::json!({"turn_id":9,"goal_start_receipt_id":4,"canonical_message_id":"fake"}),
            serde_json::json!({"turn_id":9,"goal_start_receipt_id":0}),
            serde_json::json!({"turn_id":9,"goal_start_receipt_id":u64::MAX}),
            serde_json::json!({"turn_id":9}),
        ] {
            assert!(serde_json::from_value::<CanonicalTurnReceipt>(value).is_err());
        }
    }

    #[test]
    fn conversation_goal_start_message_receipt_encoding_remains_message_only() {
        let message = DirectTurnReceipt {
            turn_id: 9,
            canonical_message_id: "message".into(),
        };
        let value = serde_json::json!({"turn_id":9,"canonical_message_id":"message"});
        assert_eq!(serde_json::to_value(&message).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<CanonicalTurnReceipt>(value).unwrap(),
            CanonicalTurnReceipt::Message(message)
        );
    }
}
