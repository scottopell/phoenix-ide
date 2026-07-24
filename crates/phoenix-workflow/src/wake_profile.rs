use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    AcceptanceProfile, BarrierId, CancellationReceiptDecl, CancellationRequest, CodecRef,
    DeliveryId, DeliveryItem, DeliveryPayload, EffectDecl, EffectId, EffectInvalidationDecl,
    EffectRole, ExecutionCapability, ExternalAcceptanceDisabled, Generation, ManualChoice,
    ManualChoiceKind, ProfileRef, RuntimeAcceptanceEnabled, SupportedCodecRegistry, Timestamp,
    TransitionPlan, WorkflowProfile,
};

pub const PROFILE_ID: &str = "wake";
pub const PROTOCOL_VERSION: u32 = 1;
pub const SNAPSHOT_CODEC_FAMILY: &str = "wake.snapshot";
pub const EVENT_CODEC_FAMILY: &str = "wake.event";
pub const INTENT_CODEC_FAMILY: &str = "wake.intent";
pub const MANUAL_CODEC_FAMILY: &str = "wake.manual";
pub const BARRIER_CODEC_FAMILY: &str = "wake.barrier";
pub const TERMINAL_CODEC_FAMILY: &str = "wake.terminal";
pub const REGISTRATION_BARRIER_ID: BarrierId = BarrierId(1);
pub const REGISTRATION_EFFECT_ID: EffectId = EffectId(1);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkScopeIdentity(pub String);

impl<'de> Deserialize<'de> for WorkScopeIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CompatibleIdentity {
            Opaque(String),
            Legacy { stable_key: String },
        }

        Ok(match CompatibleIdentity::deserialize(deserializer)? {
            CompatibleIdentity::Opaque(value) => Self(value),
            CompatibleIdentity::Legacy { stable_key } => Self(stable_key),
        })
    }
}

