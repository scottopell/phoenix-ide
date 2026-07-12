use std::collections::{BTreeMap, BTreeSet};

use crate::{
    declared_receipt_family, validate_plan, AcceptanceStatus, AtomicInboxConsumeResult, AttemptId,
    AttemptRecord, AttemptStatus, AuthoritativeWorkflow, AuthorityOutcome, BarrierEvaluation,
    BarrierId, BarrierState, BarrierStatus, CancellationRequest, ClaimAuthority, ClaimOutcome,
    ClaimResult, CodecRef, CommitOutcome, CommitResult, DeliveryStatus, DivergenceAction,
    DivergenceSeverity, EffectDecl, EffectId, EffectInvalidationDecl, EffectState, EffectStatus,
    EngineError, ExecutionMode, Generation, LeaseExpiry, ManualChoice, ManualResolutionId,
    ManualResolutionOutcome, ManualResolutionRecord, ObservationId, ObservationRecord,
    OwedAcceptanceId, OwedAcceptanceRecord, ProfileRef, ProtocolSelection, ReceiptAcceptance,
    ReceiptFamily, ReceiptId, ReceiptOrigin, ReceiptRecord, ReconciliationDecision,
    ReconciliationOutcome, ReducerDecision, ReducerInboxEvent, ReducerInboxId, ReducerInboxKind,
    ReducerInboxPayload, ResolutionStatus, ResourceLockGrant, RuntimeAcceptanceResult,
    SemanticAuthority, ShadowDivergenceId, ShadowDivergenceKind, ShadowDivergenceRecord,
    ShadowWorkflow, StaleObservationRecord, SuppressionReason, Timestamp, TransitionId,
    TransitionPlan, Version, WorkflowBinding, WorkflowId, WorkflowProfile, WorkflowState,
    WorkflowStatus, WorkflowTransition,
};

impl<P: WorkflowProfile> WorkflowState<P> {
    #[must_use]
    pub fn new_authoritative(
        workflow_id: WorkflowId,
        profile: &ProfileRef,
        accepted_protocol: ProtocolSelection,
        snapshot_codec: CodecRef,
        snapshot: P::Snapshot,
    ) -> Self {
        Self {
            binding: WorkflowBinding::Authoritative(AuthoritativeWorkflow {
                workflow_id,
                version: Version(0),
                generation: Generation(0),
                profile: profile.clone(),
                accepted_protocol,
            }),
            semantic_authority: SemanticAuthority::EngineProtocol,
            version: Version(0),
            generation: Generation(0),
            status: WorkflowStatus::Active,
            snapshot,
            snapshot_codec,
            effects: BTreeMap::new(),
            barriers: BTreeMap::new(),
            reducer_inbox: BTreeMap::new(),
            owed_acceptances: BTreeMap::new(),
            manual_resolutions: BTreeMap::new(),
            transition_log: Vec::new(),
            shadow_divergences: Vec::new(),
            crashed_workers: BTreeSet::new(),
            next_transition_id: 1,
            next_attempt_id: 1,
            next_observation_id: 1,
            next_receipt_id: 1,
            next_inbox_id: 1,
            next_owed_acceptance_id: 1,
            next_manual_resolution_id: 1,
            next_shadow_divergence_id: 1,
            next_claim_token: 1,
        }
    }

    #[must_use]
    pub fn new_shadow(
        workflow_id: WorkflowId,
        authoritative_workflow_id: WorkflowId,
        profile: &ProfileRef,
        accepted_protocol: ProtocolSelection,
        snapshot_codec: CodecRef,
        snapshot: P::Snapshot,
    ) -> Self {
        Self {
            binding: WorkflowBinding::Shadow(ShadowWorkflow {
                workflow_id,
                authoritative_workflow_id,
                profile: profile.clone(),
                accepted_protocol,
            }),
            semantic_authority: SemanticAuthority::EngineProtocol,
            version: Version(0),
            generation: Generation(0),
            status: WorkflowStatus::Active,
            snapshot,
            snapshot_codec,
            effects: BTreeMap::new(),
            barriers: BTreeMap::new(),
            reducer_inbox: BTreeMap::new(),
            owed_acceptances: BTreeMap::new(),
            manual_resolutions: BTreeMap::new(),
            transition_log: Vec::new(),
            shadow_divergences: Vec::new(),
            crashed_workers: BTreeSet::new(),
            next_transition_id: 1,
            next_attempt_id: 1,
            next_observation_id: 1,
            next_receipt_id: 1,
            next_inbox_id: 1,
            next_owed_acceptance_id: 1,
            next_manual_resolution_id: 1,
            next_shadow_divergence_id: 1,
            next_claim_token: 1,
        }
    }

