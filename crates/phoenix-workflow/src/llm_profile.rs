use serde::{Deserialize, Serialize};

use crate::{
    AcceptanceProfile, CodecRef, DeliveryItem, DeliveryPayload, EffectDecl, EffectId, EffectRole,
    ExecutionCapability, ExternalAcceptanceDisabled, Generation, ProfileRef,
    RuntimeAcceptanceEnabled, SupportedCodecRegistry, TransitionPlan, WorkflowProfile,
    WorkflowStatus,
};

pub const PROFILE_ID: &str = "top_level_llm";
pub const PROTOCOL_VERSION: u32 = 1;
pub const SNAPSHOT_CODEC_FAMILY: &str = "llm.snapshot";
pub const EVENT_CODEC_FAMILY: &str = "llm.event";
pub const INTENT_CODEC_FAMILY: &str = "llm.intent";
pub const RECEIPT_CODEC_FAMILY: &str = "llm.receipt";
pub const EFFECT_ID: EffectId = EffectId(1);
pub const CONSUMER_KIND_TOP_LEVEL_CONVERSATION: &str = "top_level_conversation";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopLevelTurnRef {
    pub conversation_id: String,
    pub accepted_turn_id: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmEffectKey {
    pub accepted_turn_id: String,
    pub generation: u64,
    pub call_ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedLlmRequest {
    pub codec_version: u32,
    pub request_fingerprint: String,
    pub provider: String,
    pub model: String,
    pub backend: String,
    pub request_aggregate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteLlmResponse {
    pub codec_version: u32,
    pub response_fingerprint: String,
    pub response_aggregate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopLevelLlmSnapshot {
    pub turn_ref: TopLevelTurnRef,
    pub accepted_assistant_message_id: Option<String>,
    pub stopped_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopLevelLlmEvent {
    Prepared {
        key: LlmEffectKey,
    },
    ResponseAccepted {
        key: LlmEffectKey,
        assistant_message_id: String,
    },
    ResponseCancelled {
        key: LlmEffectKey,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmIntent {
    pub key: LlmEffectKey,
    pub prepared_request: PreparedLlmRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmObservation {
    pub key: LlmEffectKey,
    pub attempt_ordinal: u64,
    pub process_incarnation: String,
    pub provider_request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponseReceipt {
    pub key: LlmEffectKey,
    pub response: CompleteLlmResponse,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmBarrierEvent {
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmManualPayload {
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmProfile;

impl WorkflowProfile for LlmProfile {
    type RuntimeAcceptance = RuntimeAcceptanceEnabled;
    type ExternalAcceptance = ExternalAcceptanceDisabled;
    type Snapshot = TopLevelLlmSnapshot;
    type Event = TopLevelLlmEvent;
    type Intent = LlmIntent;
    type Observation = LlmObservation;
    type Receipt = LlmResponseReceipt;
    type ReceiptReducerEvent = LlmResponseReceipt;
    type BarrierEvent = LlmBarrierEvent;
    type ManualPayload = LlmManualPayload;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        snapshot.stopped_at.is_none()
    }

    fn receipt_requires_runtime_acceptance(_event: &Self::ReceiptReducerEvent) -> bool {
        true
    }

    fn decision_handles_delivery(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        matches!(
            (&item.payload, decision_event),
            (
                DeliveryPayload::Receipt(receipt),
                TopLevelLlmEvent::ResponseAccepted { key, .. }
                    | TopLevelLlmEvent::ResponseCancelled { key, .. }
            ) if receipt.key == *key
        )
    }

    fn decision_handles_runtime_acceptance(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!(
            (&item.payload, decision_event),
            (
                DeliveryPayload::Receipt(receipt),
                TopLevelLlmEvent::ResponseAccepted { key, .. }
            ) if receipt.key == *key
        )
    }

    fn decision_handles_runtime_suppression(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!(
            (&item.payload, decision_event),
            (
                DeliveryPayload::Receipt(receipt),
                TopLevelLlmEvent::ResponseCancelled { key, .. }
            ) if receipt.key == *key
        )
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
        receipt_codec(),
    ])
    .unwrap_or_else(|| unreachable!("static llm codec registry is non-empty and valid"))
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
pub fn receipt_codec() -> CodecRef {
    CodecRef {
        family: RECEIPT_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn llm_effect(intent: LlmIntent, generation: Generation) -> EffectDecl<LlmIntent> {
    EffectDecl {
        effect_id: EFFECT_ID,
        family: "llm.call",
        kind: "top_level_call",
        codec: intent_codec(),
        generation,
        role: EffectRole::Required,
        capability: ExecutionCapability::SafelyRepeatable,
        intent,
        next_eligible_at: None,
        destructive_resource: None,
    }
}

#[must_use]
pub fn prepared_plan(
    snapshot: TopLevelLlmSnapshot,
    event: TopLevelLlmEvent,
    intent: LlmIntent,
) -> TransitionPlan<LlmProfile> {
    TransitionPlan {
        next_status: WorkflowStatus::Active,
        snapshot,
        snapshot_codec: snapshot_codec(),
        event,
        event_codec: event_codec(),
        effects: vec![llm_effect(intent, Generation(0))],
        dependencies: vec![],
        barriers: vec![],
        barrier_members: vec![],
        invalidations: vec![],
        deliveries: vec![],
        schedules: vec![],
    }
}

#[cfg(test)]
mod tests;
