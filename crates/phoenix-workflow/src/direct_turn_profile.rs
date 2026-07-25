use crate::{
    AcceptanceProfile, CodecRef, DeliveryItem, ExternalAcceptanceDisabled, ProfileRef,
    RuntimeAcceptanceEnabled, SupportedCodecRegistry, WorkflowProfile,
};
use serde::{Deserialize, Serialize};

pub const PROFILE_ID: &str = "direct_turn";
pub const PROTOCOL_VERSION: u32 = 1;
pub const SNAPSHOT_CODEC_FAMILY: &str = "direct_turn.snapshot";
pub const EVENT_CODEC_FAMILY: &str = "direct_turn.event";
pub const INTENT_CODEC_FAMILY: &str = "direct_turn.intent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTurnProfile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnSnapshot {
    pub turn_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectTurnEvent {
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTurnIntent {
    pub turn_id: u64,
    pub conversation_id: String,
    pub client_turn_key: String,
    pub prepared_fingerprint: String,
}

impl WorkflowProfile for DirectTurnProfile {
    type Snapshot = DirectTurnSnapshot;
    type RuntimeAcceptance = RuntimeAcceptanceEnabled;
    type ExternalAcceptance = ExternalAcceptanceDisabled;
    type Event = DirectTurnEvent;
    type Intent = RuntimeTurnIntent;
    type Observation = ();
    type Receipt = ();
    type ReceiptReducerEvent = ();
    type BarrierEvent = ();
    type ManualPayload = ();

    fn runtime_start_allowed(_: &Self::Snapshot) -> bool {
        true
    }

    fn receipt_requires_runtime_acceptance((): &Self::ReceiptReducerEvent) -> bool {
        false
    }

    fn decision_handles_delivery(_: &DeliveryItem<Self>, _: &Self::Event) -> bool {
        false
    }

    fn decision_handles_runtime_acceptance(_: &DeliveryItem<Self>, _: &Self::Event) -> bool {
        false
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
    SupportedCodecRegistry::new([snapshot_codec(), event_codec(), intent_codec()])
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