    /// Applies a reducer-approved transition only if the expected workflow version still
    /// matches the durable state.
    ///
    /// # Errors
    /// Returns [`EngineError`] when the workflow is shadow-only, the plan is invalid, a validated
    /// barrier event is missing during application, or the attempt counter would overflow `u32`.
    pub fn commit_transition(
        &mut self,
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<CommitResult<P>, EngineError> {
        self.ensure_executable()?;
        validate_plan(&decision.plan, barrier_events).map_err(EngineError::InvalidPlan)?;
        if decision.expected_workflow_version != self.version {
            return Ok(version_conflict_result());
        }

        let mut replacement = self.clone();
        let transition = replacement.begin_transition(decision);
        replacement.apply_invalidations(&decision.plan.invalidations);
        replacement.install_effects(&decision.plan);
        replacement.install_barriers(&decision.plan, barrier_events)?;
        replacement.install_owed_acceptances(&decision.plan);
        replacement.refresh_eligibility(Timestamp(0));
        *self = replacement;

        Ok(CommitResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            reducer_events: Vec::new(),
        })
    }

    /// Claims an eligible effect for execution under a leased authority token.
    ///
    /// # Errors
    /// Returns [`ClaimResult`] with `Ineligible` or `AuthorityConflict` when the effect cannot be
    /// claimed. Returns `Ineligible` if the attempt ordinal would overflow `u32`.
    pub fn claim_effect(
        &mut self,
        effect_id: EffectId,
        worker_id: &'static str,
        now: Timestamp,
        lease_until: LeaseExpiry,
    ) -> ClaimResult {
        if self.binding.execution_mode() == ExecutionMode::Shadow {
            return denied_claim();
        }
        if !lease_until.is_live_at(now) {
            return ineligible_claim();
        }
        if self.crashed_workers.contains(&worker_id) {
            return denied_claim();
        }
        let Some(effect) = self.effects.get(&effect_id) else {
            return ineligible_claim();
        };
        if effect.status != EffectStatus::Eligible
            || effect.declaration.generation != self.generation
        {
            return ineligible_claim();
        }
        let claim_token = self.next_claim_token;
        let Ok(resource_lock) =
            self.lock_grant_for_claim(effect_id, worker_id, claim_token, now, lease_until)
        else {
            return denied_claim();
        };
        let authority = ClaimAuthority {
            workflow_id: self.binding.workflow_id(),
            declared_workflow_version: effect.declared_workflow_version,
            generation: self.generation,
            effect_id,
            claim_token,
            worker_id,
            lease_until,
            resource_lock: resource_lock.clone(),
        };
        self.next_claim_token += 1;
        let Some(effect) = self.effects.get_mut(&effect_id) else {
            return ineligible_claim();
        };
        effect.status = EffectStatus::Claimed;
        effect.claim = Some(authority.clone());
        effect.pending_reconciliation = false;
        effect.destructive_lock = resource_lock;
        let Ok(ordinal_base) = u32::try_from(effect.attempts.len()) else {
            return ineligible_claim();
        };
        let ordinal = ordinal_base.saturating_add(1);
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
        now: Timestamp,
        new_lease_until: LeaseExpiry,
    ) -> AuthorityOutcome {
        if !new_lease_until.is_live_at(now) {
            return AuthorityOutcome::StaleAuthority;
        }
        match self.effect_authorized_mut(authority, now) {
            Some(effect) => {
                if let Some(claim) = &mut effect.claim {
                    claim.lease_until = new_lease_until;
                }
                AuthorityOutcome::Authorized
            }
            None => AuthorityOutcome::StaleAuthority,
        }
    }

    pub fn take_over_expired_claim(
        &mut self,
        effect_id: EffectId,
        expired_claim: &ClaimAuthority,
        now: Timestamp,
    ) -> AuthorityOutcome {
        let Some(effect) = self.effects.get_mut(&effect_id) else {
            return AuthorityOutcome::StaleAuthority;
        };
        let Some(live_claim) = effect.claim.as_ref() else {
            return AuthorityOutcome::StaleAuthority;
        };
        if live_claim != expired_claim || expired_claim.lease_until.is_live_at(now) {
            return AuthorityOutcome::StaleAuthority;
        }
        effect.claim = None;
        effect.destructive_lock = None;
        effect.status = EffectStatus::Eligible;
        effect.pending_reconciliation = true;
        if let Some(last_attempt) = effect.attempts.last_mut() {
            last_attempt.status = AttemptStatus::AuthorityLost;
        }
        AuthorityOutcome::Authorized
    }

