use std::collections::BTreeMap;

use crate::{
    BarrierDecl, BarrierId, BarrierMemberDecl, CancellationRequest, ClaimAuthority, CodecRef,
    EffectAmbiguity, EffectDecl, EffectId, EffectInvalidationDecl, EffectRole, EffectState,
    EffectStatus, Generation, ManualChoice, ManualChoiceKind, ObservationRecord,
    OwedAcceptanceDecl, ProfileRef, ProtocolSelection, ReceiptFamily, ReducerDecision,
    ReducerInboxEvent, ReducerInboxId, ReducerInboxKind, ReducerInboxPayload, SemanticAuthority,
    ShadowDivergenceKind, Timestamp, TransitionPlan, Version, WorkflowProfile, WorkflowState,
    WorkflowStatus,
};

pub const PROFILE_ID: &str = "wake";
pub const PROTOCOL_VERSION: u32 = 1;
pub const SNAPSHOT_CODEC_FAMILY: &str = "wake.snapshot";
pub const EVENT_CODEC_FAMILY: &str = "wake.event";
pub const INTENT_CODEC_FAMILY: &str = "wake.intent";
pub const MANUAL_CODEC_FAMILY: &str = "wake.manual";
pub const REGISTRATION_BARRIER_ID: BarrierId = BarrierId(1);
pub const REGISTRATION_EFFECT_ID: EffectId = EffectId(1);
pub const REGISTRATION_BARRIER_KIND: &str = "registration_observed";
pub const OBSERVE_HANDLE_KIND: &str = "observe_handle";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkScopeKind {
    Conversation,
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkScopeIdentity {
    pub kind: WorkScopeKind,
    pub stable_key: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BashResourceIdentity {
    pub work_scope: WorkScopeIdentity,
    pub handle_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TmuxResourceIdentity {
    pub work_scope: WorkScopeIdentity,
    pub server_generation: &'static str,
    pub window_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WakeResourceIdentity {
    Bash(BashResourceIdentity),
    TmuxWindow(TmuxResourceIdentity),
}

impl WakeResourceIdentity {
    #[must_use]
    pub const fn work_scope(self) -> WorkScopeIdentity {
        match self {
            Self::Bash(identity) => identity.work_scope,
            Self::TmuxWindow(identity) => identity.work_scope,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRegistrationIntent {
    pub contract_id: &'static str,
    pub conversation_id: &'static str,
    pub registration_scope: WorkScopeIdentity,
    pub resource: WakeResourceIdentity,
    pub registering_tool_use_id: &'static str,
    pub registered_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserveHandleIntent {
    pub contract_id: &'static str,
    pub resource: WakeResourceIdentity,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRegistrationReceipt {
    pub contract_id: &'static str,
    pub resource: WakeResourceIdentity,
    pub expires_at: Timestamp,
    pub registering_tool_use_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashTerminalStatus {
    Exited,
    Killed,
    KillPendingKernel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxTerminalStatus {
    ExitMarkerObserved,
    WindowKilled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashTerminalEvidence {
    pub identity: BashResourceIdentity,
    pub status: BashTerminalStatus,
    pub occurred_at: Timestamp,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub signal_number: Option<i32>,
    pub kill_signal_sent: Option<&'static str>,
    pub final_tail: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxTerminalEvidence {
    pub identity: TmuxResourceIdentity,
    pub status: TmuxTerminalStatus,
    pub occurred_at: Timestamp,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub final_tail: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeTerminalEvidence {
    Bash(BashTerminalEvidence),
    TmuxWindow(TmuxTerminalEvidence),
}

impl WakeTerminalEvidence {
    #[must_use]
    pub const fn occurred_at(&self) -> Timestamp {
        match self {
            Self::Bash(evidence) => evidence.occurred_at,
            Self::TmuxWindow(evidence) => evidence.occurred_at,
        }
    }

    #[must_use]
    pub const fn identity(&self) -> WakeResourceIdentity {
        match self {
            Self::Bash(evidence) => WakeResourceIdentity::Bash(evidence.identity),
            Self::TmuxWindow(evidence) => WakeResourceIdentity::TmuxWindow(evidence.identity),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeCancellationReason {
    ExplicitCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeForgottenReason {
    HandleMissing,
    RuntimeUnrecoverableAfterRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeTerminalPayload {
    Fired {
        contract_id: &'static str,
        resource: WakeResourceIdentity,
        evidence: WakeTerminalEvidence,
        resolved_at: Timestamp,
    },
    Expired {
        contract_id: &'static str,
        resource: WakeResourceIdentity,
        resolved_at: Timestamp,
    },
    Cancelled {
        contract_id: &'static str,
        resource: WakeResourceIdentity,
        reason: WakeCancellationReason,
        resolved_at: Timestamp,
    },
    Forgotten {
        contract_id: &'static str,
        resource: WakeResourceIdentity,
        reason: WakeForgottenReason,
        resolved_at: Timestamp,
    },
}

impl WakeTerminalPayload {
    #[must_use]
    pub const fn contract_id(&self) -> &'static str {
        match self {
            Self::Fired { contract_id, .. }
            | Self::Expired { contract_id, .. }
            | Self::Cancelled { contract_id, .. }
            | Self::Forgotten { contract_id, .. } => contract_id,
        }
    }

    #[must_use]
    pub const fn resource(&self) -> WakeResourceIdentity {
        match self {
            Self::Fired { resource, .. }
            | Self::Expired { resource, .. }
            | Self::Cancelled { resource, .. }
            | Self::Forgotten { resource, .. } => *resource,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRegistrationSnapshot {
    pub contract_id: &'static str,
    pub conversation_id: &'static str,
    pub registration_scope: WorkScopeIdentity,
    pub resource: WakeResourceIdentity,
    pub registering_tool_use_id: &'static str,
    pub registered_at: Timestamp,
    pub expires_at: Timestamp,
    pub registration_fence_version: Version,
    pub runtime_availability: RuntimeAvailability,
    pub continuation: Option<WakeContinuationTransfer>,
    pub terminal: Option<WakeTerminalPayload>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationEvent {
    Registered,
    CancelRequested,
    Continued,
    RuntimeAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeBarrierEvent {
    RegistrationObserved { receipt: WakeRegistrationReceipt },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAvailability {
    Idle,
    Busy,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAvailabilityProjection {
    Accept,
    Defer,
    Suppress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeContinuationTransfer {
    pub pending_contract: &'static str,
    pub resource: WakeResourceIdentity,
    pub expires_at: Timestamp,
    pub inbox_ids: Vec<ReducerInboxId>,
    pub owed_ids: Vec<u64>,
    pub successor_workflow_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRegistrationFence {
    pub status: FenceStatus,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeLifecycleFence {
    pub status: FenceStatus,
    pub version: Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeManualPayload {
    Accept,
    Defer,
    Suppress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeShadowComparisonKind {
    Registration,
    Observation,
    TerminalReceipt,
    Inbox,
    Acceptance,
    Lifecycle,
    Capability,
    UserProjection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeShadowComparison {
    pub kind: WakeShadowComparisonKind,
    pub generic_kind: ShadowDivergenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeReceiptComparison {
    pub equal: bool,
    pub exact_identity_match: bool,
    pub exact_deadline_match: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeProfile;

impl WorkflowProfile for WakeProfile {
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
                RuntimeAvailability::Idle,
                Some(
                    WakeTerminalPayload::Fired { .. }
                        | WakeTerminalPayload::Expired { .. }
                        | WakeTerminalPayload::Forgotten { .. }
                )
            )
        )
    }
}

#[must_use]
pub fn profile() -> ProfileRef {
    ProfileRef {
        profile_id: PROFILE_ID,
        protocol_version: PROTOCOL_VERSION,
    }
}

#[must_use]
pub fn protocol(selector: &'static str, accepting: bool) -> ProtocolSelection {
    ProtocolSelection {
        profile: profile(),
        authority: SemanticAuthority::EngineProtocol,
        accepting,
        runtime_acceptance_enabled: true,
        selector,
    }
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
pub fn registration_snapshot(
    intent: &WakeRegistrationIntent,
    fence_version: Version,
) -> WakeRegistrationSnapshot {
    WakeRegistrationSnapshot {
        contract_id: intent.contract_id,
        conversation_id: intent.conversation_id,
        registration_scope: intent.registration_scope,
        resource: intent.resource,
        registering_tool_use_id: intent.registering_tool_use_id,
        registered_at: intent.registered_at,
        expires_at: intent.expires_at,
        registration_fence_version: fence_version,
        runtime_availability: RuntimeAvailability::Busy,
        continuation: None,
        terminal: None,
        cancelled: false,
    }
}

#[must_use]
pub fn registration_receipt(intent: &WakeRegistrationIntent) -> WakeRegistrationReceipt {
    WakeRegistrationReceipt {
        contract_id: intent.contract_id,
        resource: intent.resource,
        expires_at: intent.expires_at,
        registering_tool_use_id: intent.registering_tool_use_id,
    }
}

#[must_use]
pub fn registration_decision(
    expected_workflow_version: Version,
    intent: &WakeRegistrationIntent,
    fence_version: Version,
) -> (
    ReducerDecision<WakeProfile>,
    BTreeMap<BarrierId, WakeBarrierEvent>,
) {
    let receipt = registration_receipt(intent);
    let observe_intent = ObserveHandleIntent {
        contract_id: intent.contract_id,
        resource: intent.resource,
        expires_at: intent.expires_at,
    };
    (
        ReducerDecision {
            expected_workflow_version,
            plan: TransitionPlan {
                snapshot: registration_snapshot(intent, fence_version),
                snapshot_codec: snapshot_codec(),
                event: WakeRegistrationEvent::Registered,
                event_codec: event_codec(),
                effects: vec![EffectDecl {
                    effect_id: REGISTRATION_EFFECT_ID,
                    family: PROFILE_ID,
                    kind: OBSERVE_HANDLE_KIND,
                    codec: intent_codec(),
                    generation: Generation(0),
                    role: EffectRole::Required,
                    ambiguity: EffectAmbiguity::ObservableReconciliation,
                    intent: observe_intent,
                    next_eligible_at: None,
                    destructive_resource: None,
                }],
                dependencies: vec![],
                barriers: vec![BarrierDecl {
                    barrier_id: REGISTRATION_BARRIER_ID,
                }],
                barrier_members: vec![BarrierMemberDecl {
                    barrier_id: REGISTRATION_BARRIER_ID,
                    effect_id: REGISTRATION_EFFECT_ID,
                    receipt_family: ReceiptFamily::CurrentGenerationEffect,
                }],
                invalidations: vec![],
                owed_acceptances: None,
            },
        },
        barrier_events(receipt),
    )
}

#[must_use]
pub fn cancellation_request(
    workflow: &WorkflowState<WakeProfile>,
    resolved_at: Timestamp,
) -> CancellationRequest<WakeProfile> {
    let invalidations = workflow
        .effects
        .iter()
        .filter_map(|(effect_id, effect)| {
            (effect.declaration.kind == OBSERVE_HANDLE_KIND
                && effect.declaration.generation == workflow.generation
                && effect.status != EffectStatus::Receipted
                && effect.status != EffectStatus::Invalidated)
                .then_some(EffectInvalidationDecl {
                    effect_id: *effect_id,
                })
        })
        .collect::<Vec<_>>();
    let mut next_snapshot = workflow.snapshot.clone();
    next_snapshot.cancelled = true;
    next_snapshot.terminal = Some(cancelled_terminal_payload(
        next_snapshot.contract_id,
        next_snapshot.resource,
        WakeCancellationReason::ExplicitCancel,
        resolved_at,
    ));
    CancellationRequest {
        expected_workflow_version: workflow.version,
        next_snapshot,
        next_snapshot_codec: snapshot_codec(),
        event: WakeRegistrationEvent::CancelRequested,
        event_codec: event_codec(),
        invalidations,
        compensation_plan: TransitionPlan {
            snapshot: workflow.snapshot.clone(),
            snapshot_codec: snapshot_codec(),
            event: WakeRegistrationEvent::CancelRequested,
            event_codec: event_codec(),
            effects: vec![],
            dependencies: vec![],
            barriers: vec![],
            barrier_members: vec![],
            invalidations: vec![],
            owed_acceptances: None,
        },
    }
}

#[must_use]
pub const fn cancelled_terminal_payload(
    contract_id: &'static str,
    resource: WakeResourceIdentity,
    reason: WakeCancellationReason,
    resolved_at: Timestamp,
) -> WakeTerminalPayload {
    WakeTerminalPayload::Cancelled {
        contract_id,
        resource,
        reason,
        resolved_at,
    }
}

#[must_use]
pub const fn project_runtime_availability(
    runtime: RuntimeAvailability,
) -> RuntimeAvailabilityProjection {
    match runtime {
        RuntimeAvailability::Busy => RuntimeAvailabilityProjection::Defer,
        RuntimeAvailability::Idle => RuntimeAvailabilityProjection::Accept,
        RuntimeAvailability::Terminal => RuntimeAvailabilityProjection::Suppress,
    }
}

#[must_use]
pub fn terminal_payload_from_evidence(
    contract_id: &'static str,
    resource: WakeResourceIdentity,
    evidence: WakeTerminalEvidence,
    expires_at: Timestamp,
) -> Option<WakeTerminalPayload> {
    if resource != evidence.identity() {
        return None;
    }
    let occurred_at = evidence.occurred_at();
    if occurred_at <= expires_at {
        Some(WakeTerminalPayload::Fired {
            contract_id,
            resource,
            evidence,
            resolved_at: occurred_at,
        })
    } else {
        Some(WakeTerminalPayload::Expired {
            contract_id,
            resource,
            resolved_at: expires_at,
        })
    }
}

#[must_use]
pub const fn forgotten_terminal_payload(
    contract_id: &'static str,
    resource: WakeResourceIdentity,
    reason: WakeForgottenReason,
    resolved_at: Timestamp,
) -> WakeTerminalPayload {
    WakeTerminalPayload::Forgotten {
        contract_id,
        resource,
        reason,
        resolved_at,
    }
}

#[must_use]
pub fn evidence_matches_resource(
    evidence: &WakeTerminalEvidence,
    resource: WakeResourceIdentity,
) -> bool {
    evidence.identity() == resource
}

#[must_use]
pub fn deadline_matches_exactly(lhs: Timestamp, rhs: Timestamp) -> bool {
    lhs == rhs
}

#[must_use]
pub fn compare_receipts(
    expected: &WakeRegistrationReceipt,
    actual: &WakeRegistrationReceipt,
) -> WakeReceiptComparison {
    WakeReceiptComparison {
        equal: expected == actual,
        exact_identity_match: expected.resource == actual.resource,
        exact_deadline_match: deadline_matches_exactly(expected.expires_at, actual.expires_at),
    }
}

#[must_use]
pub fn continuation_from_snapshot(
    snapshot: &WakeRegistrationSnapshot,
    inbox_ids: Vec<ReducerInboxId>,
    owed_ids: Vec<u64>,
    successor_workflow_id: u64,
) -> WakeContinuationTransfer {
    WakeContinuationTransfer {
        pending_contract: snapshot.contract_id,
        resource: snapshot.resource,
        expires_at: snapshot.expires_at,
        inbox_ids,
        owed_ids,
        successor_workflow_id,
    }
}

#[must_use]
pub fn transfer_continuation(
    snapshot: &WakeRegistrationSnapshot,
    inbox_ids: Vec<ReducerInboxId>,
    owed_ids: Vec<u64>,
    successor_workflow_id: u64,
) -> WakeRegistrationSnapshot {
    let mut next = snapshot.clone();
    next.continuation = Some(continuation_from_snapshot(
        snapshot,
        inbox_ids,
        owed_ids,
        successor_workflow_id,
    ));
    next
}

#[must_use]
pub const fn registration_fence(version: Version, status: FenceStatus) -> WakeRegistrationFence {
    WakeRegistrationFence { status, version }
}

#[must_use]
pub const fn lifecycle_fence(version: Version, status: FenceStatus) -> WakeLifecycleFence {
    WakeLifecycleFence { status, version }
}

#[must_use]
pub fn fence_accepts(
    workflow: &WorkflowState<WakeProfile>,
    registration: &WakeRegistrationFence,
    lifecycle: &WakeLifecycleFence,
) -> bool {
    workflow.status == WorkflowStatus::Active
        && registration.status == FenceStatus::Open
        && lifecycle.status == FenceStatus::Open
        && registration.version == lifecycle.version
        && workflow.snapshot.registration_fence_version == registration.version
}

#[must_use]
pub fn shadow_comparison(kind: WakeShadowComparisonKind) -> WakeShadowComparison {
    let generic_kind = match kind {
        WakeShadowComparisonKind::Registration => ShadowDivergenceKind::Snapshot,
        WakeShadowComparisonKind::Observation => ShadowDivergenceKind::Observation,
        WakeShadowComparisonKind::TerminalReceipt => ShadowDivergenceKind::Receipt,
        WakeShadowComparisonKind::Inbox => ShadowDivergenceKind::ReducerEvent,
        WakeShadowComparisonKind::Acceptance => ShadowDivergenceKind::Transition,
        WakeShadowComparisonKind::Lifecycle => ShadowDivergenceKind::EffectPlan,
        WakeShadowComparisonKind::Capability => ShadowDivergenceKind::Capability,
        WakeShadowComparisonKind::UserProjection => ShadowDivergenceKind::UserProjection,
    };
    WakeShadowComparison { kind, generic_kind }
}

#[must_use]
pub fn acceptance_owed_decl(
    inbox_id: ReducerInboxId,
    terminal: &WakeTerminalPayload,
) -> Option<OwedAcceptanceDecl<WakeTerminalPayload>> {
    if matches!(terminal, WakeTerminalPayload::Cancelled { .. }) {
        return None;
    }
    Some(OwedAcceptanceDecl {
        reducer_inbox_id: inbox_id,
        source_kind: "wake_terminal_receipt",
        event: terminal.clone(),
    })
}

#[must_use]
pub fn manual_choices() -> Vec<ManualChoice<WakeProfile>> {
    vec![
        ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: manual_codec(),
            payload: WakeManualPayload::Accept,
        },
        ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: manual_codec(),
            payload: WakeManualPayload::Defer,
        },
        ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: manual_codec(),
            payload: WakeManualPayload::Suppress,
        },
    ]
}

#[must_use]
pub fn authoritative_observation<'a>(
    authority: &ClaimAuthority,
    effect: &'a EffectState<WakeProfile>,
) -> Option<&'a ObservationRecord<WakeTerminalEvidence>> {
    effect
        .observations
        .iter()
        .find(|observation| observation.authority == *authority && observation.authoritative)
}

#[must_use]
pub fn inbox_contains_registration_barrier(
    event: &ReducerInboxEvent<WakeProfile>,
    receipt: &WakeRegistrationReceipt,
) -> bool {
    event.kind == ReducerInboxKind::BarrierSatisfied
        && matches!(
            &event.payload,
            ReducerInboxPayload::Barrier(WakeBarrierEvent::RegistrationObserved { receipt: found })
                if found == receipt
        )
}

#[cfg(test)]
#[path = "wake_profile/tests.rs"]
mod tests;
