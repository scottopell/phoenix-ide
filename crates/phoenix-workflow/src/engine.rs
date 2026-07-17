use std::collections::{BTreeMap, BTreeSet};

use crate::validation::{validate_plan_body, validate_status_transition, EngineError, PlanError};
use crate::{
    AcceptanceProfile, AttemptAuthority, AttemptId, AttemptRecord, AttemptStatus, AuthorityOutcome,
    BarrierId, BarrierState, BarrierStatus, CancellationRequest, ClaimOutcome, ClaimResult,
    CodecRef, CommitOutcome, CommitResult, DeliveryConsumeResult, DeliveryDecisionBinding,
    DeliveryId, DeliveryItem, DeliveryPayload, DeliveryStatus, EffectId, EffectInvalidationDecl,
    EffectState, EffectStatus, ExecutionCapability, Generation, IncompatibleWorkflow, LeaseExpiry,
    ManualChoice, ManualChoiceKind, ManualEffectOutcome, ManualResolutionCommit,
    ManualResolutionId, ManualResolutionOutcome, ManualResolutionRecord, ObservationId,
    ObservationRecord, ProcessIncarnation, ProfileMigrationOutcome, ProfileRef, ReceiptAcceptance,
    ReceiptId, ReceiptOrigin, ReceiptRecord, ReclaimableLease, ReconciliationDecision,
    ReconciliationOutcome, ReducerDecision, RenewalResult, ResolutionStatus, ResourceLockGrant,
    RuntimeAcceptanceResult, RuntimeAcceptanceStatus, ScheduleDecl, ScheduleId, SchedulePolicy,
    ScheduleState, ScheduleStatus, StaleObservationRecord, SuppressionReason, Timestamp,
    TransitionId, TransitionPlan, Version, WorkflowBinding, WorkflowId, WorkflowProfile,
    WorkflowState, WorkflowStatus, WorkflowTransition,
};

#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
impl<P: WorkflowProfile> WorkflowState<P> {
    pub fn new(
        workflow_id: WorkflowId,
        profile: &ProfileRef,
        acceptance: AcceptanceProfile,
        snapshot_codec: CodecRef,
        snapshot: P::Snapshot,
    ) -> Result<Self, EngineError> {
        if *profile != acceptance.profile {
            return Err(EngineError::ProfileProtocolMismatch);
        }
        if snapshot_codec.family.is_empty() {
            return Err(EngineError::InvalidPlan(PlanError::MissingCodec(
                "snapshot",
            )));
        }
        if !acceptance.supported_codecs.supports(&snapshot_codec) {
            return Err(EngineError::UnsupportedCodec(snapshot_codec));
        }
        Ok(Self {
            binding: WorkflowBinding {
                workflow_id,
                profile: profile.clone(),
                acceptance,
            },
            version: Version(0),
            generation: Generation(0),
            status: WorkflowStatus::Active,
            snapshot,
            snapshot_codec,
            effects: BTreeMap::new(),
            barriers: BTreeMap::new(),
            deliveries: BTreeMap::new(),
            schedules: BTreeMap::new(),
            manual_resolutions: BTreeMap::new(),
            transition_log: Vec::new(),
            process_incarnation: ProcessIncarnation(1),
            incompatible: None,
            crashed_workers: BTreeSet::new(),
            next_transition_id: 1,
            next_attempt_id: 1,
            next_observation_id: 1,
            next_receipt_id: 1,
            next_delivery_id: 1,
            next_manual_resolution_id: 1,
        })
    }

    pub fn migrate_profile(
        &mut self,
        target: &ProfileRef,
        now: Timestamp,
    ) -> ProfileMigrationOutcome {
        if self.binding.profile == *target {
            return ProfileMigrationOutcome::UpToDate;
        }
        if self.status == WorkflowStatus::Completed
            || self.status == WorkflowStatus::Cancelled
            || self.status == WorkflowStatus::Deleted
            || self.status == WorkflowStatus::Failed
        {
            self.binding.profile = target.clone();
            return ProfileMigrationOutcome::Migrated;
        }
        self.incompatible = Some(IncompatibleWorkflow {
            workflow_id: self.binding.workflow_id,
            stored_profile: self.binding.profile.clone(),
            detected_at: now,
            disposition: "manual-preservation",
        });
        ProfileMigrationOutcome::Incompatible
    }