impl WorkScopeIdentity {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BashResourceIdentity {
    pub work_scope: WorkScopeIdentity,
    pub handle_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TmuxCompletionPolicy {
    KeepOpen,
    CloseAfterCompletion,
}

fn legacy_work_scope_identity() -> WorkScopeIdentity {
    WorkScopeIdentity("legacy-unscoped".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TmuxResourceIdentity {
    // owned: pre-WorkScope wake payloads omitted this field; a sentinel keeps
    // them decodable so persisted wake recovery can classify them safely.
    #[serde(default = "legacy_work_scope_identity")]
    pub work_scope: WorkScopeIdentity,
    pub server_token: String,
    pub window_id: String,
    pub completion_policy: TmuxCompletionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SubagentResourceIdentity {
    pub child_conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WakeResourceIdentity {
    Bash(BashResourceIdentity),
    TmuxWindow(TmuxResourceIdentity),
    Subagent(SubagentResourceIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeRegistrationIntent {
    pub contract_id: String,
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub registration_scope: WorkScopeIdentity,
    pub resource: WakeResourceIdentity,
    pub registering_tool_use_id: String,
    pub registered_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveHandleIntent {
    pub contract_id: String,
    pub resource: WakeResourceIdentity,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeRegistrationReceipt {
    pub contract_id: String,
    pub resource: WakeResourceIdentity,
    pub expires_at: Timestamp,
    pub registering_tool_use_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BashTerminalStatus {
    Exited,
    Killed,
    KillPendingKernel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TmuxTerminalStatus {
    ExitMarkerObserved,
    WindowKilled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashTerminalEvidence {
    pub identity: BashResourceIdentity,
    pub status: BashTerminalStatus,
    pub occurred_at: Timestamp,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub signal_number: Option<i32>,
    pub kill_signal_sent: Option<String>,
    pub final_tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxTerminalEvidence {
    pub identity: TmuxResourceIdentity,
    pub status: TmuxTerminalStatus,
    pub occurred_at: Timestamp,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub final_tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedChildTerminalRecord(String);

impl PersistedChildTerminalRecord {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentTerminalOutcome {
    SubmitResult { result: String },
    SubmitError { kind: String, error: String },
    ImplicitTextCompletion { result: String },
    RuntimeFailure { kind: String },
    ContextExhausted,
    WallClockTimeout,
    IndependentlyObservedCancellation,
    TurnLimitHardStop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentTerminalEvidence {
    pub identity: SubagentResourceIdentity,
    pub occurred_at: Timestamp,
    pub persisted_child_terminal_record: PersistedChildTerminalRecord,
    pub outcome: SubagentTerminalOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeTerminalEvidence {
    Bash(BashTerminalEvidence),
    TmuxWindow(TmuxTerminalEvidence),
    Subagent(SubagentTerminalEvidence),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeCancellationReason {
    ExplicitCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeForgottenReason {
    PhoenixRestart,
    CascadeDestroyedHandle,
    SubagentHandleMissing,
    TmuxHandleMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeTerminalPayload {
    Fired {
        contract_id: String,
        resource: WakeResourceIdentity,
        evidence: WakeTerminalEvidence,
        resolved_at: Timestamp,
    },
    Cancelled {
        contract_id: String,
        resource: WakeResourceIdentity,
        reason: WakeCancellationReason,
        resolved_at: Timestamp,
    },
    Expired {
        contract_id: String,
        resource: WakeResourceIdentity,
        resolved_at: Timestamp,
    },
    Forgotten {
        contract_id: String,
        resource: WakeResourceIdentity,
        reason: WakeForgottenReason,
        resolved_at: Timestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeBarrierEvent {
    RegistrationObserved { receipt: WakeRegistrationReceipt },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeAvailability {
    Idle,
    Pending,
    Accepted,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeRegistrationSnapshot {
    pub contract_id: String,
    pub resource: WakeResourceIdentity,
    pub registered: bool,
    pub terminal: Option<WakeTerminalPayload>,
    pub runtime_availability: RuntimeAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeRegistrationEvent {
    Registered,
    TerminalProjected {
        terminal: Box<WakeTerminalPayload>,
    },
    RuntimeAccepted {
        terminal: Box<WakeTerminalPayload>,
    },
    RuntimeSuppressed {
        terminal: Box<WakeTerminalPayload>,
    },
    OwnershipTransferred {
        from_conversation_id: String,
        to_conversation_id: String,
        pending_delivery_ids: Vec<DeliveryId>,
    },
    CancelRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeManualPayload {
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeProfile;

impl WorkflowProfile for WakeProfile {
    type RuntimeAcceptance = RuntimeAcceptanceEnabled;
    type ExternalAcceptance = ExternalAcceptanceDisabled;
    type Snapshot = WakeRegistrationSnapshot;
    type Event = WakeRegistrationEvent;
    type Intent = ObserveHandleIntent;
    type Observation = WakeTerminalEvidence;
    type Receipt = WakeTerminalPayload;
    type ReceiptReducerEvent = WakeTerminalPayload;
    type BarrierEvent = WakeBarrierEvent;
    type ManualPayload = WakeManualPayload;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool {
        matches!(
            (&snapshot.runtime_availability, &snapshot.terminal),
            (
                RuntimeAvailability::Accepted,
                Some(
                    WakeTerminalPayload::Fired { .. }
                        | WakeTerminalPayload::Expired { .. }
                        | WakeTerminalPayload::Forgotten { .. }
                )
            )
        )
    }

    fn receipt_requires_runtime_acceptance(_event: &Self::ReceiptReducerEvent) -> bool {
        false
    }

    fn decision_handles_delivery(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool {
        match (&item.payload, decision_event) {
            (
                DeliveryPayload::Receipt(WakeTerminalPayload::Cancelled { .. }),
                WakeRegistrationEvent::CancelRequested,
            )
            | (DeliveryPayload::Barrier(_), WakeRegistrationEvent::Registered) => true,
            (
                DeliveryPayload::Receipt(terminal),
                WakeRegistrationEvent::TerminalProjected {
                    terminal: projected,
                },
            ) => projected.as_ref() == terminal,
            _ => false,
        }
    }

    fn decision_handles_runtime_acceptance(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event),
            (DeliveryPayload::Receipt(event), WakeRegistrationEvent::RuntimeAccepted { terminal })
                if !matches!(event, WakeTerminalPayload::Cancelled { .. }) && terminal.as_ref() == event)
    }

    fn decision_handles_runtime_suppression(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool {
        matches!((&item.payload, decision_event),
            (DeliveryPayload::Receipt(event), WakeRegistrationEvent::RuntimeSuppressed { terminal })
                if !matches!(event, WakeTerminalPayload::Cancelled { .. }) && terminal.as_ref() == event)
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
        manual_codec(),
        barrier_codec(),
        terminal_codec(),
    ])
    .unwrap_or_else(|| unreachable!("static wake codec registry is non-empty and valid"))
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
pub fn manual_codec() -> CodecRef {
    CodecRef {
        family: MANUAL_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}
#[must_use]
pub fn barrier_codec() -> CodecRef {
    CodecRef {
        family: BARRIER_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}
#[must_use]
pub fn terminal_codec() -> CodecRef {
    CodecRef {
        family: TERMINAL_CODEC_FAMILY,
        version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn barrier_events(receipt: WakeRegistrationReceipt) -> BTreeMap<BarrierId, WakeBarrierEvent> {
    BTreeMap::from([(
        REGISTRATION_BARRIER_ID,
        WakeBarrierEvent::RegistrationObserved { receipt },
    )])
}

#[must_use]
pub fn registration_barrier_event(receipt: WakeRegistrationReceipt) -> WakeBarrierEvent {
    WakeBarrierEvent::RegistrationObserved { receipt }
}

#[must_use]
pub fn observe_effect(
    intent: ObserveHandleIntent,
    generation: Generation,
) -> EffectDecl<ObserveHandleIntent> {
    EffectDecl {
        effect_id: REGISTRATION_EFFECT_ID,
        family: "wake.observe",
        kind: "observe_handle",
        codec: intent_codec(),
        generation,
        role: EffectRole::Required,
        capability: ExecutionCapability::ReclaimableObservation,
        intent,
        next_eligible_at: None,
        destructive_resource: None,
    }
}

#[must_use]
pub fn registration_plan(
    snapshot: WakeRegistrationSnapshot,
    event: WakeRegistrationEvent,
    intent: ObserveHandleIntent,
) -> TransitionPlan<WakeProfile> {
    TransitionPlan {
        next_status: crate::WorkflowStatus::Active,
        snapshot,
        snapshot_codec: snapshot_codec(),
        event,
        event_codec: event_codec(),
        effects: vec![observe_effect(intent, Generation(0))],
        dependencies: vec![],
        barriers: vec![],
        barrier_members: vec![],
        invalidations: vec![],
        deliveries: vec![],
        schedules: vec![],
    }
}

#[must_use]
pub fn cancellation_request(
    expected_workflow_version: crate::Version,
    next_snapshot: WakeRegistrationSnapshot,
    compensation_plan: TransitionPlan<WakeProfile>,
) -> CancellationRequest<WakeProfile> {
    CancellationRequest {
        expected_workflow_version,
        next_snapshot,
        next_snapshot_codec: snapshot_codec(),
        event: WakeRegistrationEvent::CancelRequested,
        event_codec: event_codec(),
        invalidations: vec![EffectInvalidationDecl {
            effect_id: REGISTRATION_EFFECT_ID,
        }],
        terminal_receipt: None::<CancellationReceiptDecl<WakeProfile>>,
        compensation_plan,
    }
}

#[must_use]
pub fn manual_choices(terminal: WakeTerminalPayload) -> Vec<ManualChoice<WakeProfile>> {
    vec![ManualChoice {
        kind: ManualChoiceKind::AcceptAsTerminal,
        codec: manual_codec(),
        payload: WakeManualPayload {
            note: "accept".into(),
        },
        receipt_codec: terminal_codec(),
        receipt: terminal.clone(),
        receipt_event_codec: terminal_codec(),
        receipt_event: terminal,
    }]
}

#[cfg(test)]
mod tests;