    pub fn record_observation(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
        attempt_id: AttemptId,
        observation: P::Observation,
        authoritative: bool,
    ) -> AuthorityOutcome {
        let observation_id = ObservationId(self.next_observation_id);
        self.next_observation_id += 1;
        if let Some(effect) = self.effect_authorized_mut(authority, now) {
            if !effect
                .attempts
                .iter()
                .any(|attempt| attempt.id == attempt_id && attempt.authority == *authority)
            {
                return AuthorityOutcome::StaleAuthority;
            }
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
            return AuthorityOutcome::Authorized;
        }
        if let Some(effect) = self.effects.get_mut(&authority.effect_id) {
            effect.stale_observations.push(StaleObservationRecord {
                id: observation_id,
                authority: authority.clone(),
                attempt_id,
                observation,
            });
        }
        AuthorityOutcome::StaleAuthority
    }

    pub fn accept_receipt(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
        attempt_id: Option<AttemptId>,
        origin: ReceiptOrigin,
        receipt: P::Receipt,
        receipt_event: P::ReceiptReducerEvent,
    ) -> ReceiptAcceptance<P> {
        let receipt_id = ReceiptId(self.next_receipt_id);
        self.next_receipt_id += 1;
        let generation = self.generation;
        let Some(effect) = self.effect_authorized_mut(authority, now) else {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                receipt_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            };
        };
        if effect.receipt.is_some() {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                receipt_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            };
        }
        if let Some(attempt_id) = attempt_id {
            if !effect
                .attempts
                .iter()
                .any(|attempt| attempt.id == attempt_id && attempt.authority == *authority)
            {
                return ReceiptAcceptance {
                    outcome: AuthorityOutcome::StaleAuthority,
                    receipt: None,
                    receipt_inbox_ids: Vec::new(),
                    reducer_events: Vec::new(),
                };
            }
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
        effect.destructive_lock = None;
        effect.status = EffectStatus::Receipted;
        effect.pending_reconciliation = false;
        if let Some(attempt_id) = attempt_id {
            if let Some(attempt) = effect
                .attempts
                .iter_mut()
                .find(|attempt| attempt.id == attempt_id)
            {
                attempt.status = AttemptStatus::ReceiptAccepted;
            }
        }

        let inbox_id = ReducerInboxId(self.next_inbox_id);
        self.next_inbox_id += 1;
        let reducer_event = ReducerInboxEvent {
            id: inbox_id,
            effect_id: Some(authority.effect_id),
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            payload: ReducerInboxPayload::Receipt(receipt_event),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        };
        self.reducer_inbox.insert(inbox_id, reducer_event);
        self.refresh_eligibility(now);
        let barrier_events = self.evaluate_barriers();
        ReceiptAcceptance {
            outcome: AuthorityOutcome::Authorized,
            receipt: Some(receipt_record),
            receipt_inbox_ids: vec![inbox_id],
            reducer_events: barrier_events.reducer_events,
        }
    }

    pub fn schedule_retry(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
        retry_at: Timestamp,
    ) -> ReconciliationOutcome<P> {
        match self.effect_authorized_mut(authority, now) {
            Some(effect) => {
                effect.status = EffectStatus::RetryWait;
                effect.declaration.next_eligible_at = Some(retry_at);
                effect.claim = None;
                effect.destructive_lock = None;
                ReconciliationOutcome {
                    outcome: AuthorityOutcome::Authorized,
                    decision: Some(ReconciliationDecision::RetryInfrastructure),
                    manual_resolution: None,
                }
            }
            None => ReconciliationOutcome {
                outcome: AuthorityOutcome::StaleAuthority,
                decision: None,
                manual_resolution: None,
            },
        }
    }

    pub fn require_manual_resolution(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
        permitted_choices: Vec<ManualChoice<P>>,
    ) -> ReconciliationOutcome<P> {
        let resolution_id = ManualResolutionId(self.next_manual_resolution_id);
        let workflow_version = self.version;
        let Some(effect) = self.effect_authorized_mut(authority, now) else {
            return ReconciliationOutcome {
                outcome: AuthorityOutcome::StaleAuthority,
                decision: None,
                manual_resolution: None,
            };
        };
        effect.status = EffectStatus::AmbiguityWait;
        effect.claim = None;
        effect.destructive_lock = None;
        let resolution = ManualResolutionRecord {
            id: resolution_id,
            workflow_version,
            effect_id: authority.effect_id,
            status: ResolutionStatus::Required,
            evidence: effect.observations.clone(),
            permitted_choices,
            accepted_choice: None,
        };
        self.next_manual_resolution_id += 1;
        self.manual_resolutions.insert(resolution_id, resolution);
        ReconciliationOutcome {
            outcome: AuthorityOutcome::Authorized,
            decision: Some(ReconciliationDecision::RequestManualResolution),
            manual_resolution: self
                .manual_resolutions
                .get(&resolution_id)
                .map(clone_manual_resolution),
        }
    }

    pub fn resolve_manual(
        &mut self,
        resolution_id: ManualResolutionId,
        expected_workflow_version: Version,
        choice: &ManualChoice<P>,
        receipt: P::Receipt,
        receipt_event: P::ReceiptReducerEvent,
    ) -> ManualResolutionOutcome<P> {
        let Some(existing) = self
            .manual_resolutions
            .get(&resolution_id)
            .map(clone_manual_resolution)
        else {
            return invalid_manual_resolution(None);
        };
        if existing.status != ResolutionStatus::Required
            || existing.workflow_version != expected_workflow_version
            || self.version != expected_workflow_version
        {
            return version_conflict_manual_resolution(existing);
        }
        if !manual_choice_permitted(&existing, choice) {
            return invalid_manual_resolution(Some(existing));
        }
        if !self.effect_ready_for_manual_resolution(&existing) {
            return invalid_manual_resolution(Some(existing));
        }

        let mut replacement = self.clone();
        let receipt_record = replacement.apply_manual_resolution(
            resolution_id,
            expected_workflow_version,
            &existing,
            choice,
            receipt,
            receipt_event,
        );
        let barrier_event = replacement
            .evaluate_barriers()
            .reducer_events
            .into_iter()
            .next();
        let resolution = replacement
            .manual_resolutions
            .get(&resolution_id)
            .map(clone_manual_resolution);
        *self = replacement;

        ManualResolutionOutcome {
            outcome: CommitOutcome::Committed,
            resolution,
            receipt: Some(receipt_record.clone()),
            reducer_event: barrier_event.or_else(|| {
                self.reducer_inbox
                    .values()
                    .rev()
                    .find(|event| event.effect_id == Some(existing.effect_id))
                    .map(clone_reducer_inbox_event)
            }),
        }
    }

    /// Commits a reducer decision and consumes the exact inbox items in one state replacement.
    ///
    /// # Errors
    /// Returns an engine error when the inbox set is invalid or the reducer decision cannot commit.
    pub fn consume_reducer_inbox_atomically(
        &mut self,
        inbox_ids: &[ReducerInboxId],
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<AtomicInboxConsumeResult<P>, EngineError> {
        self.ensure_executable()?;
        validate_plan(&decision.plan, barrier_events).map_err(EngineError::InvalidPlan)?;
        if decision.expected_workflow_version != self.version {
            return Ok(AtomicInboxConsumeResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                consumed_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            });
        }
        let mut replacement = self.clone();
        for inbox_id in inbox_ids {
            let Some(inbox) = replacement.reducer_inbox.get(inbox_id) else {
                return Ok(AtomicInboxConsumeResult {
                    outcome: CommitOutcome::InvalidPlan,
                    transition: None,
                    consumed_inbox_ids: Vec::new(),
                    reducer_events: Vec::new(),
                });
            };
            if inbox.delivery_status == DeliveryStatus::Consumed {
                return Ok(AtomicInboxConsumeResult {
                    outcome: CommitOutcome::InvalidPlan,
                    transition: None,
                    consumed_inbox_ids: Vec::new(),
                    reducer_events: Vec::new(),
                });
            }
        }
        let transition = replacement.begin_transition(decision);
        replacement.apply_invalidations(&decision.plan.invalidations);
        replacement.install_effects(&decision.plan);
        replacement.install_barriers(&decision.plan, barrier_events)?;
        replacement.install_owed_acceptances(&decision.plan);
        for inbox_id in inbox_ids {
            let Some(inbox) = replacement.reducer_inbox.get_mut(inbox_id) else {
                return Err(EngineError::InvalidInbox);
            };
            inbox.delivery_status = DeliveryStatus::Consumed;
            inbox.consumed_by = Some(transition.transition_id);
        }
        replacement.refresh_eligibility(Timestamp(0));
        *self = replacement;
        Ok(AtomicInboxConsumeResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            consumed_inbox_ids: inbox_ids.to_vec(),
            reducer_events: Vec::new(),
        })
    }

    /// Commits product runtime state and accepts one owed obligation atomically.
    ///
    /// # Errors
    /// Returns an engine error when the obligation or reducer decision is invalid.
    pub fn runtime_accept_atomically(
        &mut self,
        owed_id: OwedAcceptanceId,
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        self.runtime_acceptance_atomically(
            owed_id,
            decision,
            barrier_events,
            AcceptanceStatus::Accepted,
        )
    }

    /// Commits product runtime state and suppresses one owed obligation atomically.
    ///
    /// # Errors
    /// Returns an engine error when the obligation or reducer decision is invalid.
    pub fn suppress_runtime_acceptance_atomically(
        &mut self,
        owed_id: OwedAcceptanceId,
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
        _reason: SuppressionReason,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        self.runtime_acceptance_atomically(
            owed_id,
            decision,
            barrier_events,
            AcceptanceStatus::Suppressed,
        )
    }

    #[must_use]
    pub fn evaluate_barriers(&mut self) -> BarrierEvaluation<P> {
        let mut newly_satisfied = Vec::new();
        let mut reducer_events = Vec::new();
        let barrier_ids: Vec<BarrierId> = self.barriers.keys().copied().collect();
        for barrier_id in barrier_ids {
            let Some((barrier_status, required_members, reducer_event_payload)) =
                self.barriers.get(&barrier_id).map(|barrier| {
                    (
                        barrier.status,
                        barrier.required_members.clone(),
                        barrier.reducer_event.clone(),
                    )
                })
            else {
                continue;
            };
            if barrier_status == BarrierStatus::Satisfied {
                continue;
            }
            let satisfied = required_members.iter().all(|(effect_id, family)| {
                self.effects.get(effect_id).is_some_and(|effect| {
                    effect.receipt.as_ref().is_some_and(|receipt| {
                        receipt.generation == self.generation
                            && declared_receipt_family(&effect.declaration) == *family
                    })
                })
            });
            if satisfied {
                if let Some(existing) = self.barriers.get_mut(&barrier_id) {
                    existing.status = BarrierStatus::Satisfied;
                }
                newly_satisfied.push(barrier_id);
                let id = ReducerInboxId(self.next_inbox_id);
                self.next_inbox_id += 1;
                let event = ReducerInboxEvent {
                    id,
                    effect_id: None,
                    barrier_id: Some(barrier_id),
                    kind: ReducerInboxKind::BarrierSatisfied,
                    payload: ReducerInboxPayload::Barrier(reducer_event_payload),
                    delivery_status: DeliveryStatus::Pending,
                    consumed_by: None,
                };
                self.reducer_inbox
                    .insert(id, clone_reducer_inbox_event(&event));
                reducer_events.push(event);
            }
        }
        BarrierEvaluation {
            newly_satisfied,
            reducer_events,
        }
    }

    /// Advances the workflow into cancellation and installs the compensation generation as one
    /// compare-and-swap replacement.
    ///
    /// # Errors
    /// Returns [`EngineError`] when the workflow is shadow-only, the compensation plan is invalid,
    /// or replacement application fails before the original state can be swapped.
    pub fn cancel_with_compensation(
        &mut self,
        request: &CancellationRequest<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<CommitResult<P>, EngineError> {
        self.ensure_executable()?;
        validate_plan(&request.compensation_plan, barrier_events)
            .map_err(EngineError::InvalidPlan)?;
        if request.expected_workflow_version != self.version {
            return Ok(version_conflict_result());
        }

        let mut replacement = self.clone();
        replacement.enter_cancellation(request);
        let decision = replacement.cancellation_decision(request);
        let transition = replacement.begin_transition(&decision);
        replacement.apply_invalidations(&decision.plan.invalidations);
        replacement.install_effects(&decision.plan);
        replacement.install_barriers(&decision.plan, barrier_events)?;
        replacement.install_owed_acceptances(&decision.plan);
        replacement.refresh_eligibility(Timestamp(0));
        *self = replacement;

        Ok(CommitResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            reducer_events: Vec::new(),
        })
    }

    pub fn record_shadow_divergence(
        &mut self,
        authoritative_workflow_id: WorkflowId,
        kind: ShadowDivergenceKind,
        evidence_identity: String,
    ) {
        if self.binding.execution_mode() != ExecutionMode::Shadow {
            return;
        }
        let severity = match kind {
            ShadowDivergenceKind::Snapshot
            | ShadowDivergenceKind::Transition
            | ShadowDivergenceKind::EffectPlan
            | ShadowDivergenceKind::Receipt
            | ShadowDivergenceKind::ReducerEvent
            | ShadowDivergenceKind::Capability
            | ShadowDivergenceKind::UserProjection => DivergenceSeverity::Blocking,
            ShadowDivergenceKind::Observation => DivergenceSeverity::Actionable,
        };
        let action = match severity {
            DivergenceSeverity::Blocking => DivergenceAction::HaltAcceptance,
            DivergenceSeverity::Actionable => DivergenceAction::RetainAuthorityAndInvestigate,
            DivergenceSeverity::Informational => DivergenceAction::RecordOnly,
        };
        let id = ShadowDivergenceId(self.next_shadow_divergence_id);
        self.next_shadow_divergence_id += 1;
        self.shadow_divergences.push(ShadowDivergenceRecord {
            id,
            shadow_workflow_id: self.binding.workflow_id(),
            authoritative_workflow_id,
            kind,
            severity,
            action,
            evidence_identity,
        });
    }

    pub fn refresh_eligibility(&mut self, now: Timestamp) {
        let ready_ids: Vec<EffectId> = self
            .effects
            .iter()
            .filter_map(|(effect_id, effect)| {
                ((effect.status == EffectStatus::Blocked
                    || effect.status == EffectStatus::RetryWait)
                    && effect.declaration.generation == self.generation
                    && !effect.pending_reconciliation
                    && effect
                        .declaration
                        .next_eligible_at
                        .is_none_or(|deadline| deadline <= now)
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

    fn ensure_executable(&self) -> Result<(), EngineError> {
        if self.binding.execution_mode() == ExecutionMode::Shadow {
            Err(EngineError::ShadowCannotExecute)
        } else {
            Ok(())
        }
    }

    fn begin_transition(&mut self, decision: &ReducerDecision<P>) -> WorkflowTransition<P::Event> {
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
        self.snapshot_codec = decision.plan.snapshot_codec.clone();
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.version = self.version;
            workflow.generation = self.generation;
        }
        self.transition_log.push(transition.clone());
        transition
    }

    fn apply_invalidations(&mut self, invalidations: &[EffectInvalidationDecl]) {
        for invalidation in invalidations {
            if let Some(effect) = self.effects.get_mut(&invalidation.effect_id) {
                effect.status = EffectStatus::Invalidated;
                effect.claim = None;
                effect.pending_reconciliation = false;
            }
        }
    }

    fn install_effects(&mut self, plan: &TransitionPlan<P>) {
        let mut dependency_map: BTreeMap<EffectId, BTreeSet<EffectId>> = BTreeMap::new();
        for dependency in &plan.dependencies {
            dependency_map
                .entry(dependency.effect_id)
                .or_default()
                .insert(dependency.depends_on_effect_id);
        }

        for effect in &plan.effects {
            let dependencies = dependency_map.remove(&effect.effect_id).unwrap_or_default();
            let status =
                initial_effect_status(effect, &dependencies, Timestamp(0), self.generation);
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
                    stale_observations: Vec::new(),
                    receipt: None,
                    pending_reconciliation: false,
                    destructive_lock: None,
                },
            );
        }
    }

    fn install_barriers(
        &mut self,
        plan: &TransitionPlan<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<(), EngineError> {
        let mut member_map: BTreeMap<BarrierId, BTreeMap<EffectId, ReceiptFamily>> =
            BTreeMap::new();
        for member in &plan.barrier_members {
            member_map
                .entry(member.barrier_id)
                .or_default()
                .insert(member.effect_id, member.receipt_family);
        }
        for barrier in &plan.barriers {
            let members = member_map.remove(&barrier.barrier_id).unwrap_or_default();
            let Some(reducer_event) = barrier_events.get(&barrier.barrier_id).cloned() else {
                return Err(EngineError::MissingValidatedBarrierEvent(
                    barrier.barrier_id,
                ));
            };
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
        Ok(())
    }

    fn install_owed_acceptances(&mut self, plan: &TransitionPlan<P>) {
        if let Some(owed_acceptances) = &plan.owed_acceptances {
            for owed in owed_acceptances {
                let id = OwedAcceptanceId(self.next_owed_acceptance_id);
                self.next_owed_acceptance_id += 1;
                self.owed_acceptances.insert(
                    id,
                    OwedAcceptanceRecord {
                        id,
                        reducer_inbox_id: ReducerInboxId(0),
                        source_kind: owed.source_kind,
                        event: owed.event.clone(),
                        status: AcceptanceStatus::Owed,
                        accepting_transition: None,
                    },
                );
            }
        }
    }

    fn effect_ready_for_manual_resolution(&self, resolution: &ManualResolutionRecord<P>) -> bool {
        self.effects
            .get(&resolution.effect_id)
            .is_some_and(|effect| {
                effect.declaration.generation == self.generation && effect.receipt.is_none()
            })
    }

    fn apply_manual_resolution(
        &mut self,
        resolution_id: ManualResolutionId,
        expected_workflow_version: Version,
        existing: &ManualResolutionRecord<P>,
        choice: &ManualChoice<P>,
        receipt: P::Receipt,
        receipt_event: P::ReceiptReducerEvent,
    ) -> ReceiptRecord<P::Receipt> {
        self.version = self.version.next();
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.version = self.version;
        }

        let receipt_record = ReceiptRecord {
            id: ReceiptId(self.next_receipt_id),
            authority: manual_receipt_authority(
                self.binding.workflow_id(),
                expected_workflow_version,
                self.generation,
                existing.effect_id,
            ),
            attempt_id: None,
            origin: ReceiptOrigin::Manual,
            receipt,
            generation: self.generation,
        };
        self.next_receipt_id += 1;

        let receipt_inbox_event = ReducerInboxEvent {
            id: ReducerInboxId(self.next_inbox_id),
            effect_id: Some(existing.effect_id),
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            payload: ReducerInboxPayload::Receipt(receipt_event),
            delivery_status: DeliveryStatus::Pending,
            consumed_by: None,
        };
        self.next_inbox_id += 1;

        if let Some(effect) = self.effects.get_mut(&existing.effect_id) {
            effect.status = EffectStatus::Receipted;
            effect.claim = None;
            effect.pending_reconciliation = false;
            effect.receipt = Some(receipt_record.clone());
        }
        self.reducer_inbox.insert(
            receipt_inbox_event.id,
            clone_reducer_inbox_event(&receipt_inbox_event),
        );

        let mut updated = clone_manual_resolution(existing);
        updated.status = ResolutionStatus::Resolved;
        updated.workflow_version = self.version;
        updated.accepted_choice = Some(clone_manual_choice(choice));
        self.manual_resolutions.insert(resolution_id, updated);
        self.refresh_eligibility(Timestamp(0));
        receipt_record
    }

    fn enter_cancellation(&mut self, request: &CancellationRequest<P>) {
        self.generation = self.generation.next();
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.generation = self.generation;
        }
        self.status = WorkflowStatus::Cancelling;
        for effect in self.effects.values_mut() {
            if effect.declaration.generation != self.generation {
                effect.pending_reconciliation = false;
                if !matches!(
                    effect.status,
                    EffectStatus::Receipted | EffectStatus::Invalidated
                ) {
                    effect.status = EffectStatus::Invalidated;
                }
            }
        }
        self.snapshot = request.next_snapshot.clone();
        self.snapshot_codec = request.next_snapshot_codec.clone();
    }

    fn cancellation_decision(&self, request: &CancellationRequest<P>) -> ReducerDecision<P> {
        ReducerDecision {
            expected_workflow_version: self.version,
            plan: TransitionPlan {
                snapshot: self.snapshot.clone(),
                snapshot_codec: self.snapshot_codec.clone(),
                event: request.event.clone(),
                event_codec: request.event_codec.clone(),
                effects: request.compensation_plan.effects.clone(),
                dependencies: request.compensation_plan.dependencies.clone(),
                barriers: request.compensation_plan.barriers.clone(),
                barrier_members: request.compensation_plan.barrier_members.clone(),
                invalidations: request
                    .invalidations
                    .iter()
                    .copied()
                    .chain(request.compensation_plan.invalidations.iter().copied())
                    .collect(),
                owed_acceptances: request.compensation_plan.owed_acceptances.clone(),
            },
        }
    }

    fn lock_grant_for_claim(
        &self,
        effect_id: EffectId,
        worker_id: &'static str,
        claim_token: u64,
        now: Timestamp,
        lease_until: LeaseExpiry,
    ) -> Result<Option<ResourceLockGrant>, ()> {
        let effect = self.effects.get(&effect_id).ok_or(())?;
        let Some(resource) = effect.declaration.destructive_resource else {
            return Ok(None);
        };
        for (other_effect_id, other) in &self.effects {
            if *other_effect_id == effect_id {
                continue;
            }
            if other.declaration.destructive_resource != Some(resource) {
                continue;
            }
            if other.claim.as_ref().is_some_and(|claim| {
                claim.generation == self.generation && claim.lease_until.is_live_at(now)
            }) {
                return Err(());
            }
            if other.destructive_lock.as_ref().is_some_and(|lock| {
                lock.generation == self.generation && lock.lease_until.is_live_at(now)
            }) {
                return Err(());
            }
        }
        Ok(Some(ResourceLockGrant {
            resource,
            worker_id,
            claim_token,
            generation: self.generation,
            lease_until,
        }))
    }

    fn runtime_acceptance_atomically(
        &mut self,
        owed_id: OwedAcceptanceId,
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
        next_status: AcceptanceStatus,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        self.ensure_executable()?;
        validate_plan(&decision.plan, barrier_events).map_err(EngineError::InvalidPlan)?;
        if decision.expected_workflow_version != self.version {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                owed_acceptance: None,
            });
        }
        let Some(existing) = self.owed_acceptances.get(&owed_id).cloned() else {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::InvalidPlan,
                transition: None,
                owed_acceptance: None,
            });
        };
        if existing.status != AcceptanceStatus::Owed {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::InvalidPlan,
                transition: None,
                owed_acceptance: Some(existing),
            });
        }
        let mut replacement = self.clone();
        let transition = replacement.begin_transition(decision);
        replacement.apply_invalidations(&decision.plan.invalidations);
        replacement.install_effects(&decision.plan);
        replacement.install_barriers(&decision.plan, barrier_events)?;
        replacement.install_owed_acceptances(&decision.plan);
        let owed = replacement
            .owed_acceptances
            .get_mut(&owed_id)
            .expect("validated owed exists");
        owed.status = next_status;
        owed.accepting_transition = Some(transition.transition_id);
        let owed_acceptance = owed.clone();
        replacement.refresh_eligibility(Timestamp(0));
        *self = replacement;
        Ok(RuntimeAcceptanceResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            owed_acceptance: Some(owed_acceptance),
        })
    }

    fn effect_authorized_mut(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
    ) -> Option<&mut EffectState<P>> {
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
            && claim.lease_until.is_live_at(now)
        {
            Some(effect)
        } else {
            None
        }
    }
}