    pub fn commit_transition(
        &mut self,
        decision: &ReducerDecision<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<CommitResult<P>, EngineError> {
        if decision.expected_workflow_version != self.version {
            return Ok(CommitResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                deliveries: Vec::new(),
            });
        }
        validate_status_transition(self.status, decision.plan.next_status)
            .map_err(EngineError::InvalidPlan)?;
        validate_plan_body(
            &decision.plan,
            barrier_events,
            &self.binding.acceptance.supported_codecs,
        )
        .map_err(EngineError::InvalidPlan)?;

        let transition = self.begin_transition(decision);
        self.apply_invalidations(&decision.plan.invalidations);
        self.install_effects(&decision.plan);
        self.install_barriers(&decision.plan, barrier_events)?;
        let deliveries = self.install_declared_deliveries(&decision.plan);
        self.install_schedules(&decision.plan);
        self.refresh_eligibility(Timestamp(0));
        Ok(CommitResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            deliveries,
        })
    }

    pub fn claim_effect(
        &mut self,
        effect_id: EffectId,
        now: Timestamp,
        lease_until: Option<LeaseExpiry>,
    ) -> ClaimResult {
        let Some(effect) = self.effects.get(&effect_id) else {
            return ineligible_claim();
        };
        if effect.status != EffectStatus::Eligible
            || effect.declaration.generation != self.generation
            || effect.pending_reconciliation
        {
            return ineligible_claim();
        }
        let Ok(ordinal_base) = u32::try_from(effect.attempts.len()) else {
            return ineligible_claim();
        };
        let Some(ordinal) = ordinal_base.checked_add(1) else {
            return ineligible_claim();
        };
        let attempt_id = AttemptId(self.next_attempt_id);
        let authority = AttemptAuthority {
            workflow_id: self.binding.workflow_id,
            declared_workflow_version: effect.declared_workflow_version,
            generation: self.generation,
            effect_id,
            attempt_id,
            process_incarnation: self.process_incarnation,
        };
        let reclaimable_lease = match lease_for_capability(
            &effect.declaration.capability,
            attempt_id,
            now,
            lease_until,
        ) {
            Ok(lease) => lease,
            Err(outcome) => {
                return ClaimResult {
                    outcome,
                    authority: None,
                    attempt: None,
                }
            }
        };
        let attempt = AttemptRecord {
            id: attempt_id,
            ordinal,
            authority: authority.clone(),
            status: AttemptStatus::Begun,
            lease: reclaimable_lease.clone(),
        };
        self.next_attempt_id += 1;
        let effect = self.effects.get_mut(&effect_id).expect("effect exists");
        effect.status = EffectStatus::Executing;
        effect.authority = Some(authority.clone());
        effect.reclaimable_lease.clone_from(&reclaimable_lease);
        effect.attempts.push(attempt.clone());
        ClaimResult {
            outcome: ClaimOutcome::Started,
            authority: Some(authority),
            attempt: Some(attempt),
        }
    }

    pub fn renew_lease(
        &mut self,
        authority: &AttemptAuthority,
        now: Timestamp,
        new_lease_until: LeaseExpiry,
    ) -> RenewalResult {
        let Some(effect) = self.effect_authorized_mut(authority, now, true) else {
            return stale_renewal();
        };
        let Some(existing) = &mut effect.reclaimable_lease else {
            return stale_renewal();
        };
        if new_lease_until <= existing.lease_until || !new_lease_until.is_live_at(now) {
            return stale_renewal();
        }
        existing.lease_until = new_lease_until;
        if let Some(attempt) = effect
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == authority.attempt_id)
        {
            attempt.lease = Some(existing.clone());
        }
        RenewalResult {
            outcome: AuthorityOutcome::Authorized,
            authority: Some(authority.clone()),
        }
    }

    pub fn expire_lease(
        &mut self,
        effect_id: EffectId,
        attempt_id: AttemptId,
        now: Timestamp,
    ) -> AuthorityOutcome {
        let Some(effect) = self.effects.get_mut(&effect_id) else {
            return AuthorityOutcome::StaleAuthority;
        };
        if effect
            .authority
            .as_ref()
            .is_none_or(|authority| authority.attempt_id != attempt_id)
        {
            return AuthorityOutcome::StaleAuthority;
        }
        let Some(lease) = effect.reclaimable_lease.as_ref() else {
            return AuthorityOutcome::StaleAuthority;
        };
        if lease.lease_until.is_live_at(now) {
            return AuthorityOutcome::StaleAuthority;
        }
        effect.authority = None;
        effect.reclaimable_lease = None;
        effect.destructive_lock = None;
        if let Some(attempt) = effect
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
        {
            attempt.status = AttemptStatus::AuthorityLost;
            attempt.lease = None;
        }
        effect.status = match effect.declaration.capability {
            ExecutionCapability::ReclaimableObservation => EffectStatus::Eligible,
            ExecutionCapability::IdempotentSubmission { .. }
            | ExecutionCapability::ObservableSubmission { .. }
            | ExecutionCapability::ManualOnAmbiguity => EffectStatus::AmbiguityWait,
            ExecutionCapability::SafelyRepeatable => EffectStatus::RetryWait,
        };
        AuthorityOutcome::Authorized
    }

    pub fn record_observation(
        &mut self,
        authority: &AttemptAuthority,
        now: Timestamp,
        observed_at: Timestamp,
        attempt_id: AttemptId,
        observation_codec: CodecRef,
        observation: P::Observation,
    ) -> AuthorityOutcome {
        if observation_codec.family.is_empty() {
            return AuthorityOutcome::StaleAuthority;
        }
        let observation_id = ObservationId(self.next_observation_id);
        self.next_observation_id += 1;
        let Some(effect) = self.effect_authorized_mut(authority, now, true) else {
            return self.record_stale_observation(
                observation_id,
                authority,
                attempt_id,
                observation_codec,
                observed_at,
                now,
                observation,
            );
        };
        effect.observations.push(ObservationRecord {
            id: observation_id,
            authority: authority.clone(),
            attempt_id,
            observation_codec,
            observation,
            observed_at,
            recorded_at: now,
            authoritative: true,
        });
        if let Some(attempt) = effect
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
        {
            attempt.status = AttemptStatus::ObservationRecorded;
        }
        AuthorityOutcome::Authorized
    }

    #[allow(clippy::too_many_arguments)]
    pub fn accept_receipt(
        &mut self,
        authority: &AttemptAuthority,
        now: Timestamp,
        attempt_id: Option<AttemptId>,
        origin: ReceiptOrigin,
        receipt_codec: CodecRef,
        receipt: P::Receipt,
        receipt_event_codec: CodecRef,
        receipt_event: P::ReceiptReducerEvent,
    ) -> ReceiptAcceptance<P> {
        if receipt_codec.family.is_empty() || receipt_event_codec.family.is_empty() {
            return stale_receipt();
        }
        let receipt_id = ReceiptId(self.next_receipt_id);
        let delivery_id = DeliveryId(self.next_delivery_id);
        let generation = self.generation;
        let requires_runtime_acceptance = P::receipt_requires_runtime_acceptance(&receipt_event);
        let receipt_record = ReceiptRecord {
            id: receipt_id,
            authority: authority.clone(),
            attempt_id,
            origin,
            receipt_codec,
            receipt,
            generation,
        };
        let delivery = DeliveryItem {
            id: delivery_id,
            effect_id: Some(authority.effect_id),
            barrier_id: None,
            consumer_kind: "reducer",
            event_codec: receipt_event_codec,
            requires_runtime_acceptance,
            payload: DeliveryPayload::Receipt(receipt_event),
            status: DeliveryStatus::Pending,
            runtime_acceptance_status: requires_runtime_acceptance
                .then_some(RuntimeAcceptanceStatus::Owed),
            suppression_reason: None,
            accepted_by: None,
        };
        self.next_receipt_id += 1;
        self.next_delivery_id += 1;
        let Some(effect) =
            self.effect_authorized_mut(authority, now, origin != ReceiptOrigin::Manual)
        else {
            return stale_receipt();
        };
        if effect.receipt.is_some() || attempt_id != Some(authority.attempt_id) {
            return stale_receipt();
        }
        effect.receipt = Some(receipt_record.clone());
        effect.status = EffectStatus::Receipted;
        effect.authority = None;
        effect.reclaimable_lease = None;
        effect.pending_reconciliation = false;
        if let Some(attempt) = effect
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == authority.attempt_id)
        {
            attempt.status = AttemptStatus::ReceiptAccepted;
            attempt.lease = None;
        }
        self.deliveries.insert(delivery_id, delivery.clone());
        ReceiptAcceptance {
            outcome: AuthorityOutcome::Authorized,
            receipt: Some(receipt_record),
            delivery_ids: vec![delivery_id],
            deliveries: vec![delivery],
        }
    }

    pub fn schedule_retry(
        &mut self,
        authority: &AttemptAuthority,
        now: Timestamp,
        retry_at: Timestamp,
    ) -> ReconciliationOutcome<P> {
        let Some(effect) = self.effect_authorized_mut(authority, now, false) else {
            return stale_reconciliation_outcome();
        };
        match effect.declaration.capability {
            ExecutionCapability::ManualOnAmbiguity
            | ExecutionCapability::ObservableSubmission { .. } => {
                return stale_reconciliation_outcome()
            }
            ExecutionCapability::ReclaimableObservation
            | ExecutionCapability::IdempotentSubmission { .. }
            | ExecutionCapability::SafelyRepeatable => {}
        }
        effect.status = EffectStatus::RetryWait;
        effect.declaration.next_eligible_at = Some(retry_at);
        effect.authority = None;
        effect.reclaimable_lease = None;
        ReconciliationOutcome {
            outcome: AuthorityOutcome::Authorized,
            decision: Some(ReconciliationDecision::RetryInfrastructure),
            manual_resolution: None,
        }
    }

    pub fn require_manual_resolution(
        &mut self,
        authority: &AttemptAuthority,
        now: Timestamp,
        permitted_choices: Vec<ManualChoice<P>>,
    ) -> ReconciliationOutcome<P> {
        if permitted_choices.is_empty() {
            return stale_reconciliation_outcome();
        }
        let resolution_id = ManualResolutionId(self.next_manual_resolution_id);
        let workflow_version = self.version;
        let Some(effect) = self.effect_authorized_mut(authority, now, false) else {
            return stale_reconciliation_outcome();
        };
        effect.status = EffectStatus::AmbiguityWait;
        effect.authority = None;
        effect.reclaimable_lease = None;
        effect.pending_reconciliation = true;
        let resolution: ManualResolutionRecord<P> = ManualResolutionRecord {
            id: resolution_id,
            workflow_version,
            effect_id: authority.effect_id,
            status: ResolutionStatus::Required,
            evidence: effect.observations.clone(),
            permitted_choices,
            accepted_choice: None,
            resolved_by: None,
        };
        self.next_manual_resolution_id += 1;
        self.manual_resolutions
            .insert(resolution_id, resolution.clone());
        ReconciliationOutcome {
            outcome: AuthorityOutcome::Authorized,
            decision: Some(ReconciliationDecision::RequestManualResolution),
            manual_resolution: Some(resolution),
        }
    }

    pub fn resolve_manual(
        &mut self,
        resolution_id: ManualResolutionId,
        expected_workflow_version: Version,
        resolved_by: &'static str,
        choice: &ManualChoice<P>,
        commit: &ManualResolutionCommit<P>,
    ) -> ManualResolutionOutcome<P> {
        if expected_workflow_version != self.version {
            return invalid_manual_resolution(None);
        }
        let Some(mut resolution) = self.manual_resolutions.get(&resolution_id).cloned() else {
            return invalid_manual_resolution(None);
        };
        let Some(effect) = self.effects.get_mut(&resolution.effect_id) else {
            return invalid_manual_resolution(None);
        };
        if effect.status != EffectStatus::AmbiguityWait {
            return invalid_manual_resolution(Some(resolution));
        }
        resolution.status = ResolutionStatus::Resolved;
        resolution.accepted_choice = Some(choice.clone());
        resolution.resolved_by = Some(resolved_by);
        self.manual_resolutions
            .insert(resolution_id, resolution.clone());
        match choice.kind {
            ManualChoiceKind::Retry => {
                effect.status = EffectStatus::RetryWait;
                effect.declaration.next_eligible_at = commit.retry_at;
                ManualResolutionOutcome {
                    outcome: CommitOutcome::Committed,
                    resolution: Some(resolution),
                    effect_outcome: Some(ManualEffectOutcome::Retry),
                }
            }
            ManualChoiceKind::Compensate => {
                effect.status = EffectStatus::Invalidated;
                ManualResolutionOutcome {
                    outcome: CommitOutcome::Committed,
                    resolution: Some(resolution),
                    effect_outcome: Some(ManualEffectOutcome::Compensate),
                }
            }
            ManualChoiceKind::Suppress => {
                effect.status = EffectStatus::Invalidated;
                ManualResolutionOutcome {
                    outcome: CommitOutcome::Committed,
                    resolution: Some(resolution),
                    effect_outcome: Some(ManualEffectOutcome::Suppressed),
                }
            }
            ManualChoiceKind::AcceptAsTerminal => {
                let authority = manual_receipt_authority(
                    self.binding.workflow_id,
                    effect.declared_workflow_version,
                    self.generation,
                    effect.declaration.effect_id,
                    self.process_incarnation,
                );
                let receipt = ReceiptRecord {
                    id: ReceiptId(self.next_receipt_id),
                    authority,
                    attempt_id: None,
                    origin: ReceiptOrigin::Manual,
                    receipt_codec: choice.receipt_codec.clone(),
                    receipt: choice.receipt.clone(),
                    generation: self.generation,
                };
                self.next_receipt_id += 1;
                effect.receipt = Some(receipt.clone());
                effect.status = EffectStatus::Receipted;
                let delivery: DeliveryItem<P> = DeliveryItem {
                    id: DeliveryId(self.next_delivery_id),
                    effect_id: Some(effect.declaration.effect_id),
                    barrier_id: None,
                    consumer_kind: "reducer",
                    event_codec: choice.receipt_event_codec.clone(),
                    requires_runtime_acceptance: P::receipt_requires_runtime_acceptance(
                        &choice.receipt_event,
                    ),
                    payload: DeliveryPayload::Receipt(choice.receipt_event.clone()),
                    status: DeliveryStatus::Pending,
                    runtime_acceptance_status: P::receipt_requires_runtime_acceptance(
                        &choice.receipt_event,
                    )
                    .then_some(RuntimeAcceptanceStatus::Owed),
                    suppression_reason: None,
                    accepted_by: None,
                };
                self.next_delivery_id += 1;
                self.deliveries.insert(delivery.id, delivery.clone());
                ManualResolutionOutcome {
                    outcome: CommitOutcome::Committed,
                    resolution: Some(resolution),
                    effect_outcome: Some(ManualEffectOutcome::Receipt {
                        receipt: Box::new(receipt),
                        reducer_event: Box::new(delivery),
                    }),
                }
            }
        }
    }

    pub fn consume_deliveries(
        &mut self,
        binding: &DeliveryDecisionBinding<P>,
    ) -> Result<DeliveryConsumeResult<P>, EngineError> {
        if binding.decision.expected_workflow_version != self.version {
            return Ok(DeliveryConsumeResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                consumed_delivery_ids: Vec::new(),
                deliveries: Vec::new(),
            });
        }
        let mut consumed_ids = Vec::new();
        for item in &binding.items {
            let Some(existing) = self.deliveries.get(&item.id) else {
                return Err(EngineError::InvalidInbox);
            };
            if existing.status != DeliveryStatus::Pending
                || !P::decision_handles_delivery(existing, &binding.decision.plan.event)
            {
                return Err(EngineError::InvalidInbox);
            }
            consumed_ids.push(item.id);
        }
        let transition = self.begin_transition(&binding.decision);
        let mut items = Vec::new();
        for id in &consumed_ids {
            let item = self.deliveries.get_mut(id).expect("delivery exists");
            item.status = DeliveryStatus::Accepted;
            item.accepted_by = Some(transition.transition_id);
            items.push(item.clone());
        }
        Ok(DeliveryConsumeResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            consumed_delivery_ids: consumed_ids,
            deliveries: items,
        })
    }

    pub fn accept_runtime_delivery(
        &mut self,
        delivery_id: DeliveryId,
        decision: &ReducerDecision<P>,
        suppress: bool,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        if decision.expected_workflow_version != self.version {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                delivery: None,
            });
        }
        let Some(item) = self.deliveries.get(&delivery_id).cloned() else {
            return Err(EngineError::InvalidInbox);
        };
        if item.runtime_acceptance_status != Some(RuntimeAcceptanceStatus::Owed) {
            return Err(EngineError::InvalidInbox);
        }
        let predicate = if suppress {
            P::decision_handles_runtime_suppression(&item, &decision.plan.event)
        } else {
            P::decision_handles_runtime_acceptance(&item, &decision.plan.event)
        };
        if !predicate {
            return Err(EngineError::InvalidInbox);
        }
        let transition = self.begin_transition(decision);
        let item = self
            .deliveries
            .get_mut(&delivery_id)
            .expect("delivery exists");
        item.runtime_acceptance_status = Some(if suppress {
            RuntimeAcceptanceStatus::Suppressed
        } else {
            RuntimeAcceptanceStatus::Accepted
        });
        if suppress {
            item.status = DeliveryStatus::Suppressed;
            item.suppression_reason = Some(SuppressionReason::ReducerTerminal);
        }
        item.accepted_by = Some(transition.transition_id);
        Ok(RuntimeAcceptanceResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            delivery: Some(item.clone()),
        })
    }

    pub fn cancel_with_compensation(
        &mut self,
        request: &CancellationRequest<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<CommitResult<P>, EngineError> {
        if request.expected_workflow_version != self.version {
            return Ok(CommitResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                deliveries: Vec::new(),
            });
        }
        self.generation = self.generation.next();
        self.status = WorkflowStatus::Cancelling;
        self.apply_invalidations(&request.invalidations);
        self.commit_transition(
            &ReducerDecision {
                expected_workflow_version: self.version,
                plan: request.compensation_plan.clone(),
            },
            barrier_events,
        )
    }

    pub fn refresh_eligibility(&mut self, now: Timestamp) {
        let receipted: BTreeSet<EffectId> = self
            .effects
            .iter()
            .filter_map(|(effect_id, effect)| {
                (effect.status == EffectStatus::Receipted).then_some(*effect_id)
            })
            .collect();
        for effect in self.effects.values_mut() {
            if effect.status == EffectStatus::Blocked
                && effect.declaration.generation == self.generation
                && !effect.pending_reconciliation
                && effect
                    .declaration
                    .next_eligible_at
                    .is_none_or(|deadline| deadline <= now)
                && effect
                    .dependencies
                    .iter()
                    .all(|dependency_id| receipted.contains(dependency_id))
            {
                effect.status = EffectStatus::Eligible;
            }
            if effect.status == EffectStatus::RetryWait
                && effect
                    .declaration
                    .next_eligible_at
                    .is_some_and(|deadline| deadline <= now)
            {
                effect.status = EffectStatus::Eligible;
            }
        }
        for schedule in self.schedules.values_mut() {
            if schedule.status == ScheduleStatus::Idle && schedule.next_eligible_at <= now {
                schedule.status = ScheduleStatus::Due;
            }
        }
    }

    pub fn advance_schedule(
        &mut self,
        schedule_id: ScheduleId,
        now: Timestamp,
    ) -> Option<ScheduleState> {
        let schedule = self.schedules.get_mut(&schedule_id)?;
        if schedule.policy != SchedulePolicy::CoalesceLatest || schedule.next_eligible_at > now {
            return None;
        }
        schedule.status = ScheduleStatus::Due;
        Some(schedule.clone())
    }

    pub fn complete_schedule_occurrence(
        &mut self,
        schedule_id: ScheduleId,
        next_eligible_at: Timestamp,
    ) -> Option<ScheduleState> {
        let schedule = self.schedules.get_mut(&schedule_id)?;
        schedule.status = ScheduleStatus::Idle;
        schedule.active_effect_id = None;
        schedule.next_eligible_at = next_eligible_at;
        Some(schedule.clone())
    }

    fn begin_transition(&mut self, decision: &ReducerDecision<P>) -> WorkflowTransition<P::Event> {
        let next_version = self.version.next();
        let transition = WorkflowTransition {
            transition_id: TransitionId(self.next_transition_id),
            from_version: self.version,
            to_version: next_version,
            generation: self.generation,
            event: decision.plan.event.clone(),
            event_codec: decision.plan.event_codec.clone(),
        };
        self.next_transition_id += 1;
        self.version = next_version;
        self.status = decision.plan.next_status;
        self.snapshot = decision.plan.snapshot.clone();
        self.snapshot_codec = decision.plan.snapshot_codec.clone();
        self.transition_log.push(transition.clone());
        transition
    }

    fn apply_invalidations(&mut self, invalidations: &[EffectInvalidationDecl]) {
        let targets: BTreeSet<EffectId> = invalidations.iter().map(|item| item.effect_id).collect();
        for effect_id in targets {
            if let Some(effect) = self.effects.get_mut(&effect_id) {
                if matches!(
                    effect.status,
                    EffectStatus::Blocked
                        | EffectStatus::Eligible
                        | EffectStatus::Executing
                        | EffectStatus::RetryWait
                        | EffectStatus::AmbiguityWait
                ) {
                    effect.status = EffectStatus::Invalidated;
                }
                effect.authority = None;
                effect.reclaimable_lease = None;
                effect.pending_reconciliation = false;
            }
        }
    }

    fn install_effects(&mut self, plan: &TransitionPlan<P>) {
        for declaration in &plan.effects {
            let status = if declaration.generation == self.generation
                && declaration.next_eligible_at.is_none()
            {
                EffectStatus::Eligible
            } else {
                EffectStatus::Blocked
            };
            self.effects.insert(
                declaration.effect_id,
                EffectState {
                    declaration: declaration.clone(),
                    declared_workflow_version: self.version,
                    status,
                    dependencies: plan
                        .dependencies
                        .iter()
                        .filter(|dep| dep.effect_id == declaration.effect_id)
                        .map(|dep| dep.depends_on_effect_id)
                        .collect(),
                    authority: None,
                    reclaimable_lease: None,
                    attempts: Vec::new(),
                    observations: Vec::new(),
                    stale_observations: Vec::new(),
                    receipt: None,
                    pending_reconciliation: false,
                    destructive_lock: declaration.destructive_resource.map(|resource| {
                        ResourceLockGrant {
                            resource,
                            generation: self.generation,
                            lease_until: LeaseExpiry::MAX_FINITE,
                        }
                    }),
                },
            );
        }
    }

    fn install_barriers(
        &mut self,
        plan: &TransitionPlan<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<(), EngineError> {
        for declaration in &plan.barriers {
            let Some(event) = barrier_events.get(&declaration.barrier_id) else {
                return Err(EngineError::MissingValidatedBarrierEvent(
                    declaration.barrier_id,
                ));
            };
            self.barriers.insert(
                declaration.barrier_id,
                BarrierState {
                    barrier_id: declaration.barrier_id,
                    status: BarrierStatus::Waiting,
                    required_members: plan
                        .barrier_members
                        .iter()
                        .filter(|member| member.barrier_id == declaration.barrier_id)
                        .map(|member| (member.effect_id, member.receipt_family))
                        .collect(),
                    reducer_event_codec: declaration.reducer_event_codec.clone(),
                    reducer_event: event.clone(),
                },
            );
        }
        Ok(())
    }

    fn install_declared_deliveries(&mut self, plan: &TransitionPlan<P>) -> Vec<DeliveryItem<P>> {
        let mut deliveries = Vec::new();
        for declaration in &plan.deliveries {
            let item = DeliveryItem {
                id: DeliveryId(self.next_delivery_id),
                effect_id: declaration.effect_id,
                barrier_id: declaration.barrier_id,
                consumer_kind: declaration.consumer_kind,
                event_codec: declaration.event_codec.clone(),
                requires_runtime_acceptance: declaration.requires_runtime_acceptance,
                payload: declaration.payload.clone(),
                status: DeliveryStatus::Pending,
                runtime_acceptance_status: declaration
                    .requires_runtime_acceptance
                    .then_some(RuntimeAcceptanceStatus::Owed),
                suppression_reason: None,
                accepted_by: None,
            };
            self.next_delivery_id += 1;
            self.deliveries.insert(item.id, item.clone());
            deliveries.push(item);
        }
        deliveries
    }

    fn install_schedules(&mut self, plan: &TransitionPlan<P>) {
        for ScheduleDecl {
            schedule_id,
            policy,
            next_eligible_at,
            key,
        } in &plan.schedules
        {
            self.schedules.insert(
                *schedule_id,
                ScheduleState {
                    schedule_id: *schedule_id,
                    policy: *policy,
                    key,
                    status: ScheduleStatus::Idle,
                    next_eligible_at: *next_eligible_at,
                    active_effect_id: None,
                },
            );
        }
    }

    fn effect_authorized_mut(
        &mut self,
        authority: &AttemptAuthority,
        now: Timestamp,
        allow_lease_expired: bool,
    ) -> Option<&mut EffectState<P>> {
        if authority.workflow_id != self.binding.workflow_id
            || authority.generation != self.generation
            || authority.process_incarnation != self.process_incarnation
        {
            return None;
        }
        let effect = self.effects.get_mut(&authority.effect_id)?;
        if effect.declared_workflow_version != authority.declared_workflow_version
            || effect.authority.as_ref()? != authority
        {
            return None;
        }
        if effect
            .reclaimable_lease
            .as_ref()
            .is_some_and(|lease| !allow_lease_expired && !lease.lease_until.is_live_at(now))
        {
            return None;
        }
        Some(effect)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_stale_observation(
        &mut self,
        observation_id: ObservationId,
        authority: &AttemptAuthority,
        attempt_id: AttemptId,
        observation_codec: CodecRef,
        observed_at: Timestamp,
        recorded_at: Timestamp,
        observation: P::Observation,
    ) -> AuthorityOutcome {
        if let Some(effect) = self.effects.get_mut(&authority.effect_id) {
            effect.stale_observations.push(StaleObservationRecord {
                id: observation_id,
                authority: authority.clone(),
                attempt_id,
                observation_codec,
                observed_at,
                recorded_at,
                observation,
            });
        }
        AuthorityOutcome::StaleAuthority
    }
}

#[allow(clippy::wildcard_enum_match_arm)]
fn lease_for_capability(
    capability: &ExecutionCapability,
    attempt_id: AttemptId,
    now: Timestamp,
    lease_until: Option<LeaseExpiry>,
) -> Result<Option<ReclaimableLease>, ClaimOutcome> {
    match capability {
        ExecutionCapability::ReclaimableObservation => {
            let Some(lease_until) = lease_until else {
                return Err(ClaimOutcome::AuthorityConflict);
            };
            if !lease_until.is_live_at(now) {
                return Err(ClaimOutcome::AuthorityConflict);
            }
            Ok(Some(ReclaimableLease {
                attempt_id,
                lease_until,
            }))
        }
        _ => Ok(None),
    }
}

fn ineligible_claim() -> ClaimResult {
    ClaimResult {
        outcome: ClaimOutcome::Ineligible,
        authority: None,
        attempt: None,
    }
}

fn stale_renewal() -> RenewalResult {
    RenewalResult {
        outcome: AuthorityOutcome::StaleAuthority,
        authority: None,
    }
}

fn stale_receipt<P: WorkflowProfile>() -> ReceiptAcceptance<P> {
    ReceiptAcceptance {
        outcome: AuthorityOutcome::StaleAuthority,
        receipt: None,
        delivery_ids: Vec::new(),
        deliveries: Vec::new(),
    }
}

fn stale_reconciliation_outcome<P: WorkflowProfile>() -> ReconciliationOutcome<P> {
    ReconciliationOutcome {
        outcome: AuthorityOutcome::StaleAuthority,
        decision: None,
        manual_resolution: None,
    }
}

fn invalid_manual_resolution<P: WorkflowProfile>(
    resolution: Option<ManualResolutionRecord<P>>,
) -> ManualResolutionOutcome<P>
where
    P::Observation: Clone + Eq,
    P::ManualPayload: Clone + Eq,
    P::Receipt: Clone + Eq,
    P::BarrierEvent: Clone + Eq,
{
    ManualResolutionOutcome {
        outcome: CommitOutcome::InvalidPlan,
        resolution,
        effect_outcome: None,
    }
}

fn manual_receipt_authority(
    workflow_id: WorkflowId,
    declared_workflow_version: Version,
    generation: Generation,
    effect_id: EffectId,
    process_incarnation: ProcessIncarnation,
) -> AttemptAuthority {
    AttemptAuthority {
        workflow_id,
        declared_workflow_version,
        generation,
        effect_id,
        attempt_id: AttemptId(0),
        process_incarnation,
    }
}
