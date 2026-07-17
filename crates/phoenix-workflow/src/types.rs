use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkflowId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BarrierId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransitionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttemptId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObservationId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReceiptId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeliveryId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManualResolutionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScheduleId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessIncarnation(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StableCommandId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u64);

impl Version {
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);

impl Generation {
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseExpiry(pub u64);

impl LeaseExpiry {
    pub const MAX_FINITE: Self = Self(u64::MAX - 1);

    #[must_use]
    pub const fn finite(value: u64) -> Option<Self> {
        if value < u64::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub fn is_live_at(self, now: Timestamp) -> bool {
        now.0 < self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileRef {
    pub profile_kind: &'static str,
    pub profile_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CodecRef {
    pub family: &'static str,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedCodecRegistry(BTreeSet<CodecRef>);

impl SupportedCodecRegistry {
    #[must_use]
    pub fn new(codecs: impl IntoIterator<Item = CodecRef>) -> Option<Self> {
        let codecs = codecs.into_iter().collect::<BTreeSet<_>>();
        (!codecs.is_empty() && codecs.iter().all(|codec| !codec.family.is_empty()))
            .then_some(Self(codecs))
    }

    #[must_use]
    pub fn supports(&self, codec: &CodecRef) -> bool {
        self.0.contains(codec)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulePolicy {
    CoalesceLatest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStatus {
    Idle,
    Due,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowStatus {
    Active,
    Cancelling,
    ManualResolution,
    Incompatible,
    Cancelled,
    DeletionPending,
    Deleted,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectRole {
    Required,
    Optional,
    Compensation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptFamily {
    CurrentGenerationEffect,
    CompensationEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectStatus {
    Blocked,
    Eligible,
    Executing,
    RetryWait,
    AmbiguityWait,
    Receipted,
    Invalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptStatus {
    Begun,
    ObservationRecorded,
    ReceiptAccepted,
    AuthorityLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptOrigin {
    Execution,
    Adoption,
    Reconciliation,
    Manual,
    CancellationArbitration,
    ScheduleCollapse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierStatus {
    Waiting,
    Satisfied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed,
    VersionConflict,
    InvalidPlan,
    UnsupportedCodec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Started,
    Ineligible,
    AuthorityConflict,
    UnsupportedCodec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityOutcome {
    Authorized,
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalResult {
    pub outcome: AuthorityOutcome,
    pub authority: Option<AttemptAuthority>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Pending,
    Accepted,
    Suppressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAcceptanceStatus {
    Owed,
    Accepted,
    Suppressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    Cancelled,
    Superseded,
    LifecycleTerminal,
    ReducerTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Required,
    Resolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualChoiceKind {
    Retry,
    Compensate,
    Suppress,
    AcceptAsTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationDecision {
    Perform,
    Adopt,
    Repair,
    Compensate,
    DurableConflict,
    RequestManualResolution,
    RetryInfrastructure,
    StopAuthorityLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionCapability {
    ReclaimableObservation,
    IdempotentSubmission { stable_command_id: StableCommandId },
    ObservableSubmission { stable_command_id: StableCommandId },
    SafelyRepeatable,
    ManualOnAmbiguity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceProfile {
    pub profile: ProfileRef,
    pub supported_codecs: SupportedCodecRegistry,
    pub runtime_acceptance_enabled: bool,
    pub external_acceptance_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NonEmptyExternalKey(String);

impl NonEmptyExternalKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalAcceptanceKey {
    pub profile: ProfileRef,
    pub authority_scope: NonEmptyExternalKey,
    pub idempotency_key: NonEmptyExternalKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAcceptanceReceipt<H> {
    pub idempotency_key: NonEmptyExternalKey,
    pub workflow_id: WorkflowId,
    pub handle: H,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAcceptanceBinding<H> {
    pub profile: ProfileRef,
    pub authority_scope: NonEmptyExternalKey,
    pub idempotency_key: NonEmptyExternalKey,
    pub intent_fingerprint: String,
    pub receipt: ExternalAcceptanceReceipt<H>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAcceptanceOutcome<H> {
    New(ExternalAcceptanceReceipt<H>),
    Replay(ExternalAcceptanceReceipt<H>),
    Conflict,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowBinding {
    pub workflow_id: WorkflowId,
    pub profile: ProfileRef,
    pub acceptance: AcceptanceProfile,
}

pub trait WorkflowProfile {
    type Snapshot: Clone + Eq + std::fmt::Debug;
    type Event: Clone + Eq + std::fmt::Debug;
    type Intent: Clone + Eq + std::fmt::Debug;
    type Observation: Clone + Eq + std::fmt::Debug;
    type Receipt: Clone + Eq + std::fmt::Debug;
    type ReceiptReducerEvent: Clone + Eq + std::fmt::Debug;
    type BarrierEvent: Clone + Eq + std::fmt::Debug;
    type ManualPayload: Clone + Eq + std::fmt::Debug;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool;

    fn receipt_requires_runtime_acceptance(event: &Self::ReceiptReducerEvent) -> bool;

    fn decision_handles_delivery(item: &DeliveryItem<Self>, decision_event: &Self::Event) -> bool
    where
        Self: Sized;

    fn decision_handles_runtime_acceptance(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool
    where
        Self: Sized;

    fn decision_handles_runtime_suppression(
        item: &DeliveryItem<Self>,
        decision_event: &Self::Event,
    ) -> bool
    where
        Self: Sized;
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeliveryPayload<P: WorkflowProfile> {
    Receipt(P::ReceiptReducerEvent),
    Barrier(P::BarrierEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDecl<I> {
    pub effect_id: EffectId,
    pub family: &'static str,
    pub kind: &'static str,
    pub codec: CodecRef,
    pub generation: Generation,
    pub role: EffectRole,
    pub capability: ExecutionCapability,
    pub intent: I,
    pub next_eligible_at: Option<Timestamp>,
    pub destructive_resource: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DependencyDecl {
    pub effect_id: EffectId,
    pub depends_on_effect_id: EffectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierDecl {
    pub barrier_id: BarrierId,
    pub reducer_event_codec: CodecRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarrierMemberDecl {
    pub barrier_id: BarrierId,
    pub effect_id: EffectId,
    pub receipt_family: ReceiptFamily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectInvalidationDecl {
    pub effect_id: EffectId,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryDecl<P: WorkflowProfile> {
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub consumer_kind: &'static str,
    pub event_codec: CodecRef,
    pub requires_runtime_acceptance: bool,
    pub payload: DeliveryPayload<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleDecl {
    pub schedule_id: ScheduleId,
    pub policy: SchedulePolicy,
    pub next_eligible_at: Timestamp,
    pub key: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TransitionPlan<P: WorkflowProfile> {
    pub next_status: WorkflowStatus,
    pub snapshot: P::Snapshot,
    pub snapshot_codec: CodecRef,
    pub event: P::Event,
    pub event_codec: CodecRef,
    pub effects: Vec<EffectDecl<P::Intent>>,
    pub dependencies: Vec<DependencyDecl>,
    pub barriers: Vec<BarrierDecl>,
    pub barrier_members: Vec<BarrierMemberDecl>,
    pub invalidations: Vec<EffectInvalidationDecl>,
    pub deliveries: Vec<DeliveryDecl<P>>,
    pub schedules: Vec<ScheduleDecl>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReducerDecision<P: WorkflowProfile> {
    pub expected_workflow_version: Version,
    pub plan: TransitionPlan<P>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryDecisionBinding<P: WorkflowProfile> {
    pub items: Vec<DeliveryItem<P>>,
    pub decision: ReducerDecision<P>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CancellationReceiptDecl<P: WorkflowProfile> {
    pub effect_id: EffectId,
    pub receipt_codec: CodecRef,
    pub receipt: P::Receipt,
    pub event_codec: CodecRef,
    pub event: P::ReceiptReducerEvent,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CancellationRequest<P: WorkflowProfile> {
    pub expected_workflow_version: Version,
    pub next_snapshot: P::Snapshot,
    pub next_snapshot_codec: CodecRef,
    pub event: P::Event,
    pub event_codec: CodecRef,
    pub invalidations: Vec<EffectInvalidationDecl>,
    pub terminal_receipt: Option<CancellationReceiptDecl<P>>,
    pub compensation_plan: TransitionPlan<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptAuthority {
    pub workflow_id: WorkflowId,
    pub declared_workflow_version: Version,
    pub generation: Generation,
    pub effect_id: EffectId,
    pub attempt_id: AttemptId,
    pub process_incarnation: ProcessIncarnation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimableLease {
    pub attempt_id: AttemptId,
    pub lease_until: LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLockGrant {
    pub resource: &'static str,
    pub generation: Generation,
    pub lease_until: LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRecord {
    pub id: AttemptId,
    pub ordinal: u32,
    pub authority: AttemptAuthority,
    pub status: AttemptStatus,
    pub lease: Option<ReclaimableLease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRecord<O> {
    pub id: ObservationId,
    pub authority: AttemptAuthority,
    pub attempt_id: AttemptId,
    pub observation_codec: CodecRef,
    pub observation: O,
    pub observed_at: Timestamp,
    pub recorded_at: Timestamp,
    pub authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleObservationRecord<O> {
    pub id: ObservationId,
    pub authority: AttemptAuthority,
    pub attempt_id: AttemptId,
    pub observation_codec: CodecRef,
    pub observed_at: Timestamp,
    pub recorded_at: Timestamp,
    pub observation: O,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRecord<R> {
    pub id: ReceiptId,
    pub authority: AttemptAuthority,
    pub attempt_id: Option<AttemptId>,
    pub origin: ReceiptOrigin,
    pub receipt_codec: CodecRef,
    pub receipt: R,
    pub generation: Generation,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryItem<P: WorkflowProfile> {
    pub id: DeliveryId,
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub consumer_kind: &'static str,
    pub event_codec: CodecRef,
    pub requires_runtime_acceptance: bool,
    pub payload: DeliveryPayload<P>,
    pub status: DeliveryStatus,
    pub runtime_acceptance_status: Option<RuntimeAcceptanceStatus>,
    pub suppression_reason: Option<SuppressionReason>,
    pub accepted_by: Option<TransitionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTransition<E> {
    pub transition_id: TransitionId,
    pub from_version: Version,
    pub to_version: Version,
    pub generation: Generation,
    pub event: E,
    pub event_codec: CodecRef,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommitResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub deliveries: Vec<DeliveryItem<P>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryConsumeResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub consumed_delivery_ids: Vec<DeliveryId>,
    pub deliveries: Vec<DeliveryItem<P>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeAcceptanceResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub delivery: Option<DeliveryItem<P>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimResult {
    pub outcome: ClaimOutcome,
    pub authority: Option<AttemptAuthority>,
    pub attempt: Option<AttemptRecord>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReceiptAcceptance<P: WorkflowProfile> {
    pub outcome: AuthorityOutcome,
    pub receipt: Option<ReceiptRecord<P::Receipt>>,
    pub delivery_ids: Vec<DeliveryId>,
    pub deliveries: Vec<DeliveryItem<P>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BarrierEvaluation<P: WorkflowProfile> {
    pub newly_satisfied: Vec<BarrierId>,
    pub deliveries: Vec<DeliveryItem<P>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ManualChoice<P: WorkflowProfile>
where
    P::ManualPayload: Clone + Eq,
{
    pub kind: ManualChoiceKind,
    pub codec: CodecRef,
    pub payload: P::ManualPayload,
    pub receipt_codec: CodecRef,
    pub receipt: P::Receipt,
    pub receipt_event_codec: CodecRef,
    pub receipt_event: P::ReceiptReducerEvent,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ManualResolutionRecord<P: WorkflowProfile>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
{
    pub id: ManualResolutionId,
    pub workflow_version: Version,
    pub effect_id: EffectId,
    pub status: ResolutionStatus,
    pub evidence: Vec<ObservationRecord<P::Observation>>,
    pub permitted_choices: Vec<ManualChoice<P>>,
    pub accepted_choice: Option<ManualChoice<P>>,
    pub resolved_by: Option<&'static str>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ManualResolutionCommit<P: WorkflowProfile> {
    pub transition_codec: CodecRef,
    pub transition_event: P::Event,
    pub next_status: WorkflowStatus,
    pub retry_at: Option<Timestamp>,
    pub compensation_effects: Vec<EffectDecl<P::Intent>>,
    pub compensation_dependencies: Vec<DependencyDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationOutcome {
    pub outcome: AuthorityOutcome,
    pub preserved_winner: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManualEffectOutcome<P: WorkflowProfile> {
    Receipt {
        receipt: Box<ReceiptRecord<P::Receipt>>,
        reducer_event: Box<DeliveryItem<P>>,
    },
    Retry,
    Compensate,
    Failed,
    Suppressed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ManualResolutionOutcome<P: WorkflowProfile>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
    P::Receipt: Clone + Eq,
    P::BarrierEvent: Clone + Eq,
{
    pub outcome: CommitOutcome,
    pub resolution: Option<ManualResolutionRecord<P>>,
    pub effect_outcome: Option<ManualEffectOutcome<P>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReconciliationOutcome<P: WorkflowProfile>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
{
    pub outcome: AuthorityOutcome,
    pub decision: Option<ReconciliationDecision>,
    pub manual_resolution: Option<ManualResolutionRecord<P>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMigrationOutcome {
    UpToDate,
    Migrated,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibleWorkflow {
    pub workflow_id: WorkflowId,
    pub stored_profile: ProfileRef,
    pub detected_at: Timestamp,
    pub disposition: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectState<P: WorkflowProfile> {
    pub declaration: EffectDecl<P::Intent>,
    pub declared_workflow_version: Version,
    pub status: EffectStatus,
    pub dependencies: BTreeSet<EffectId>,
    pub authority: Option<AttemptAuthority>,
    pub reclaimable_lease: Option<ReclaimableLease>,
    pub attempts: Vec<AttemptRecord>,
    pub observations: Vec<ObservationRecord<P::Observation>>,
    pub stale_observations: Vec<StaleObservationRecord<P::Observation>>,
    pub receipt: Option<ReceiptRecord<P::Receipt>>,
    pub pending_reconciliation: bool,
    pub destructive_lock: Option<ResourceLockGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierState<P: WorkflowProfile> {
    pub barrier_id: BarrierId,
    pub status: BarrierStatus,
    pub required_members: BTreeMap<EffectId, ReceiptFamily>,
    pub reducer_event_codec: CodecRef,
    pub reducer_event: P::BarrierEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleState {
    pub schedule_id: ScheduleId,
    pub policy: SchedulePolicy,
    pub key: &'static str,
    pub status: ScheduleStatus,
    pub next_eligible_at: Timestamp,
    pub active_effect_id: Option<EffectId>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WorkflowState<P: WorkflowProfile> {
    pub binding: WorkflowBinding,
    pub version: Version,
    pub generation: Generation,
    pub status: WorkflowStatus,
    pub snapshot: P::Snapshot,
    pub snapshot_codec: CodecRef,
    pub effects: BTreeMap<EffectId, EffectState<P>>,
    pub barriers: BTreeMap<BarrierId, BarrierState<P>>,
    pub deliveries: BTreeMap<DeliveryId, DeliveryItem<P>>,
    pub schedules: BTreeMap<ScheduleId, ScheduleState>,
    pub manual_resolutions: BTreeMap<ManualResolutionId, ManualResolutionRecord<P>>,
    pub transition_log: Vec<WorkflowTransition<P::Event>>,
    pub process_incarnation: ProcessIncarnation,
    pub incompatible: Option<IncompatibleWorkflow>,
    pub crashed_workers: BTreeSet<&'static str>,
    pub(crate) next_transition_id: u64,
    pub(crate) next_attempt_id: u64,
    pub(crate) next_observation_id: u64,
    pub(crate) next_receipt_id: u64,
    pub(crate) next_delivery_id: u64,
    pub(crate) next_manual_resolution_id: u64,
}

impl<P: WorkflowProfile> Clone for WorkflowState<P> {
    fn clone(&self) -> Self {
        Self {
            binding: self.binding.clone(),
            version: self.version,
            generation: self.generation,
            status: self.status,
            snapshot: self.snapshot.clone(),
            snapshot_codec: self.snapshot_codec.clone(),
            effects: self
                .effects
                .iter()
                .map(|(effect_id, effect)| (*effect_id, clone_effect_state(effect)))
                .collect(),
            barriers: self
                .barriers
                .iter()
                .map(|(barrier_id, barrier)| (*barrier_id, clone_barrier_state(barrier)))
                .collect(),
            deliveries: self
                .deliveries
                .iter()
                .map(|(delivery_id, item)| (*delivery_id, clone_delivery_item(item)))
                .collect(),
            schedules: self.schedules.clone(),
            manual_resolutions: self
                .manual_resolutions
                .iter()
                .map(|(resolution_id, resolution)| {
                    (*resolution_id, clone_manual_resolution(resolution))
                })
                .collect(),
            transition_log: self.transition_log.clone(),
            process_incarnation: self.process_incarnation,
            incompatible: self.incompatible.clone(),
            crashed_workers: self.crashed_workers.clone(),
            next_transition_id: self.next_transition_id,
            next_attempt_id: self.next_attempt_id,
            next_observation_id: self.next_observation_id,
            next_receipt_id: self.next_receipt_id,
            next_delivery_id: self.next_delivery_id,
            next_manual_resolution_id: self.next_manual_resolution_id,
        }
    }
}

fn clone_delivery_item<P: WorkflowProfile>(item: &DeliveryItem<P>) -> DeliveryItem<P> {
    DeliveryItem {
        id: item.id,
        effect_id: item.effect_id,
        barrier_id: item.barrier_id,
        consumer_kind: item.consumer_kind,
        event_codec: item.event_codec.clone(),
        requires_runtime_acceptance: item.requires_runtime_acceptance,
        payload: match &item.payload {
            DeliveryPayload::Receipt(payload) => DeliveryPayload::Receipt(payload.clone()),
            DeliveryPayload::Barrier(payload) => DeliveryPayload::Barrier(payload.clone()),
        },
        status: item.status,
        runtime_acceptance_status: item.runtime_acceptance_status,
        suppression_reason: item.suppression_reason,
        accepted_by: item.accepted_by,
    }
}

fn clone_manual_choice<P: WorkflowProfile>(choice: &ManualChoice<P>) -> ManualChoice<P> {
    ManualChoice {
        kind: choice.kind,
        codec: choice.codec.clone(),
        payload: choice.payload.clone(),
        receipt_codec: choice.receipt_codec.clone(),
        receipt: choice.receipt.clone(),
        receipt_event_codec: choice.receipt_event_codec.clone(),
        receipt_event: choice.receipt_event.clone(),
    }
}

fn clone_manual_resolution<P: WorkflowProfile>(
    resolution: &ManualResolutionRecord<P>,
) -> ManualResolutionRecord<P> {
    ManualResolutionRecord {
        id: resolution.id,
        workflow_version: resolution.workflow_version,
        effect_id: resolution.effect_id,
        status: resolution.status,
        evidence: resolution.evidence.clone(),
        permitted_choices: resolution
            .permitted_choices
            .iter()
            .map(clone_manual_choice)
            .collect(),
        accepted_choice: resolution.accepted_choice.as_ref().map(clone_manual_choice),
        resolved_by: resolution.resolved_by,
    }
}

fn clone_effect_state<P: WorkflowProfile>(effect: &EffectState<P>) -> EffectState<P> {
    EffectState {
        declaration: effect.declaration.clone(),
        declared_workflow_version: effect.declared_workflow_version,
        status: effect.status,
        dependencies: effect.dependencies.clone(),
        authority: effect.authority.clone(),
        reclaimable_lease: effect.reclaimable_lease.clone(),
        attempts: effect.attempts.clone(),
        observations: effect.observations.clone(),
        stale_observations: effect.stale_observations.clone(),
        receipt: effect.receipt.clone(),
        pending_reconciliation: effect.pending_reconciliation,
        destructive_lock: effect.destructive_lock.clone(),
    }
}

fn clone_barrier_state<P: WorkflowProfile>(barrier: &BarrierState<P>) -> BarrierState<P> {
    BarrierState {
        barrier_id: barrier.barrier_id,
        status: barrier.status,
        required_members: barrier.required_members.clone(),
        reducer_event_codec: barrier.reducer_event_codec.clone(),
        reducer_event: barrier.reducer_event.clone(),
    }
}

impl<P: WorkflowProfile> Clone for DeliveryPayload<P> {
    fn clone(&self) -> Self {
        match self {
            Self::Receipt(event) => Self::Receipt(event.clone()),
            Self::Barrier(event) => Self::Barrier(event.clone()),
        }
    }
}

impl<P: WorkflowProfile> Clone for TransitionPlan<P> {
    fn clone(&self) -> Self {
        Self {
            next_status: self.next_status,
            snapshot: self.snapshot.clone(),
            snapshot_codec: self.snapshot_codec.clone(),
            event: self.event.clone(),
            event_codec: self.event_codec.clone(),
            effects: self.effects.clone(),
            dependencies: self.dependencies.clone(),
            barriers: self.barriers.clone(),
            barrier_members: self.barrier_members.clone(),
            invalidations: self.invalidations.clone(),
            deliveries: self.deliveries.clone(),
            schedules: self.schedules.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for ReducerDecision<P> {
    fn clone(&self) -> Self {
        Self {
            expected_workflow_version: self.expected_workflow_version,
            plan: self.plan.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for DeliveryDecisionBinding<P> {
    fn clone(&self) -> Self {
        Self {
            items: self.items.iter().map(clone_delivery_item).collect(),
            decision: self.decision.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for CancellationRequest<P> {
    fn clone(&self) -> Self {
        Self {
            expected_workflow_version: self.expected_workflow_version,
            next_snapshot: self.next_snapshot.clone(),
            next_snapshot_codec: self.next_snapshot_codec.clone(),
            event: self.event.clone(),
            event_codec: self.event_codec.clone(),
            invalidations: self.invalidations.clone(),
            terminal_receipt: self.terminal_receipt.clone(),
            compensation_plan: self.compensation_plan.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for DeliveryItem<P> {
    fn clone(&self) -> Self {
        clone_delivery_item(self)
    }
}

impl<P: WorkflowProfile> Clone for CommitResult<P> {
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome,
            transition: self.transition.clone(),
            deliveries: self.deliveries.iter().map(clone_delivery_item).collect(),
        }
    }
}

impl<P: WorkflowProfile> Clone for DeliveryConsumeResult<P> {
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome,
            transition: self.transition.clone(),
            consumed_delivery_ids: self.consumed_delivery_ids.clone(),
            deliveries: self.deliveries.iter().map(clone_delivery_item).collect(),
        }
    }
}

impl<P: WorkflowProfile> Clone for RuntimeAcceptanceResult<P> {
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome,
            transition: self.transition.clone(),
            delivery: self.delivery.as_ref().map(clone_delivery_item),
        }
    }
}

impl<P: WorkflowProfile> Clone for ReceiptAcceptance<P> {
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome,
            receipt: self.receipt.clone(),
            delivery_ids: self.delivery_ids.clone(),
            deliveries: self.deliveries.iter().map(clone_delivery_item).collect(),
        }
    }
}

impl<P: WorkflowProfile> Clone for BarrierEvaluation<P> {
    fn clone(&self) -> Self {
        Self {
            newly_satisfied: self.newly_satisfied.clone(),
            deliveries: self.deliveries.iter().map(clone_delivery_item).collect(),
        }
    }
}

impl<P: WorkflowProfile> Clone for ManualResolutionRecord<P>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
{
    fn clone(&self) -> Self {
        clone_manual_resolution(self)
    }
}

impl<P: WorkflowProfile> Clone for ManualResolutionCommit<P> {
    fn clone(&self) -> Self {
        Self {
            transition_codec: self.transition_codec.clone(),
            transition_event: self.transition_event.clone(),
            next_status: self.next_status,
            retry_at: self.retry_at,
            compensation_effects: self.compensation_effects.clone(),
            compensation_dependencies: self.compensation_dependencies.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for ManualEffectOutcome<P> {
    fn clone(&self) -> Self {
        match self {
            Self::Receipt {
                receipt,
                reducer_event,
            } => Self::Receipt {
                receipt: Box::new((**receipt).clone()),
                reducer_event: Box::new(clone_delivery_item(reducer_event)),
            },
            Self::Retry => Self::Retry,
            Self::Compensate => Self::Compensate,
            Self::Failed => Self::Failed,
            Self::Suppressed => Self::Suppressed,
        }
    }
}

impl<P: WorkflowProfile> Clone for ManualResolutionOutcome<P>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
    P::Receipt: Clone + Eq,
    P::BarrierEvent: Clone + Eq,
{
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome,
            resolution: self.resolution.as_ref().map(clone_manual_resolution),
            effect_outcome: self.effect_outcome.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for ReconciliationOutcome<P>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
{
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome,
            decision: self.decision,
            manual_resolution: self.manual_resolution.as_ref().map(clone_manual_resolution),
        }
    }
}

impl<P: WorkflowProfile> Clone for DeliveryDecl<P> {
    fn clone(&self) -> Self {
        Self {
            effect_id: self.effect_id,
            barrier_id: self.barrier_id,
            consumer_kind: self.consumer_kind,
            event_codec: self.event_codec.clone(),
            requires_runtime_acceptance: self.requires_runtime_acceptance,
            payload: self.payload.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for CancellationReceiptDecl<P> {
    fn clone(&self) -> Self {
        Self {
            effect_id: self.effect_id,
            receipt_codec: self.receipt_codec.clone(),
            receipt: self.receipt.clone(),
            event_codec: self.event_codec.clone(),
            event: self.event.clone(),
        }
    }
}

impl<P: WorkflowProfile> Clone for ManualChoice<P>
where
    P::ManualPayload: Clone + Eq,
{
    fn clone(&self) -> Self {
        clone_manual_choice(self)
    }
}
