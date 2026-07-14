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
pub struct ReducerInboxId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwedAcceptanceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManualResolutionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShadowDivergenceId(pub u64);

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
    #[must_use]
    pub fn is_live_at(self, now: Timestamp) -> bool {
        now.0 < self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileRef {
    pub profile_id: &'static str,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecRef {
    pub family: &'static str,
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticAuthority {
    LegacyProtocol,
    EngineProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Authoritative,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowStatus {
    Active,
    Cancelling,
    Cancelled,
    DeletionPending,
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
    Claimed,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed,
    Ineligible,
    AuthorityConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityOutcome {
    Authorized,
    StaleAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalResult {
    pub outcome: AuthorityOutcome,
    pub authority: Option<ClaimAuthority>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReducerInboxKind {
    ReceiptAccepted,
    BarrierSatisfied,
    ManualResolutionRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Pending,
    Consumed,
    Suppressed { reason: SuppressionReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptanceStatus {
    Owed,
    Accepted,
    Suppressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    Cancelled,
    Superseded,
    LifecycleTerminal,
    OperatorRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwedAcceptanceDisposition {
    Owed,
    Accepted {
        transition: TransitionId,
    },
    Suppressed {
        transition: TransitionId,
        reason: SuppressionReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Required,
    Resolved,
    Suppressed {
        transition: TransitionId,
        reason: SuppressionReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualChoiceKind {
    Adopt,
    Retry,
    Compensate,
    Fail,
    Suppress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceSeverity {
    Blocking,
    Actionable,
    Informational,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceAction {
    HaltAcceptance,
    RetainAuthorityAndInvestigate,
    RecordOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceResolutionAction {
    Rollback,
    Reauthorize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowDivergenceKind {
    Snapshot,
    Transition,
    EffectPlan,
    Observation,
    Receipt,
    ReducerEvent,
    Capability,
    UserProjection,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectAmbiguity {
    ObservableReconciliation,
    ExternalIdempotency,
    SafeRepeatability,
    ManualResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolSelection {
    pub profile: ProfileRef,
    pub authority: SemanticAuthority,
    pub accepting: bool,
    pub runtime_acceptance_enabled: bool,
    pub external_acceptance_enabled: bool,
    pub selector: &'static str,
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
pub struct ProtocolSelectionIdentity {
    pub selector: &'static str,
    pub authority: SemanticAuthority,
    pub profile: ProfileRef,
}

impl From<&ProtocolSelection> for ProtocolSelectionIdentity {
    fn from(selection: &ProtocolSelection) -> Self {
        Self {
            selector: selection.selector,
            authority: selection.authority,
            profile: selection.profile.clone(),
        }
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
    pub accepted_protocol: ProtocolSelectionIdentity,
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
pub struct AuthoritativeWorkflow {
    pub workflow_id: WorkflowId,
    pub version: Version,
    pub generation: Generation,
    pub profile: ProfileRef,
    pub accepted_protocol: ProtocolSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowWorkflow {
    pub workflow_id: WorkflowId,
    pub authoritative_workflow_id: WorkflowId,
    pub profile: ProfileRef,
    pub accepted_protocol: ProtocolSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowBinding {
    Authoritative(AuthoritativeWorkflow),
    Shadow(ShadowWorkflow),
}

impl WorkflowBinding {
    #[must_use]
    pub fn execution_mode(&self) -> ExecutionMode {
        match self {
            Self::Authoritative(_) => ExecutionMode::Authoritative,
            Self::Shadow(_) => ExecutionMode::Shadow,
        }
    }

    #[must_use]
    pub fn workflow_id(&self) -> WorkflowId {
        match self {
            Self::Authoritative(workflow) => workflow.workflow_id,
            Self::Shadow(workflow) => workflow.workflow_id,
        }
    }

    #[must_use]
    pub fn accepted_protocol(&self) -> &ProtocolSelection {
        match self {
            Self::Authoritative(workflow) => &workflow.accepted_protocol,
            Self::Shadow(workflow) => &workflow.accepted_protocol,
        }
    }
}

impl EffectAmbiguity {
    #[must_use]
    pub const fn is_manual_only(self) -> bool {
        matches!(self, Self::ManualResolution)
    }
}

pub trait WorkflowProfile {
    type Snapshot: Clone + Eq + std::fmt::Debug;
    type Event: Clone + Eq + std::fmt::Debug;
    type Intent: Clone + Eq + std::fmt::Debug;
    type Observation: Clone + Eq + std::fmt::Debug;
    type Receipt: Clone + Eq + std::fmt::Debug;
    type ReceiptReducerEvent: Clone + Eq + std::fmt::Debug;
    type BarrierEvent: Clone + Eq + std::fmt::Debug;
    type OwedAcceptanceEvent: Clone + Eq + std::fmt::Debug;
    type ManualPayload: Clone + Eq + std::fmt::Debug;

    fn runtime_start_allowed(snapshot: &Self::Snapshot) -> bool;

    fn receipt_requires_runtime_acceptance(event: &Self::ReceiptReducerEvent) -> bool;

    fn decision_handles_inbox(
        event: &ReducerInboxPayload<Self>,
        decision_event: &Self::Event,
    ) -> bool
    where
        Self: Sized;

    fn owed_acceptance_matches_inbox(
        event: &Self::OwedAcceptanceEvent,
        inbox_payload: &ReducerInboxPayload<Self>,
    ) -> bool
    where
        Self: Sized;

    fn decision_handles_owed_acceptance(
        event: &Self::OwedAcceptanceEvent,
        decision_event: &Self::Event,
    ) -> bool;

    fn decision_handles_owed_acceptance_suppression(
        event: &Self::OwedAcceptanceEvent,
        decision_event: &Self::Event,
    ) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReducerInboxPayload<P: WorkflowProfile> {
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
    pub ambiguity: EffectAmbiguity,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedAcceptanceDecl<E> {
    pub reducer_inbox_id: ReducerInboxId,
    pub source_kind: &'static str,
    pub event_codec: CodecRef,
    pub event: E,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub owed_acceptances: Option<Vec<OwedAcceptanceDecl<P::OwedAcceptanceEvent>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerDecision<P: WorkflowProfile> {
    pub expected_workflow_version: Version,
    pub plan: TransitionPlan<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxDecisionBinding<P: WorkflowProfile> {
    pub inbox: Vec<ReducerInboxEvent<P>>,
    pub decision: ReducerDecision<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedAcceptanceDecisionBinding<P: WorkflowProfile> {
    pub owed: OwedAcceptanceRecord<P::OwedAcceptanceEvent>,
    pub decision: ReducerDecision<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationRequest<P: WorkflowProfile> {
    pub expected_workflow_version: Version,
    pub next_snapshot: P::Snapshot,
    pub next_snapshot_codec: CodecRef,
    pub event: P::Event,
    pub event_codec: CodecRef,
    pub invalidations: Vec<EffectInvalidationDecl>,
    pub reducer_inbox_events: Vec<ReducerInboxDecl<P>>,
    pub compensation_plan: TransitionPlan<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerInboxDecl<P: WorkflowProfile> {
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub kind: ReducerInboxKind,
    pub event_codec: CodecRef,
    pub requires_runtime_acceptance: bool,
    pub payload: ReducerInboxPayload<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimAuthority {
    pub workflow_id: WorkflowId,
    pub declared_workflow_version: Version,
    pub generation: Generation,
    pub effect_id: EffectId,
    pub claim_token: u64,
    pub worker_id: &'static str,
    pub lease_until: LeaseExpiry,
    pub resource_lock: Option<ResourceLockGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLockGrant {
    pub resource: &'static str,
    pub worker_id: &'static str,
    pub claim_token: u64,
    pub generation: Generation,
    pub lease_until: LeaseExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRecord {
    pub id: AttemptId,
    pub ordinal: u32,
    pub authority: ClaimAuthority,
    pub status: AttemptStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRecord<O> {
    pub id: ObservationId,
    pub authority: ClaimAuthority,
    pub attempt_id: AttemptId,
    pub observation_codec: CodecRef,
    pub observation: O,
    pub authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleObservationRecord<O> {
    pub id: ObservationId,
    pub authority: ClaimAuthority,
    pub attempt_id: AttemptId,
    pub observation_codec: CodecRef,
    pub observation: O,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRecord<R> {
    pub id: ReceiptId,
    pub authority: ClaimAuthority,
    pub attempt_id: Option<AttemptId>,
    pub origin: ReceiptOrigin,
    pub receipt_codec: CodecRef,
    pub receipt: R,
    pub generation: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerInboxEvent<P: WorkflowProfile> {
    pub id: ReducerInboxId,
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub kind: ReducerInboxKind,
    pub event_codec: CodecRef,
    pub requires_runtime_acceptance: bool,
    pub payload: ReducerInboxPayload<P>,
    pub delivery_status: DeliveryStatus,
    pub consumed_by: Option<TransitionId>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub reducer_events: Vec<ReducerInboxEvent<P>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicInboxConsumeResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub consumed_inbox_ids: Vec<ReducerInboxId>,
    pub reducer_events: Vec<ReducerInboxEvent<P>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAcceptanceResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub owed_acceptance: Option<OwedAcceptanceRecord<P::OwedAcceptanceEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimResult {
    pub outcome: ClaimOutcome,
    pub authority: Option<ClaimAuthority>,
    pub attempt: Option<AttemptRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptAcceptance<P: WorkflowProfile> {
    pub outcome: AuthorityOutcome,
    pub receipt: Option<ReceiptRecord<P::Receipt>>,
    pub receipt_inbox_ids: Vec<ReducerInboxId>,
    pub reducer_events: Vec<ReducerInboxEvent<P>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierEvaluation<P: WorkflowProfile> {
    pub newly_satisfied: Vec<BarrierId>,
    pub reducer_events: Vec<ReducerInboxEvent<P>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualResolutionCommit<P: WorkflowProfile> {
    pub transition_codec: CodecRef,
    pub transition_event: P::Event,
    pub next_status: WorkflowStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationOutcome {
    pub outcome: AuthorityOutcome,
    pub preserved_winner: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualEffectOutcome<P: WorkflowProfile> {
    Receipt {
        receipt: Box<ReceiptRecord<P::Receipt>>,
        reducer_event: Box<ReducerInboxEvent<P>>,
    },
    Retry,
    Compensate,
    Failed,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationOutcome<P: WorkflowProfile>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
{
    pub outcome: AuthorityOutcome,
    pub decision: Option<ReconciliationDecision>,
    pub manual_resolution: Option<ManualResolutionRecord<P>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedAcceptanceRecord<E> {
    pub id: OwedAcceptanceId,
    pub reducer_inbox_id: ReducerInboxId,
    pub source_kind: &'static str,
    pub event_codec: CodecRef,
    pub event: E,
    pub disposition: OwedAcceptanceDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainCategoryEvidence {
    pub count: usize,
    pub identities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainProof {
    pub profile: ProfileRef,
    pub protocol: ProtocolSelection,
    pub selector: &'static str,
    pub query_identity: &'static str,
    pub query_version: u32,
    pub authority: Option<SemanticAuthority>,
    pub complete: bool,
    pub categories: BTreeMap<&'static str, DrainCategoryEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowComparisonEvidence {
    pub profile_detail_kind: String,
    pub expected_codec: Option<CodecRef>,
    pub expected_payload: Option<String>,
    pub actual_codec: Option<CodecRef>,
    pub actual_payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowDivergenceRecord {
    pub id: ShadowDivergenceId,
    pub shadow_workflow_id: WorkflowId,
    pub authoritative_workflow_id: WorkflowId,
    pub kind: ShadowDivergenceKind,
    pub severity: DivergenceSeverity,
    pub action: DivergenceAction,
    pub resolution: ShadowDivergenceResolution,
    pub evidence_identity: String,
    pub profile_detail_kind: String,
    pub expected_codec: Option<CodecRef>,
    pub expected_payload: Option<String>,
    pub actual_codec: Option<CodecRef>,
    pub actual_payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowDivergenceResolution {
    Unresolved,
    Resolved {
        action: DivergenceResolutionAction,
        resolved_by: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectState<P: WorkflowProfile> {
    pub declaration: EffectDecl<P::Intent>,
    pub declared_workflow_version: Version,
    pub status: EffectStatus,
    pub dependencies: BTreeSet<EffectId>,
    pub claim: Option<ClaimAuthority>,
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

#[derive(Debug, PartialEq, Eq)]
pub struct WorkflowState<P: WorkflowProfile> {
    pub binding: WorkflowBinding,
    pub semantic_authority: Option<SemanticAuthority>,
    pub version: Version,
    pub generation: Generation,
    pub status: WorkflowStatus,
    pub snapshot: P::Snapshot,
    pub snapshot_codec: CodecRef,
    pub effects: BTreeMap<EffectId, EffectState<P>>,
    pub barriers: BTreeMap<BarrierId, BarrierState<P>>,
    pub reducer_inbox: BTreeMap<ReducerInboxId, ReducerInboxEvent<P>>,
    pub owed_acceptances: BTreeMap<OwedAcceptanceId, OwedAcceptanceRecord<P::OwedAcceptanceEvent>>,
    pub manual_resolutions: BTreeMap<ManualResolutionId, ManualResolutionRecord<P>>,
    pub transition_log: Vec<WorkflowTransition<P::Event>>,
    pub shadow_divergences: Vec<ShadowDivergenceRecord>,
    pub crashed_workers: BTreeSet<&'static str>,
    pub(crate) next_transition_id: u64,
    pub(crate) next_attempt_id: u64,
    pub(crate) next_observation_id: u64,
    pub(crate) next_receipt_id: u64,
    pub(crate) next_inbox_id: u64,
    pub(crate) next_owed_acceptance_id: u64,
    pub(crate) next_manual_resolution_id: u64,
    pub(crate) next_shadow_divergence_id: u64,
    pub(crate) next_claim_token: u64,
}

impl<P: WorkflowProfile> Clone for WorkflowState<P> {
    fn clone(&self) -> Self {
        Self {
            binding: self.binding.clone(),
            semantic_authority: self.semantic_authority,
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
            reducer_inbox: self
                .reducer_inbox
                .iter()
                .map(|(inbox_id, event)| (*inbox_id, clone_reducer_inbox_event(event)))
                .collect(),
            owed_acceptances: self.owed_acceptances.clone(),
            manual_resolutions: self
                .manual_resolutions
                .iter()
                .map(|(resolution_id, resolution)| {
                    (*resolution_id, clone_manual_resolution(resolution))
                })
                .collect(),
            transition_log: self.transition_log.clone(),
            shadow_divergences: self.shadow_divergences.clone(),
            crashed_workers: self.crashed_workers.clone(),
            next_transition_id: self.next_transition_id,
            next_attempt_id: self.next_attempt_id,
            next_observation_id: self.next_observation_id,
            next_receipt_id: self.next_receipt_id,
            next_inbox_id: self.next_inbox_id,
            next_owed_acceptance_id: self.next_owed_acceptance_id,
            next_manual_resolution_id: self.next_manual_resolution_id,
            next_shadow_divergence_id: self.next_shadow_divergence_id,
            next_claim_token: self.next_claim_token,
        }
    }
}

fn clone_reducer_inbox_event<P: WorkflowProfile>(
    event: &ReducerInboxEvent<P>,
) -> ReducerInboxEvent<P> {
    ReducerInboxEvent {
        id: event.id,
        effect_id: event.effect_id,
        barrier_id: event.barrier_id,
        kind: event.kind,
        event_codec: event.event_codec.clone(),
        requires_runtime_acceptance: event.requires_runtime_acceptance,
        payload: match &event.payload {
            ReducerInboxPayload::Receipt(payload) => ReducerInboxPayload::Receipt(payload.clone()),
            ReducerInboxPayload::Barrier(payload) => ReducerInboxPayload::Barrier(payload.clone()),
        },
        delivery_status: event.delivery_status,
        consumed_by: event.consumed_by,
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
        claim: effect.claim.clone(),
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
