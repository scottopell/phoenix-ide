use std::collections::BTreeMap;

use crate::{
    BarrierDecl, BarrierId, BarrierMemberDecl, CancellationRequest, ClaimAuthority, CodecRef,
    EffectAmbiguity, EffectDecl, EffectId, EffectInvalidationDecl, EffectRole, EffectState,
    EffectStatus, Generation, ManualChoice, ManualChoiceKind, ObservationRecord, ProfileRef,
    ProtocolSelection, ReceiptFamily, ReducerDecision, ReducerInboxEvent, ReducerInboxKind,
    ReducerInboxPayload, SemanticAuthority, ShadowDivergenceKind, Timestamp, TransitionPlan,
    Version, WorkflowProfile, WorkflowState, WorkflowStatus,
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
pub const BASH_RESOURCE: &str = "bash";
pub const TMUX_RESOURCE: &str = "tmux";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WakeResourceIdentity {
    Bash { handle_id: &'static str },
    Tmux { session_id: &'static str },
}

impl WakeResourceIdentity {
    #[must_use]
    pub const fn resource_family(self) -> &'static str {
        match self {
            Self::Bash { .. } => BASH_RESOURCE,
            Self::Tmux { .. } => TMUX_RESOURCE,
        }
    }

    #[must_use]
    pub const fn stable_key(self) -> &'static str {
        match self {
            Self::Bash { handle_id } => handle_id,
            Self::Tmux { session_id } => session_id,
        }
    }

    #[must_use]
    pub fn destructive_resource(self) -> &'static str {
        match self {
            Self::Bash { handle_id } => leak_concat(BASH_RESOURCE, handle_id),
            Self::Tmux { session_id } => leak_concat(TMUX_RESOURCE, session_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRegistrationSnapshot {
    pub identity: WakeResourceIdentity,
    pub deadline: Timestamp,
    pub accepted: BusyIdleAcceptance,
    pub continuation: Option<WakeContinuation>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationEvent {
    Registered,
    CancelRequested,
    Continued,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationIntent {
    ObserveHandle {
        identity: WakeResourceIdentity,
        deadline: Timestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalWakeEvidence {
    Exited {
        occurred_at: Timestamp,
        exit_code: i32,
    },
    TmuxFinished {
        occurred_at: Timestamp,
    },
    Missing,
}

impl TerminalWakeEvidence {
    #[must_use]
    pub const fn occurred_at(&self) -> Option<Timestamp> {
        match self {
            Self::Exited { occurred_at, .. } | Self::TmuxFinished { occurred_at } => {
                Some(*occurred_at)
            }
            Self::Missing => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalWakeCause {
    Fired,
    Expired,
    Cancelled,
    Forgotten,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalWakeReceipt {
    pub identity: WakeResourceIdentity,
    pub cause: TerminalWakeCause,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeBarrierEvent {
    RegistrationObserved { identity: WakeResourceIdentity },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyIdleAcceptance {
    Busy,
    Idle,
    Either,
}

impl BusyIdleAcceptance {
    #[must_use]
    pub const fn accepts(self, cause: &TerminalWakeCause) -> bool {
        !matches!(cause, TerminalWakeCause::Cancelled)
    }

    #[must_use]
    pub const fn projects_busy(self) -> bool {
        matches!(self, Self::Busy | Self::Either)
    }

    #[must_use]
    pub const fn projects_idle(self) -> bool {
        matches!(self, Self::Idle | Self::Either)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAvailability {
    Idle,
    Busy,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusyIdleProjection {
    AcceptNow(TerminalWakeCause),
    Defer,
    Suppress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeContinuation {
    pub identity: WakeResourceIdentity,
    pub deadline: Timestamp,
    pub accepted: BusyIdleAcceptance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeLifecycleFence {
    pub workflow_version: Version,
    pub generation: Generation,
    pub effect_id: EffectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeManualPayload {
    AcceptBusy,
    AcceptIdle,
    AcceptDeadline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeShadowParity {
    pub identity: WakeResourceIdentity,
    pub selected_cause: TerminalWakeCause,
    pub divergence_kind: Option<ShadowDivergenceKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeProfile;

impl WorkflowProfile for WakeProfile {
    type Snapshot = WakeRegistrationSnapshot;
    type Event = WakeRegistrationEvent;
    type Intent = WakeRegistrationIntent;
    type Observation = TerminalWakeEvidence;
    type Receipt = TerminalWakeReceipt;
    type ReceiptReducerEvent = TerminalWakeReceipt;
    type BarrierEvent = WakeBarrierEvent;
    type ManualPayload = WakeManualPayload;
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
pub fn barrier_events() -> BTreeMap<BarrierId, WakeBarrierEvent> {
    BTreeMap::from([(
        REGISTRATION_BARRIER_ID,
        WakeBarrierEvent::RegistrationObserved {
            identity: WakeResourceIdentity::Bash {
                handle_id: "registration",
            },
        },
    )])
}

#[must_use]
pub fn registration_barrier_event(identity: WakeResourceIdentity) -> WakeBarrierEvent {
    WakeBarrierEvent::RegistrationObserved { identity }
}

#[must_use]
pub fn registration_snapshot(
    identity: WakeResourceIdentity,
    deadline: Timestamp,
    accepted: BusyIdleAcceptance,
) -> WakeRegistrationSnapshot {
    WakeRegistrationSnapshot {
        identity,
        deadline,
        accepted,
        continuation: None,
        cancelled: false,
    }
}

#[must_use]
pub fn registration_decision(
    expected_workflow_version: Version,
    identity: WakeResourceIdentity,
    deadline: Timestamp,
    accepted: BusyIdleAcceptance,
) -> (
    ReducerDecision<WakeProfile>,
    BTreeMap<BarrierId, WakeBarrierEvent>,
) {
    let snapshot = registration_snapshot(identity, deadline, accepted);
    let effect = EffectDecl {
        effect_id: REGISTRATION_EFFECT_ID,
        family: PROFILE_ID,
        kind: OBSERVE_HANDLE_KIND,
        codec: intent_codec(),
        generation: Generation(0),
        role: EffectRole::Required,
        ambiguity: EffectAmbiguity::ObservableReconciliation,
        intent: WakeRegistrationIntent::ObserveHandle { identity, deadline },
        next_eligible_at: None,
        destructive_resource: None,
    };
    let barrier_event = registration_barrier_event(identity);
    (
        ReducerDecision {
            expected_workflow_version,
            plan: TransitionPlan {
                snapshot,
                snapshot_codec: snapshot_codec(),
                event: WakeRegistrationEvent::Registered,
                event_codec: event_codec(),
                effects: vec![effect],
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
        BTreeMap::from([(REGISTRATION_BARRIER_ID, barrier_event)]),
    )
}

#[must_use]
pub fn cancellation_request(
    workflow: &WorkflowState<WakeProfile>,
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
    let mut snapshot = workflow.snapshot.clone();
    snapshot.cancelled = true;
    CancellationRequest {
        expected_workflow_version: workflow.version,
        next_snapshot: snapshot,
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
pub fn project_runtime_acceptance(
    runtime: RuntimeAvailability,
    cause: TerminalWakeCause,
) -> BusyIdleProjection {
    match (runtime, cause) {
        (RuntimeAvailability::Idle, cause) if cause != TerminalWakeCause::Cancelled => {
            BusyIdleProjection::AcceptNow(cause)
        }
        (RuntimeAvailability::Busy, _) => BusyIdleProjection::Defer,
        (RuntimeAvailability::Terminal, _) | (_, TerminalWakeCause::Cancelled) => {
            BusyIdleProjection::Suppress
        }
        (RuntimeAvailability::Idle, _) => BusyIdleProjection::Suppress,
    }
}

#[must_use]
pub fn receipt_from_evidence(
    identity: WakeResourceIdentity,
    evidence: &TerminalWakeEvidence,
    deadline: Timestamp,
) -> TerminalWakeReceipt {
    let (cause, observed_at) = match evidence.occurred_at() {
        Some(occurred_at) if occurred_at <= deadline => (TerminalWakeCause::Fired, occurred_at),
        Some(_) => (TerminalWakeCause::Expired, deadline),
        None => (TerminalWakeCause::Forgotten, deadline),
    };
    TerminalWakeReceipt {
        identity,
        cause,
        observed_at,
    }
}

#[must_use]
pub fn continuation_from_snapshot(snapshot: &WakeRegistrationSnapshot) -> WakeContinuation {
    WakeContinuation {
        identity: snapshot.identity,
        deadline: snapshot.deadline,
        accepted: snapshot.accepted,
    }
}

#[must_use]
pub fn transfer_continuation(
    snapshot: &WakeRegistrationSnapshot,
    next_accepted: BusyIdleAcceptance,
) -> WakeRegistrationSnapshot {
    WakeRegistrationSnapshot {
        identity: snapshot.identity,
        deadline: snapshot.deadline,
        accepted: next_accepted,
        continuation: Some(continuation_from_snapshot(snapshot)),
        cancelled: snapshot.cancelled,
    }
}

#[must_use]
pub fn lifecycle_fence(
    workflow: &WorkflowState<WakeProfile>,
    effect_id: EffectId,
) -> Option<WakeLifecycleFence> {
    workflow
        .effects
        .get(&effect_id)
        .map(|_| WakeLifecycleFence {
            workflow_version: workflow.version,
            generation: workflow.generation,
            effect_id,
        })
}

#[must_use]
pub fn fence_accepts(workflow: &WorkflowState<WakeProfile>, fence: &WakeLifecycleFence) -> bool {
    workflow.version == fence.workflow_version
        && workflow.generation == fence.generation
        && workflow.effects.contains_key(&fence.effect_id)
        && workflow.status == WorkflowStatus::Active
}

#[must_use]
pub fn shadow_parity(
    authoritative: &TerminalWakeReceipt,
    shadow: &TerminalWakeReceipt,
) -> WakeShadowParity {
    WakeShadowParity {
        identity: authoritative.identity,
        selected_cause: authoritative.cause.clone(),
        divergence_kind: (authoritative != shadow).then_some(ShadowDivergenceKind::Receipt),
    }
}

#[must_use]
pub fn manual_choices() -> Vec<ManualChoice<WakeProfile>> {
    vec![
        ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: manual_codec(),
            payload: WakeManualPayload::AcceptBusy,
        },
        ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: manual_codec(),
            payload: WakeManualPayload::AcceptIdle,
        },
        ManualChoice {
            kind: ManualChoiceKind::Adopt,
            codec: manual_codec(),
            payload: WakeManualPayload::AcceptDeadline,
        },
    ]
}

#[must_use]
pub fn authoritative_observation<'a>(
    authority: &ClaimAuthority,
    effect: &'a EffectState<WakeProfile>,
) -> Option<&'a ObservationRecord<TerminalWakeEvidence>> {
    effect
        .observations
        .iter()
        .find(|observation| observation.authority == *authority && observation.authoritative)
}

#[must_use]
pub fn inbox_contains_registration_barrier(
    event: &ReducerInboxEvent<WakeProfile>,
    identity: WakeResourceIdentity,
) -> bool {
    event.kind == ReducerInboxKind::BarrierSatisfied
        && matches!(
            &event.payload,
            ReducerInboxPayload::Barrier(WakeBarrierEvent::RegistrationObserved { identity: found })
                if *found == identity
        )
}

fn leak_concat(prefix: &str, value: &str) -> &'static str {
    Box::leak(format!("{prefix}:{value}").into_boxed_str())
}

#[cfg(test)]
#[path = "wake_profile/tests.rs"]
mod tests;