fn version_conflict_result<P: WorkflowProfile>() -> CommitResult<P> {
    CommitResult {
        outcome: CommitOutcome::VersionConflict,
        transition: None,
        reducer_events: Vec::new(),
    }
}

fn invalid_manual_resolution<P: WorkflowProfile>(
    resolution: Option<ManualResolutionRecord<P>>,
) -> ManualResolutionOutcome<P> {
    ManualResolutionOutcome {
        outcome: CommitOutcome::InvalidPlan,
        resolution,
        receipt: None,
        reducer_event: None,
    }
}

fn version_conflict_manual_resolution<P: WorkflowProfile>(
    resolution: ManualResolutionRecord<P>,
) -> ManualResolutionOutcome<P> {
    ManualResolutionOutcome {
        outcome: CommitOutcome::VersionConflict,
        resolution: Some(resolution),
        receipt: None,
        reducer_event: None,
    }
}

fn manual_choice_permitted<P: WorkflowProfile>(
    resolution: &ManualResolutionRecord<P>,
    choice: &ManualChoice<P>,
) -> bool {
    resolution.permitted_choices.iter().any(|candidate| {
        candidate.kind == choice.kind
            && candidate.codec == choice.codec
            && candidate.payload == choice.payload
    })
}

fn initial_effect_status<I>(
    effect: &EffectDecl<I>,
    dependencies: &BTreeSet<EffectId>,
    now: Timestamp,
    generation: Generation,
) -> EffectStatus {
    if effect.generation != generation {
        return EffectStatus::Blocked;
    }
    if dependencies.is_empty()
        && effect
            .next_eligible_at
            .is_none_or(|deadline| deadline <= now)
    {
        EffectStatus::Eligible
    } else if effect.next_eligible_at.is_some() {
        EffectStatus::RetryWait
    } else {
        EffectStatus::Blocked
    }
}

