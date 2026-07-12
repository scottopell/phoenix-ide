use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

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
pub struct LeaseExpiry(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticAuthority {
    LegacyProtocol,
    EngineProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Authoritative,
    Shadow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoritativeWorkflow {
    pub workflow_id: WorkflowId,
    pub version: Version,
    pub generation: Generation,
    pub profile: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowWorkflow {
    pub workflow_id: WorkflowId,
    pub authoritative_workflow_id: WorkflowId,
    pub profile: &'static str,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReducerInboxKind {
    ReceiptAccepted,
    BarrierSatisfied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguityPolicy {
    ObservableReconciliation,
    ExternalIdempotency,
    SafeRepeatability,
    ManualResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectAmbiguity {
    ObservableReconciliation,
    ExternalIdempotency,
    SafeRepeatability,
    ManualResolution,
}

impl EffectAmbiguity {
    #[must_use]
    pub fn policy(self) -> AmbiguityPolicy {
        match self {
            Self::ObservableReconciliation => AmbiguityPolicy::ObservableReconciliation,
            Self::ExternalIdempotency => AmbiguityPolicy::ExternalIdempotency,
            Self::SafeRepeatability => AmbiguityPolicy::SafeRepeatability,
            Self::ManualResolution => AmbiguityPolicy::ManualResolution,
        }
    }
}

pub trait WorkflowProfile {
    type Snapshot: Clone + Eq + std::fmt::Debug;
    type Event: Clone + Eq + std::fmt::Debug;
    type Intent: Clone + Eq + std::fmt::Debug;
    type Observation: Clone + Eq + std::fmt::Debug;
    type Receipt: Clone + Eq + std::fmt::Debug;
    type BarrierEvent: Clone + Eq + std::fmt::Debug;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDecl<I> {
    pub effect_id: EffectId,
    pub family: &'static str,
    pub kind: &'static str,
    pub generation: Generation,
    pub role: EffectRole,
    pub ambiguity: EffectAmbiguity,
    pub intent: I,
    pub next_eligible_at: Option<LeaseExpiry>,
    pub destructive_resource: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DependencyDecl {
    pub effect_id: EffectId,
    pub depends_on_effect_id: EffectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarrierDecl {
    pub barrier_id: BarrierId,
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
pub struct TransitionPlan<P: WorkflowProfile> {
    pub snapshot: P::Snapshot,
    pub event: P::Event,
    pub effects: Vec<EffectDecl<P::Intent>>,
    pub dependencies: Vec<DependencyDecl>,
    pub barriers: Vec<BarrierDecl>,
    pub barrier_members: Vec<BarrierMemberDecl>,
    pub invalidations: Vec<EffectInvalidationDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerDecision<P: WorkflowProfile> {
    pub expected_workflow_version: Version,
    pub plan: TransitionPlan<P>,
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
    pub observation: O,
    pub authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRecord<R> {
    pub id: ReceiptId,
    pub authority: ClaimAuthority,
    pub attempt_id: Option<AttemptId>,
    pub origin: ReceiptOrigin,
    pub receipt: R,
    pub generation: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerInboxEvent<E> {
    pub effect_id: Option<EffectId>,
    pub barrier_id: Option<BarrierId>,
    pub kind: ReducerInboxKind,
    pub event: E,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTransition<E> {
    pub transition_id: TransitionId,
    pub from_version: Version,
    pub to_version: Version,
    pub generation: Generation,
    pub event: E,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResult<P: WorkflowProfile> {
    pub outcome: CommitOutcome,
    pub transition: Option<WorkflowTransition<P::Event>>,
    pub reducer_events: Vec<ReducerInboxEvent<P::BarrierEvent>>,
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
    pub reducer_event: Option<ReducerInboxEvent<P::BarrierEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierEvaluation<E> {
    pub newly_satisfied: Vec<BarrierId>,
    pub reducer_events: Vec<ReducerInboxEvent<E>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    DuplicateEffectId(EffectId),
    DuplicateBarrierId(BarrierId),
    UnknownEffectReference(EffectId),
    UnknownBarrierReference(BarrierId),
    DependencyCycle,
    BarrierHasNoMembers(BarrierId),
    BarrierIncludesNonRequiredEffect {
        barrier_id: BarrierId,
        effect_id: EffectId,
    },
    BarrierReceiptFamilyMismatch {
        barrier_id: BarrierId,
        effect_id: EffectId,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EngineError {
    #[error("plan validation failed: {0:?}")]
    InvalidPlan(PlanError),
    #[error("workflow binding is shadow-only and cannot execute")]
    ShadowCannotExecute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReducerEvent<P: WorkflowProfile> {
    BarrierSatisfied(P::BarrierEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent<P: WorkflowProfile> {
    ReceiptAccepted(P::Receipt),
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
    pub receipt: Option<ReceiptRecord<P::Receipt>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierState<P: WorkflowProfile> {
    pub barrier_id: BarrierId,
    pub status: BarrierStatus,
    pub required_members: BTreeMap<EffectId, ReceiptFamily>,
    pub reducer_event: P::BarrierEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowState<P: WorkflowProfile> {
    pub binding: WorkflowBinding,
    pub semantic_authority: SemanticAuthority,
    pub version: Version,
    pub generation: Generation,
    pub status: WorkflowStatus,
    pub snapshot: P::Snapshot,
    pub effects: BTreeMap<EffectId, EffectState<P>>,
    pub barriers: BTreeMap<BarrierId, BarrierState<P>>,
    pub transition_log: Vec<WorkflowTransition<P::Event>>,
    next_transition_id: u64,
    next_attempt_id: u64,
    next_observation_id: u64,
    next_receipt_id: u64,
    next_claim_token: u64,
}

impl<P: WorkflowProfile> WorkflowState<P> {
    #[must_use]
    pub fn new_authoritative(
        workflow_id: WorkflowId,
        profile: &'static str,
        snapshot: P::Snapshot,
    ) -> Self {
        Self {
            binding: WorkflowBinding::Authoritative(AuthoritativeWorkflow {
                workflow_id,
                version: Version(0),
                generation: Generation(0),
                profile,
            }),
            semantic_authority: SemanticAuthority::EngineProtocol,
            version: Version(0),
            generation: Generation(0),
            status: WorkflowStatus::Active,
            snapshot,
            effects: BTreeMap::new(),
            barriers: BTreeMap::new(),
            transition_log: Vec::new(),
            next_transition_id: 1,
            next_attempt_id: 1,
            next_observation_id: 1,
            next_receipt_id: 1,
            next_claim_token: 1,
        }
    }

    #[must_use]
    pub fn new_shadow(
        workflow_id: WorkflowId,
        authoritative_workflow_id: WorkflowId,
        profile: &'static str,
        snapshot: P::Snapshot,
    ) -> Self {
        Self {
            binding: WorkflowBinding::Shadow(ShadowWorkflow {
                workflow_id,
                authoritative_workflow_id,
                profile,
            }),
            semantic_authority: SemanticAuthority::EngineProtocol,
            version: Version(0),
            generation: Generation(0),
            status: WorkflowStatus::Active,
            snapshot,
            effects: BTreeMap::new(),
            barriers: BTreeMap::new(),
            transition_log: Vec::new(),
            next_transition_id: 1,
            next_attempt_id: 1,
            next_observation_id: 1,
            next_receipt_id: 1,
            next_claim_token: 1,
        }
    }

    /// Apply one reducer-authored transition under workflow-version CAS.
    ///
    /// # Errors
    /// Returns [`EngineError::ShadowCannotExecute`] for shadow workflows and
    /// [`EngineError::InvalidPlan`] when the declared transition plan fails structural validation.
    ///
    /// # Panics
    /// Panics if `barrier_events` omits an entry for a barrier that previously passed validation.
    pub fn commit_transition(
        &mut self,
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<CommitResult<P>, EngineError> {
        if self.binding.execution_mode() == ExecutionMode::Shadow {
            return Err(EngineError::ShadowCannotExecute);
        }
        if let Err(error) = validate_plan(&decision.plan, barrier_events) {
            return Err(EngineError::InvalidPlan(error));
        }
        if decision.expected_workflow_version != self.version {
            return Ok(CommitResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                reducer_events: Vec::new(),
            });
        }

        let next_version = self.version.next();
        let transition = WorkflowTransition {
            transition_id: TransitionId(self.next_transition_id),
            from_version: self.version,
            to_version: next_version,
            generation: self.generation,
            event: decision.plan.event.clone(),
        };
        self.next_transition_id += 1;
        self.version = next_version;
        self.snapshot = decision.plan.snapshot.clone();
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.version = self.version;
            workflow.generation = self.generation;
        }
        self.transition_log.push(transition.clone());

        for invalidation in &decision.plan.invalidations {
            if let Some(effect) = self.effects.get_mut(&invalidation.effect_id) {
                effect.status = EffectStatus::Invalidated;
                effect.claim = None;
            }
        }

        let mut dependency_map: BTreeMap<EffectId, BTreeSet<EffectId>> = BTreeMap::new();
        for dependency in &decision.plan.dependencies {
            dependency_map
                .entry(dependency.effect_id)
                .or_default()
                .insert(dependency.depends_on_effect_id);
        }

        for effect in &decision.plan.effects {
            let dependencies = dependency_map.remove(&effect.effect_id).unwrap_or_default();
            let status = initial_effect_status(effect, &dependencies);
            self.effects.insert(
                effect.effect_id,
                EffectState {
                    declaration: effect.clone(),
                    declared_workflow_version: self.version,
                    status,
                    dependencies,
                    claim: None,
                    attempts: Vec::new(),
                    observations: Vec::new(),
                    receipt: None,
                },
            );
        }

        let mut member_map: BTreeMap<BarrierId, BTreeMap<EffectId, ReceiptFamily>> =
            BTreeMap::new();
        for member in &decision.plan.barrier_members {
            member_map
                .entry(member.barrier_id)
                .or_default()
                .insert(member.effect_id, member.receipt_family);
        }

        for barrier in &decision.plan.barriers {
            let members = member_map.remove(&barrier.barrier_id).unwrap_or_default();
            let reducer_event = barrier_events
                .get(&barrier.barrier_id)
                .cloned()
                .expect("validated barrier event missing");
            self.barriers.insert(
                barrier.barrier_id,
                BarrierState {
                    barrier_id: barrier.barrier_id,
                    status: BarrierStatus::Waiting,
                    required_members: members,
                    reducer_event,
                },
            );
        }

        self.refresh_eligibility();

        Ok(CommitResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            reducer_events: Vec::new(),
        })
    }

    /// Claim one eligible effect and append its immutable attempt record.
    ///
    /// # Panics
    /// Panics if one effect accumulates more than `u32::MAX` attempts.
    pub fn claim_effect(
        &mut self,
        effect_id: EffectId,
        worker_id: &'static str,
        lease_until: LeaseExpiry,
    ) -> ClaimResult {
        if self.binding.execution_mode() == ExecutionMode::Shadow {
            return ClaimResult {
                outcome: ClaimOutcome::AuthorityConflict,
                authority: None,
                attempt: None,
            };
        }
        let Some(effect) = self.effects.get_mut(&effect_id) else {
            return ClaimResult {
                outcome: ClaimOutcome::Ineligible,
                authority: None,
                attempt: None,
            };
        };
        if effect.status != EffectStatus::Eligible {
            return ClaimResult {
                outcome: ClaimOutcome::Ineligible,
                authority: None,
                attempt: None,
            };
        }
        let authority = ClaimAuthority {
            workflow_id: self.binding.workflow_id(),
            declared_workflow_version: effect.declared_workflow_version,
            generation: self.generation,
            effect_id,
            claim_token: self.next_claim_token,
            worker_id,
            lease_until,
        };
        self.next_claim_token += 1;
        effect.status = EffectStatus::Claimed;
        effect.claim = Some(authority.clone());
        let ordinal = u32::try_from(effect.attempts.len()).expect("attempt count exceeds u32") + 1;
        let attempt = AttemptRecord {
            id: AttemptId(self.next_attempt_id),
            ordinal,
            authority: authority.clone(),
            status: AttemptStatus::Begun,
        };
        self.next_attempt_id += 1;
        effect.attempts.push(attempt.clone());
        ClaimResult {
            outcome: ClaimOutcome::Claimed,
            authority: Some(authority),
            attempt: Some(attempt),
        }
    }

    pub fn renew_claim(
        &mut self,
        authority: &ClaimAuthority,
        new_lease_until: LeaseExpiry,
    ) -> AuthorityOutcome {
        match self.effect_authorized_mut(authority) {
            Some(effect) => {
                if let Some(claim) = &mut effect.claim {
                    claim.lease_until = new_lease_until;
                }
                AuthorityOutcome::Authorized
            }
            None => AuthorityOutcome::StaleAuthority,
        }
    }

    pub fn record_observation(
        &mut self,
        authority: &ClaimAuthority,
        attempt_id: AttemptId,
        observation: P::Observation,
        authoritative: bool,
    ) -> AuthorityOutcome {
        let observation_id = ObservationId(self.next_observation_id);
        match self.effect_authorized_mut(authority) {
            Some(effect) => {
                effect.observations.push(ObservationRecord {
                    id: observation_id,
                    authority: authority.clone(),
                    attempt_id,
                    observation,
                    authoritative,
                });
                if let Some(attempt) = effect
                    .attempts
                    .iter_mut()
                    .find(|attempt| attempt.id == attempt_id)
                {
                    attempt.status = AttemptStatus::ObservationRecorded;
                }
                self.next_observation_id += 1;
                AuthorityOutcome::Authorized
            }
            None => AuthorityOutcome::StaleAuthority,
        }
    }

    pub fn accept_receipt(
        &mut self,
        authority: &ClaimAuthority,
        attempt_id: Option<AttemptId>,
        origin: ReceiptOrigin,
        receipt: P::Receipt,
    ) -> ReceiptAcceptance<P> {
        let receipt_id = ReceiptId(self.next_receipt_id);
        let generation = self.generation;
        let Some(effect) = self.effect_authorized_mut(authority) else {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                reducer_event: None,
            };
        };
        if effect.receipt.is_some() {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                reducer_event: None,
            };
        }
        let receipt_record = ReceiptRecord {
            id: receipt_id,
            authority: authority.clone(),
            attempt_id,
            origin,
            receipt,
            generation,
        };
        effect.receipt = Some(receipt_record.clone());
        effect.claim = None;
        effect.status = EffectStatus::Receipted;
        if let Some(attempt_id) = attempt_id {
            if let Some(attempt) = effect
                .attempts
                .iter_mut()
                .find(|attempt| attempt.id == attempt_id)
            {
                attempt.status = AttemptStatus::ReceiptAccepted;
            }
        }
        self.next_receipt_id += 1;
        self.refresh_eligibility();
        let evaluation = self.evaluate_barriers();
        ReceiptAcceptance {
            outcome: AuthorityOutcome::Authorized,
            receipt: Some(receipt_record),
            reducer_event: evaluation.reducer_events.into_iter().next(),
        }
    }

    #[must_use]
    pub fn evaluate_barriers(&mut self) -> BarrierEvaluation<P::BarrierEvent> {
        let mut newly_satisfied = Vec::new();
        let mut reducer_events = Vec::new();
        for barrier in self.barriers.values_mut() {
            if barrier.status == BarrierStatus::Satisfied {
                continue;
            }
            let satisfied = barrier.required_members.iter().all(|(effect_id, family)| {
                self.effects.get(effect_id).is_some_and(|effect| {
                    effect.receipt.as_ref().is_some_and(|receipt| {
                        receipt.generation == self.generation
                            && declared_receipt_family(&effect.declaration) == *family
                    })
                })
            });
            if satisfied {
                barrier.status = BarrierStatus::Satisfied;
                newly_satisfied.push(barrier.barrier_id);
                reducer_events.push(ReducerInboxEvent {
                    effect_id: None,
                    barrier_id: Some(barrier.barrier_id),
                    kind: ReducerInboxKind::BarrierSatisfied,
                    event: barrier.reducer_event.clone(),
                });
            }
        }
        BarrierEvaluation {
            newly_satisfied,
            reducer_events,
        }
    }

    /// Advance generation, revoke old authority, invalidate prior work, and append a compensation DAG.
    ///
    /// # Errors
    /// Returns [`EngineError::ShadowCannotExecute`] for shadow workflows and
    /// [`EngineError::InvalidPlan`] when the compensation plan fails structural validation.
    pub fn cancel_with_compensation(
        &mut self,
        next_snapshot: P::Snapshot,
        event: P::Event,
        invalidations: Vec<EffectInvalidationDecl>,
        compensation_plan: TransitionPlan<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<CommitResult<P>, EngineError> {
        if self.binding.execution_mode() == ExecutionMode::Shadow {
            return Err(EngineError::ShadowCannotExecute);
        }
        validate_plan(&compensation_plan, barrier_events).map_err(EngineError::InvalidPlan)?;
        self.generation = self.generation.next();
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.generation = self.generation;
        }
        self.status = WorkflowStatus::Cancelling;
        for effect in self.effects.values_mut() {
            if effect.declaration.generation != self.generation {
                effect.claim = None;
                if !matches!(
                    effect.status,
                    EffectStatus::Receipted | EffectStatus::Invalidated
                ) {
                    effect.status = EffectStatus::Invalidated;
                }
            }
        }
        let decision = ReducerDecision {
            expected_workflow_version: self.version,
            plan: TransitionPlan {
                snapshot: next_snapshot,
                event,
                effects: compensation_plan.effects,
                dependencies: compensation_plan.dependencies,
                barriers: compensation_plan.barriers,
                barrier_members: compensation_plan.barrier_members,
                invalidations: invalidations
                    .into_iter()
                    .chain(compensation_plan.invalidations)
                    .collect(),
            },
        };
        self.commit_transition(&decision, barrier_events)
    }

    fn effect_authorized_mut(&mut self, authority: &ClaimAuthority) -> Option<&mut EffectState<P>> {
        if self.binding.execution_mode() == ExecutionMode::Shadow
            || authority.workflow_id != self.binding.workflow_id()
        {
            return None;
        }
        let effect = self.effects.get_mut(&authority.effect_id)?;
        let claim = effect.claim.as_ref()?;
        if claim.workflow_id == authority.workflow_id
            && claim.declared_workflow_version == authority.declared_workflow_version
            && claim.generation == authority.generation
            && claim.effect_id == authority.effect_id
            && claim.claim_token == authority.claim_token
            && claim.worker_id == authority.worker_id
            && authority.generation == self.generation
        {
            Some(effect)
        } else {
            None
        }
    }

    fn refresh_eligibility(&mut self) {
        let ready_ids: Vec<EffectId> = self
            .effects
            .iter()
            .filter_map(|(effect_id, effect)| {
                ((effect.status == EffectStatus::Blocked
                    || effect.status == EffectStatus::RetryWait)
                    && effect.declaration.generation == self.generation
                    && effect.dependencies.iter().all(|dependency_id| {
                        self.effects
                            .get(dependency_id)
                            .is_some_and(|dependency| dependency.status == EffectStatus::Receipted)
                    }))
                .then_some(*effect_id)
            })
            .collect();
        for effect_id in ready_ids {
            if let Some(effect) = self.effects.get_mut(&effect_id) {
                effect.status = EffectStatus::Eligible;
            }
        }
    }
}

fn initial_effect_status<I>(_: &EffectDecl<I>, dependencies: &BTreeSet<EffectId>) -> EffectStatus {
    if dependencies.is_empty() {
        EffectStatus::Eligible
    } else {
        EffectStatus::Blocked
    }
}

#[must_use]
pub fn declared_receipt_family<I>(effect: &EffectDecl<I>) -> ReceiptFamily {
    match effect.role {
        EffectRole::Compensation => ReceiptFamily::CompensationEffect,
        EffectRole::Required | EffectRole::Optional => ReceiptFamily::CurrentGenerationEffect,
    }
}

/// Validate the complete declared transition DAG before persistence or execution.
///
/// # Errors
/// Returns the first structural violation among duplicate identifiers, unknown references,
/// cyclic dependencies, and invalid barrier membership.
///
/// # Panics
/// Panics only if an internal consistency check fails after a prior reference-validation step.
#[allow(clippy::too_many_lines)]
pub fn validate_plan<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
) -> Result<(), PlanError> {
    let mut effect_ids = BTreeSet::new();
    for effect in &plan.effects {
        if !effect_ids.insert(effect.effect_id) {
            return Err(PlanError::DuplicateEffectId(effect.effect_id));
        }
    }

    let mut barrier_ids = BTreeSet::new();
    for barrier in &plan.barriers {
        if !barrier_ids.insert(barrier.barrier_id) {
            return Err(PlanError::DuplicateBarrierId(barrier.barrier_id));
        }
        if !barrier_events.contains_key(&barrier.barrier_id) {
            return Err(PlanError::UnknownBarrierReference(barrier.barrier_id));
        }
    }

    for dependency in &plan.dependencies {
        if !effect_ids.contains(&dependency.effect_id) {
            return Err(PlanError::UnknownEffectReference(dependency.effect_id));
        }
        if !effect_ids.contains(&dependency.depends_on_effect_id) {
            return Err(PlanError::UnknownEffectReference(
                dependency.depends_on_effect_id,
            ));
        }
    }

    let mut members_by_barrier: BTreeMap<BarrierId, Vec<BarrierMemberDecl>> = BTreeMap::new();
    for member in &plan.barrier_members {
        if !barrier_ids.contains(&member.barrier_id) {
            return Err(PlanError::UnknownBarrierReference(member.barrier_id));
        }
        if !effect_ids.contains(&member.effect_id) {
            return Err(PlanError::UnknownEffectReference(member.effect_id));
        }
        members_by_barrier
            .entry(member.barrier_id)
            .or_default()
            .push(*member);
    }

    let effect_by_id: BTreeMap<EffectId, &EffectDecl<P::Intent>> = plan
        .effects
        .iter()
        .map(|effect| (effect.effect_id, effect))
        .collect();

    for barrier in &plan.barriers {
        let Some(members) = members_by_barrier.get(&barrier.barrier_id) else {
            return Err(PlanError::BarrierHasNoMembers(barrier.barrier_id));
        };
        let mut compensation_only = true;
        for member in members {
            let effect = effect_by_id[&member.effect_id];
            compensation_only &= effect.role == EffectRole::Compensation;
            let expected = declared_receipt_family(effect);
            if member.receipt_family != expected {
                return Err(PlanError::BarrierReceiptFamilyMismatch {
                    barrier_id: barrier.barrier_id,
                    effect_id: member.effect_id,
                });
            }
        }
        if !compensation_only {
            for member in members {
                let effect = effect_by_id[&member.effect_id];
                if effect.role != EffectRole::Required {
                    return Err(PlanError::BarrierIncludesNonRequiredEffect {
                        barrier_id: barrier.barrier_id,
                        effect_id: member.effect_id,
                    });
                }
            }
        }
    }

    let mut indegree: BTreeMap<EffectId, usize> = plan
        .effects
        .iter()
        .map(|effect| (effect.effect_id, 0))
        .collect();
    let mut outgoing: BTreeMap<EffectId, Vec<EffectId>> = BTreeMap::new();
    for dependency in &plan.dependencies {
        *indegree
            .get_mut(&dependency.effect_id)
            .expect("validated effect exists") += 1;
        outgoing
            .entry(dependency.depends_on_effect_id)
            .or_default()
            .push(dependency.effect_id);
    }
    let mut queue: VecDeque<EffectId> = indegree
        .iter()
        .filter_map(|(effect_id, count)| (*count == 0).then_some(*effect_id))
        .collect();
    let mut visited = 0usize;
    while let Some(effect_id) = queue.pop_front() {
        visited += 1;
        for child in outgoing.get(&effect_id).into_iter().flatten() {
            let count = indegree.get_mut(child).expect("validated child exists");
            *count -= 1;
            if *count == 0 {
                queue.push_back(*child);
            }
        }
    }
    if visited != indegree.len() {
        return Err(PlanError::DependencyCycle);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestProfile;

    impl WorkflowProfile for TestProfile {
        type Snapshot = &'static str;
        type Event = &'static str;
        type Intent = &'static str;
        type Observation = &'static str;
        type Receipt = &'static str;
        type BarrierEvent = &'static str;
    }

    fn effect(
        effect_id: u64,
        role: EffectRole,
        generation: Generation,
    ) -> EffectDecl<&'static str> {
        EffectDecl {
            effect_id: EffectId(effect_id),
            family: "test",
            kind: "step",
            generation,
            role,
            ambiguity: EffectAmbiguity::SafeRepeatability,
            intent: "intent",
            next_eligible_at: None,
            destructive_resource: None,
        }
    }

    fn plan() -> TransitionPlan<TestProfile> {
        TransitionPlan {
            snapshot: "next",
            event: "evt",
            effects: vec![
                effect(1, EffectRole::Required, Generation(0)),
                effect(2, EffectRole::Required, Generation(0)),
            ],
            dependencies: vec![DependencyDecl {
                effect_id: EffectId(2),
                depends_on_effect_id: EffectId(1),
            }],
            barriers: vec![BarrierDecl {
                barrier_id: BarrierId(10),
            }],
            barrier_members: vec![
                BarrierMemberDecl {
                    barrier_id: BarrierId(10),
                    effect_id: EffectId(1),
                    receipt_family: ReceiptFamily::CurrentGenerationEffect,
                },
                BarrierMemberDecl {
                    barrier_id: BarrierId(10),
                    effect_id: EffectId(2),
                    receipt_family: ReceiptFamily::CurrentGenerationEffect,
                },
            ],
            invalidations: vec![],
        }
    }

    fn barrier_events() -> BTreeMap<BarrierId, &'static str> {
        BTreeMap::from([(BarrierId(10), "barrier-event")])
    }

    #[test]
    fn validates_happy_path_plan() {
        assert_eq!(validate_plan(&plan(), &barrier_events()), Ok(()));
    }

    #[test]
    fn rejects_duplicate_effect_ids() {
        let mut plan = plan();
        plan.effects
            .push(effect(1, EffectRole::Required, Generation(0)));
        assert_eq!(
            validate_plan(&plan, &barrier_events()),
            Err(PlanError::DuplicateEffectId(EffectId(1)))
        );
    }

    #[test]
    fn rejects_unknown_dependency_reference() {
        let mut plan = plan();
        plan.dependencies.push(DependencyDecl {
            effect_id: EffectId(2),
            depends_on_effect_id: EffectId(99),
        });
        assert_eq!(
            validate_plan(&plan, &barrier_events()),
            Err(PlanError::UnknownEffectReference(EffectId(99)))
        );
    }

    #[test]
    fn rejects_dependency_cycles() {
        let mut plan = plan();
        plan.dependencies.push(DependencyDecl {
            effect_id: EffectId(1),
            depends_on_effect_id: EffectId(2),
        });
        assert_eq!(
            validate_plan(&plan, &barrier_events()),
            Err(PlanError::DependencyCycle)
        );
    }

    #[test]
    fn rejects_optional_barrier_members() {
        let mut plan = plan();
        plan.effects[1].role = EffectRole::Optional;
        assert_eq!(
            validate_plan(&plan, &barrier_events()),
            Err(PlanError::BarrierIncludesNonRequiredEffect {
                barrier_id: BarrierId(10),
                effect_id: EffectId(2)
            })
        );
    }

    #[test]
    fn cas_commit_emits_transition_but_not_product_completion() {
        let mut workflow =
            WorkflowState::<TestProfile>::new_authoritative(WorkflowId(1), "test", "initial");
        let result = workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("commit succeeds");
        assert_eq!(result.outcome, CommitOutcome::Committed);
        assert_eq!(
            result.reducer_events,
            Vec::<ReducerInboxEvent<&'static str>>::new()
        );
        let transition = result.transition.expect("transition emitted");
        assert_eq!(transition.from_version, Version(0));
        assert_eq!(transition.to_version, Version(1));
    }

    #[test]
    fn stale_cas_does_not_mutate_state() {
        let mut workflow =
            WorkflowState::<TestProfile>::new_authoritative(WorkflowId(1), "test", "initial");
        let result = workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(7),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("cas handled");
        assert_eq!(result.outcome, CommitOutcome::VersionConflict);
        assert!(workflow.effects.is_empty());
        assert!(workflow.transition_log.is_empty());
    }

    #[test]
    fn shadow_workflow_cannot_execute() {
        let mut workflow = WorkflowState::<TestProfile>::new_shadow(
            WorkflowId(2),
            WorkflowId(1),
            "test",
            "initial",
        );
        assert_eq!(
            workflow.commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan()
                },
                &barrier_events()
            ),
            Err(EngineError::ShadowCannotExecute)
        );
    }

    #[test]
    fn claim_renew_observe_and_receipt_require_live_authority() {
        let mut workflow =
            WorkflowState::<TestProfile>::new_authoritative(WorkflowId(1), "test", "initial");
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("commit succeeds");
        let claim = workflow.claim_effect(EffectId(1), "worker-a", LeaseExpiry(10));
        let authority = claim.authority.expect("authority issued");
        let attempt = claim.attempt.expect("attempt created");
        assert_eq!(
            workflow.renew_claim(&authority, LeaseExpiry(20)),
            AuthorityOutcome::Authorized
        );
        assert_eq!(
            workflow.record_observation(&authority, attempt.id, "saw", true),
            AuthorityOutcome::Authorized
        );
        let accepted = workflow.accept_receipt(
            &authority,
            Some(attempt.id),
            ReceiptOrigin::Execution,
            "done",
        );
        assert_eq!(accepted.outcome, AuthorityOutcome::Authorized);
        let stale = workflow.accept_receipt(
            &authority,
            Some(attempt.id),
            ReceiptOrigin::Execution,
            "duplicate",
        );
        assert_eq!(stale.outcome, AuthorityOutcome::StaleAuthority);
    }

    #[test]
    fn barrier_satisfaction_emits_reducer_inbox_event() {
        let mut workflow =
            WorkflowState::<TestProfile>::new_authoritative(WorkflowId(1), "test", "initial");
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("commit succeeds");
        let first = workflow.claim_effect(EffectId(1), "worker-a", LeaseExpiry(10));
        let first_authority = first.authority.expect("authority issued");
        let first_attempt = first.attempt.expect("attempt created");
        let accepted = workflow.accept_receipt(
            &first_authority,
            Some(first_attempt.id),
            ReceiptOrigin::Execution,
            "done-1",
        );
        assert!(accepted.reducer_event.is_none());
        let second = workflow.claim_effect(EffectId(2), "worker-b", LeaseExpiry(10));
        let second_authority = second.authority.expect("authority issued");
        let second_attempt = second.attempt.expect("attempt created");
        let accepted = workflow.accept_receipt(
            &second_authority,
            Some(second_attempt.id),
            ReceiptOrigin::Execution,
            "done-2",
        );
        assert_eq!(
            accepted.reducer_event,
            Some(ReducerInboxEvent {
                effect_id: None,
                barrier_id: Some(BarrierId(10)),
                kind: ReducerInboxKind::BarrierSatisfied,
                event: "barrier-event",
            })
        );
    }

    #[test]
    fn cancellation_bumps_generation_and_revokes_prior_claims() {
        let mut workflow =
            WorkflowState::<TestProfile>::new_authoritative(WorkflowId(1), "test", "initial");
        workflow
            .commit_transition(
                &ReducerDecision {
                    expected_workflow_version: Version(0),
                    plan: plan(),
                },
                &barrier_events(),
            )
            .expect("commit succeeds");
        let claim = workflow.claim_effect(EffectId(1), "worker-a", LeaseExpiry(10));
        let authority = claim.authority.expect("authority issued");
        let compensation_plan = TransitionPlan {
            snapshot: "cancelled",
            event: "cancel",
            effects: vec![effect(20, EffectRole::Compensation, Generation(1))],
            dependencies: vec![],
            barriers: vec![BarrierDecl {
                barrier_id: BarrierId(11),
            }],
            barrier_members: vec![BarrierMemberDecl {
                barrier_id: BarrierId(11),
                effect_id: EffectId(20),
                receipt_family: ReceiptFamily::CompensationEffect,
            }],
            invalidations: vec![],
        };
        let cancel_events = BTreeMap::from([(BarrierId(11), "compensated")]);
        let _ = workflow
            .cancel_with_compensation(
                "cancelled",
                "cancel",
                vec![EffectInvalidationDecl {
                    effect_id: EffectId(1),
                }],
                compensation_plan,
                &cancel_events,
            )
            .expect("cancel succeeds");
        assert_eq!(workflow.generation, Generation(1));
        assert_eq!(
            workflow.renew_claim(&authority, LeaseExpiry(30)),
            AuthorityOutcome::StaleAuthority
        );
        assert_eq!(
            workflow
                .effects
                .get(&EffectId(20))
                .map(|effect| effect.declaration.role),
            Some(EffectRole::Compensation)
        );
    }

    proptest! {
        #[test]
        fn plan_cycle_detection_matches_simple_generator(extra_edges in prop::collection::vec((0u8..4, 0u8..4), 0..8)) {
            let effects = (0u64..4)
                .map(|idx| effect(idx + 1, EffectRole::Required, Generation(0)))
                .collect::<Vec<_>>();
            let dependencies = extra_edges
                .into_iter()
                .filter(|(a, b)| a != b)
                .map(|(a, b)| DependencyDecl {
                    effect_id: EffectId(u64::from(a) + 1),
                    depends_on_effect_id: EffectId(u64::from(b) + 1),
                })
                .collect::<Vec<_>>();
            let plan: TransitionPlan<TestProfile> = TransitionPlan {
                snapshot: "next",
                event: "evt",
                effects,
                dependencies: dependencies.clone(),
                barriers: vec![BarrierDecl { barrier_id: BarrierId(10) }],
                barrier_members: vec![
                    BarrierMemberDecl { barrier_id: BarrierId(10), effect_id: EffectId(1), receipt_family: ReceiptFamily::CurrentGenerationEffect },
                ],
                invalidations: vec![],
            };
            let result = validate_plan(&plan, &BTreeMap::from([(BarrierId(10), "barrier")]));
            let has_two_cycle = dependencies.iter().any(|left| {
                dependencies.iter().any(|right| {
                    left.effect_id == right.depends_on_effect_id && left.depends_on_effect_id == right.effect_id
                })
            });
            prop_assert!(!(has_two_cycle && result.is_ok()));
        }
    }
}