fn denied_claim() -> ClaimResult {
    ClaimResult {
        outcome: ClaimOutcome::AuthorityConflict,
        authority: None,
        attempt: None,
    }
}

fn ineligible_claim() -> ClaimResult {
    ClaimResult {
        outcome: ClaimOutcome::Ineligible,
        authority: None,
        attempt: None,
    }
}

fn clone_manual_choice<P: WorkflowProfile>(choice: &ManualChoice<P>) -> ManualChoice<P> {
    ManualChoice {
        kind: choice.kind,
        codec: choice.codec.clone(),
        payload: choice.payload.clone(),
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
        payload: match &event.payload {
            ReducerInboxPayload::Receipt(payload) => ReducerInboxPayload::Receipt(payload.clone()),
            ReducerInboxPayload::Barrier(payload) => ReducerInboxPayload::Barrier(payload.clone()),
        },
        delivery_status: event.delivery_status,
        consumed_by: event.consumed_by,
    }
}

fn manual_receipt_authority(
    workflow_id: WorkflowId,
    declared_workflow_version: Version,
    generation: Generation,
    effect_id: EffectId,
) -> ClaimAuthority {
    ClaimAuthority {
        workflow_id,
        declared_workflow_version,
        generation,
        effect_id,
        claim_token: 0,
        worker_id: "manual",
        lease_until: LeaseExpiry(u64::MAX),
        resource_lock: None,
    }
}
