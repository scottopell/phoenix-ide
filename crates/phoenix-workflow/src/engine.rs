use std::collections::{BTreeMap, BTreeSet};

use crate::validation::{validate_plan_body, validate_status_transition, EngineError, PlanError};
use crate::{
    AcceptanceProfile, AttemptAuthority, AttemptId, AttemptRecord, AttemptStatus, AuthorityOutcome,
    BarrierId, BarrierState, BarrierStatus, CancellationReceiptDecl, CancellationRequest,
    ClaimOutcome, ClaimResult, CodecRef, CommitOutcome, CommitResult, DeliveryConsumeResult,
    DeliveryDecisionBinding, DeliveryId, DeliveryItem, DeliveryPayload, DeliveryStatus, EffectId,
    EffectInvalidationDecl, EffectRole, EffectState, EffectStatus, ExecutionCapability, Generation,
    IncompatibleWorkflow, LeaseExpiry, ManualChoice, ManualChoiceKind, ManualEffectOutcome,
    ManualResolutionCommit, ManualResolutionId, ManualResolutionOutcome, ManualResolutionRecord,
    ObservationId, ObservationRecord, ProcessIncarnation, ProfileMigrationOutcome, ProfileRef,
    ReceiptAcceptance, ReceiptFamily, ReceiptId, ReceiptOrigin, ReceiptRecord, ReclaimableLease,
    ReconciliationDecision, ReconciliationOutcome, ReducerDecision, RenewalResult,
    ResolutionStatus, ResourceLockGrant, RuntimeAcceptanceResult, RuntimeAcceptanceStatus,
    ScheduleDecl, ScheduleId, ScheduleOccurrence, ScheduleOccurrenceId, SchedulePolicy,
    ScheduleState, ScheduleStatus, StaleObservationRecord, SuppressionReason, Timestamp,
    TransitionId, TransitionPlan, Version, WorkflowBinding, WorkflowId, WorkflowProfile,
    WorkflowState, WorkflowStatus, WorkflowTransition,
};

#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
impl<P: WorkflowProfile> WorkflowState<P> {
    pub fn new(
        workflow_id: WorkflowId,
        profile: &ProfileRef,
        acceptance: AcceptanceProfile<P::RuntimeAcceptance, P::ExternalAcceptance>,
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
                acceptance: acceptance.erase(),
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
            next_schedule_occurrence_id: 1,
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
        self.status = WorkflowStatus::Incompatible;
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
        for invalidation in &decision.plan.invalidations {
            let Some(effect) = self.effects.get(&invalidation.effect_id) else {
                return Err(EngineError::InvalidPlan(
                    PlanError::UnknownInvalidationTarget(invalidation.effect_id),
                ));
            };
            if effect.receipt.is_some() {
                return Err(EngineError::InvalidPlan(
                    PlanError::InvalidatesReceiptedEffect(invalidation.effect_id),
                ));
            }
        }
        for effect in &decision.plan.effects {
            if self.effects.contains_key(&effect.effect_id) {
                return Err(EngineError::InvalidPlan(PlanError::EffectIdCollision(
                    effect.effect_id,
                )));
            }
        }
        for barrier in &decision.plan.barriers {
            if self.barriers.contains_key(&barrier.barrier_id) {
                return Err(EngineError::InvalidPlan(PlanError::BarrierIdCollision(
                    barrier.barrier_id,
                )));
            }
        }
        for schedule in &decision.plan.schedules {
            if self.schedules.contains_key(&schedule.schedule_id)
                || self
                    .schedules
                    .values()
                    .any(|existing| existing.key == schedule.key)
            {
                return Err(EngineError::InvalidPlan(PlanError::ScheduleIdCollision(
                    schedule.schedule_id,
                )));
            }
        }

        let transition = self.begin_transition(decision);
        self.apply_invalidations(&decision.plan.invalidations);
        self.install_effects(&decision.plan);
        self.install_barriers(&decision.plan, barrier_events)?;
        let deliveries = self.install_declared_deliveries(&decision.plan);
        self.install_schedules(&decision.plan);
        self.refresh_eligibility(Timestamp(0));
        self.refresh_barriers();
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
        if matches!(
            self.status,
            WorkflowStatus::ManualResolution | WorkflowStatus::Incompatible
        ) {
            return ineligible_claim();
        }
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
        if !existing.lease_until.is_live_at(now)
            || new_lease_until <= existing.lease_until
            || !new_lease_until.is_live_at(now)
        {
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
                authority.attempt_id,
                observation_codec,
                observed_at,
                now,
                observation,
            );
        };
        effect.observations.push(ObservationRecord {
            id: observation_id,
            authority: authority.clone(),
            attempt_id: authority.attempt_id,
            observation_codec,
            observation,
            observed_at,
            recorded_at: now,
            authoritative: true,
        });
        if let Some(attempt) = effect
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == authority.attempt_id)
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
        if receipt_codec.family.is_empty()
            || receipt_event_codec.family.is_empty()
            || !self
                .binding
                .acceptance
                .supported_codecs
                .supports(&receipt_codec)
            || !self
                .binding
                .acceptance
                .supported_codecs
                .supports(&receipt_event_codec)
        {
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
        self.refresh_eligibility(now);
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
        let evidence = {
            let Some(effect) = self.effect_authorized_mut(authority, now, false) else {
                return stale_reconciliation_outcome();
            };
            effect.status = EffectStatus::AmbiguityWait;
            effect.authority = None;
            effect.reclaimable_lease = None;
            effect.pending_reconciliation = true;
            effect.observations.clone()
        };
        self.status = WorkflowStatus::ManualResolution;
        let resolution: ManualResolutionRecord<P> = ManualResolutionRecord {
            id: resolution_id,
            workflow_version,
            effect_id: authority.effect_id,
            status: ResolutionStatus::Required,
            evidence,
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
        let Some(existing_resolution) = self.validate_manual_resolution_request(
            resolution_id,
            expected_workflow_version,
            choice,
        ) else {
            return invalid_manual_resolution(None);
        };

        let mut staged = self.clone();
        let mut resolution = staged
            .manual_resolutions
            .get(&resolution_id)
            .cloned()
            .expect("resolution exists in staged clone");
        if staged.version != expected_workflow_version {
            return invalid_manual_resolution(None);
        }
        if staged
            .apply_manual_resolution_choice(&mut resolution, resolved_by, choice, commit)
            .is_none()
        {
            return invalid_manual_resolution(Some(existing_resolution));
        }

        staged
            .manual_resolutions
            .insert(resolution_id, resolution.clone());
        staged.finish_manual_resolution(commit);
        let effect_outcome = Some(staged.manual_effect_outcome(&resolution, choice.kind));
        *self = staged;
        ManualResolutionOutcome {
            outcome: CommitOutcome::Committed,
            resolution: Some(resolution),
            effect_outcome,
        }
    }

    fn validate_manual_resolution_request(
        &self,
        resolution_id: ManualResolutionId,
        expected_workflow_version: Version,
        choice: &ManualChoice<P>,
    ) -> Option<ManualResolutionRecord<P>> {
        if expected_workflow_version != self.version {
            return None;
        }
        let resolution = self.manual_resolutions.get(&resolution_id)?.clone();
        (resolution.status == ResolutionStatus::Required
            && resolution.permitted_choices.iter().any(|candidate| {
                candidate.kind == choice.kind
                    && candidate.codec == choice.codec
                    && candidate.payload == choice.payload
                    && candidate.receipt_codec == choice.receipt_codec
                    && candidate.receipt == choice.receipt
                    && candidate.receipt_event_codec == choice.receipt_event_codec
                    && candidate.receipt_event == choice.receipt_event
            }))
        .then_some(resolution)
    }

    fn apply_manual_resolution_choice(
        &mut self,
        resolution: &mut ManualResolutionRecord<P>,
        resolved_by: &'static str,
        choice: &ManualChoice<P>,
        commit: &ManualResolutionCommit<P>,
    ) -> Option<()> {
        let effect_id = resolution.effect_id;
        let (declared_workflow_version, effect_id) = {
            let effect = self.effects.get_mut(&effect_id)?;
            if effect.status != EffectStatus::AmbiguityWait {
                return None;
            }

            resolution.status = ResolutionStatus::Resolved;
            resolution.accepted_choice = Some(choice.clone());
            resolution.resolved_by = Some(resolved_by);
            match choice.kind {
                ManualChoiceKind::Retry => {
                    let retry_at = commit.retry_at?;
                    effect.status = EffectStatus::RetryWait;
                    effect.declaration.next_eligible_at = Some(retry_at);
                    return Some(());
                }
                ManualChoiceKind::Compensate | ManualChoiceKind::Suppress => {
                    effect.status = EffectStatus::Invalidated;
                    return Some(());
                }
                ManualChoiceKind::AcceptAsTerminal => {
                    effect.status = EffectStatus::Receipted;
                    (
                        effect.declared_workflow_version,
                        effect.declaration.effect_id,
                    )
                }
            }
        };

        self.install_manual_receipt(effect_id, declared_workflow_version, choice);
        Some(())
    }

    fn install_manual_receipt(
        &mut self,
        effect_id: EffectId,
        declared_workflow_version: Version,
        choice: &ManualChoice<P>,
    ) {
        let authority = manual_receipt_authority(
            self.binding.workflow_id,
            declared_workflow_version,
            self.generation,
            effect_id,
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
        self.effects
            .get_mut(&effect_id)
            .expect("effect exists")
            .receipt = Some(receipt);

        let requires_runtime_acceptance =
            P::receipt_requires_runtime_acceptance(&choice.receipt_event);
        let delivery: DeliveryItem<P> = DeliveryItem {
            id: DeliveryId(self.next_delivery_id),
            effect_id: Some(effect_id),
            barrier_id: None,
            consumer_kind: "reducer",
            event_codec: choice.receipt_event_codec.clone(),
            requires_runtime_acceptance,
            payload: DeliveryPayload::Receipt(choice.receipt_event.clone()),
            status: DeliveryStatus::Pending,
            runtime_acceptance_status: requires_runtime_acceptance
                .then_some(RuntimeAcceptanceStatus::Owed),
            suppression_reason: None,
            accepted_by: None,
        };
        self.next_delivery_id += 1;
        self.deliveries.insert(delivery.id, delivery);
    }

    fn finish_manual_resolution(&mut self, commit: &ManualResolutionCommit<P>) {
        let transition = WorkflowTransition {
            transition_id: TransitionId(self.next_transition_id),
            from_version: self.version,
            to_version: self.version.next(),
            generation: self.generation,
            event: commit.transition_event.clone(),
            event_codec: commit.transition_codec.clone(),
        };
        self.next_transition_id += 1;
        self.version = transition.to_version;
        self.status = commit.next_status;
        self.transition_log.push(transition);
        self.refresh_eligibility(Timestamp(0));
    }

    fn manual_effect_outcome(
        &self,
        resolution: &ManualResolutionRecord<P>,
        choice_kind: ManualChoiceKind,
    ) -> ManualEffectOutcome<P> {
        match choice_kind {
            ManualChoiceKind::Retry => ManualEffectOutcome::Retry,
            ManualChoiceKind::Compensate => ManualEffectOutcome::Compensate,
            ManualChoiceKind::Suppress => ManualEffectOutcome::Suppressed,
            ManualChoiceKind::AcceptAsTerminal => {
                let effect = self
                    .effects
                    .get(&resolution.effect_id)
                    .expect("effect exists");
                let receipt = effect.receipt.clone().expect("manual receipt installed");
                let reducer_event = self
                    .deliveries
                    .values()
                    .find(|delivery| delivery.effect_id == Some(resolution.effect_id))
                    .cloned()
                    .expect("manual reducer delivery installed");
                ManualEffectOutcome::Receipt {
                    receipt: Box::new(receipt),
                    reducer_event: Box::new(reducer_event),
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
        if binding.items.is_empty() {
            return Err(EngineError::InvalidPlan(PlanError::EmptyDeliveryBatch));
        }
        validate_status_transition(self.status, binding.decision.plan.next_status)
            .map_err(EngineError::InvalidPlan)?;
        validate_plan_body(
            &binding.decision.plan,
            &BTreeMap::new(),
            &self.binding.acceptance.supported_codecs,
        )
        .map_err(EngineError::InvalidPlan)?;
        self.validate_plan_collisions(&binding.decision.plan)?;
        let mut consumed_ids = Vec::new();
        for item in &binding.items {
            let Some(existing) = self.deliveries.get(&item.id) else {
                return Err(EngineError::InvalidInbox);
            };
            if !matches!(
                existing.status,
                DeliveryStatus::Pending | DeliveryStatus::Deferred
            ) || !P::decision_handles_delivery(existing, &binding.decision.plan.event)
            {
                return Err(EngineError::InvalidInbox);
            }
            consumed_ids.push(item.id);
        }
        let transition = self.begin_transition(&binding.decision);
        self.apply_invalidations(&binding.decision.plan.invalidations);
        self.install_effects(&binding.decision.plan);
        self.install_barriers(&binding.decision.plan, &BTreeMap::new())?;
        let mut items = Vec::new();
        for id in &consumed_ids {
            let item = self.deliveries.get_mut(id).expect("delivery exists");
            item.status = DeliveryStatus::Accepted;
            item.accepted_by = Some(transition.transition_id);
            items.push(item.clone());
        }
        self.install_declared_deliveries(&binding.decision.plan);
        self.install_schedules(&binding.decision.plan);
        self.refresh_eligibility(Timestamp(0));
        self.refresh_barriers();
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
        if item.runtime_acceptance_status != Some(RuntimeAcceptanceStatus::Owed)
            || !self.binding.acceptance.runtime_acceptance_enabled()
        {
            return Err(EngineError::InvalidInbox);
        }
        validate_status_transition(self.status, decision.plan.next_status)
            .map_err(EngineError::InvalidPlan)?;
        validate_plan_body(
            &decision.plan,
            &BTreeMap::new(),
            &self.binding.acceptance.supported_codecs,
        )
        .map_err(EngineError::InvalidPlan)?;
        self.validate_plan_collisions(&decision.plan)?;
        let predicate = if suppress {
            P::decision_handles_runtime_suppression(&item, &decision.plan.event)
        } else {
            P::decision_handles_runtime_acceptance(&item, &decision.plan.event)
        };
        if !predicate {
            return Err(EngineError::InvalidInbox);
        }
        let transition = self.begin_transition(decision);
        self.apply_invalidations(&decision.plan.invalidations);
        self.install_effects(&decision.plan);
        self.install_barriers(&decision.plan, &BTreeMap::new())?;
        let accepted_item = {
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
            } else {
                item.status = DeliveryStatus::Accepted;
            }
            item.accepted_by = Some(transition.transition_id);
            item.clone()
        };
        self.install_declared_deliveries(&decision.plan);
        self.install_schedules(&decision.plan);
        self.refresh_eligibility(Timestamp(0));
        self.refresh_barriers();
        Ok(RuntimeAcceptanceResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            delivery: Some(accepted_item),
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
        if matches!(
            self.status,
            WorkflowStatus::Completed
                | WorkflowStatus::Cancelled
                | WorkflowStatus::Failed
                | WorkflowStatus::Incompatible
                | WorkflowStatus::Deleted
                | WorkflowStatus::DeletionPending
        ) {
            return Err(EngineError::InvalidPlan(
                PlanError::InvalidStatusTransition {
                    current: self.status,
                    next: WorkflowStatus::Cancelling,
                },
            ));
        }
        let mut staged = self.clone();
        staged.generation = staged.generation.next();
        staged.status = WorkflowStatus::Cancelling;
        if let Some(terminal_receipt) = &request.terminal_receipt {
            staged.install_cancellation_terminal_receipt(terminal_receipt)?;
        }
        staged.apply_invalidations(&request.invalidations);
        let result = staged.commit_transition(
            &ReducerDecision {
                expected_workflow_version: staged.version,
                plan: request.compensation_plan.clone(),
            },
            barrier_events,
        )?;
        *self = staged;
        Ok(result)
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
                && effect.receipt.is_none()
                && effect
                    .declaration
                    .next_eligible_at
                    .is_none_or(|deadline| deadline <= now)
                && if effect.dependencies.is_empty() {
                    effect.declaration.next_eligible_at.is_some()
                } else {
                    effect
                        .dependencies
                        .iter()
                        .all(|dependency_id| receipted.contains(dependency_id))
                }
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
        self.refresh_barriers();
        let mut due_schedule_ids = Vec::new();
        for (schedule_id, schedule) in &self.schedules {
            if schedule.status == ScheduleStatus::Idle && schedule.next_eligible_at <= now {
                due_schedule_ids.push(*schedule_id);
            }
        }
        for schedule_id in due_schedule_ids {
            let _ = self.reconcile_schedule_due(schedule_id, now);
        }
    }

    pub fn reconcile_schedule_due(
        &mut self,
        schedule_id: ScheduleId,
        now: Timestamp,
    ) -> Option<ScheduleOccurrence> {
        let schedule = self.schedules.get_mut(&schedule_id)?;
        if schedule.policy != SchedulePolicy::CoalesceLatest
            || schedule.status != ScheduleStatus::Idle
            || schedule.next_eligible_at > now
        {
            return None;
        }
        let occurrence = ScheduleOccurrence {
            schedule_id,
            occurrence_id: ScheduleOccurrenceId(self.next_schedule_occurrence_id),
            generation: self.generation,
            due_at: schedule.next_eligible_at,
        };
        self.next_schedule_occurrence_id += 1;
        schedule.status = ScheduleStatus::Due;
        schedule.due_occurrence = Some(occurrence);
        schedule.active_occurrence = None;
        Some(occurrence)
    }

    pub fn start_schedule_occurrence(
        &mut self,
        occurrence: ScheduleOccurrence,
        active_effect_id: Option<EffectId>,
    ) -> Option<ScheduleState> {
        let schedule = self.schedules.get_mut(&occurrence.schedule_id)?;
        if schedule.status != ScheduleStatus::Due || schedule.due_occurrence != Some(occurrence) {
            return None;
        }
        schedule.status = ScheduleStatus::Active;
        schedule.active_effect_id = active_effect_id;
        schedule.active_occurrence = Some(occurrence);
        schedule.due_occurrence = None;
        Some(schedule.clone())
    }

    pub fn complete_schedule_occurrence(
        &mut self,
        occurrence: ScheduleOccurrence,
        next_eligible_at: Timestamp,
    ) -> Option<ScheduleState> {
        let schedule = self.schedules.get_mut(&occurrence.schedule_id)?;
        if schedule.status != ScheduleStatus::Active
            || schedule.active_occurrence != Some(occurrence)
        {
            return None;
        }
        schedule.status = ScheduleStatus::Idle;
        schedule.active_effect_id = None;
        schedule.active_occurrence = None;
        schedule.due_occurrence = None;
        schedule.next_eligible_at = next_eligible_at;
        Some(schedule.clone())
    }

    fn validate_plan_collisions(&self, plan: &TransitionPlan<P>) -> Result<(), EngineError> {
        for effect in &plan.effects {
            if self.effects.contains_key(&effect.effect_id) {
                return Err(EngineError::InvalidPlan(PlanError::EffectIdCollision(
                    effect.effect_id,
                )));
            }
        }
        for barrier in &plan.barriers {
            if self.barriers.contains_key(&barrier.barrier_id) {
                return Err(EngineError::InvalidPlan(PlanError::BarrierIdCollision(
                    barrier.barrier_id,
                )));
            }
        }
        for schedule in &plan.schedules {
            if self.schedules.contains_key(&schedule.schedule_id)
                || self
                    .schedules
                    .values()
                    .any(|existing| existing.key == schedule.key)
            {
                return Err(EngineError::InvalidPlan(PlanError::ScheduleIdCollision(
                    schedule.schedule_id,
                )));
            }
        }
        Ok(())
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
        let mut pending: Vec<EffectId> = invalidations.iter().map(|item| item.effect_id).collect();
        let mut targets = BTreeSet::new();
        while let Some(effect_id) = pending.pop() {
            if !targets.insert(effect_id) {
                continue;
            }
            for (dependent_id, effect) in &self.effects {
                if effect.declaration.generation == self.generation
                    && effect.dependencies.contains(&effect_id)
                {
                    pending.push(*dependent_id);
                }
            }
        }
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
            let has_dependencies = plan
                .dependencies
                .iter()
                .any(|dependency| dependency.effect_id == declaration.effect_id);
            let status = if declaration.generation == self.generation
                && declaration.next_eligible_at.is_none()
                && !has_dependencies
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

    fn install_cancellation_terminal_receipt(
        &mut self,
        terminal_receipt: &CancellationReceiptDecl<P>,
    ) -> Result<(), EngineError> {
        if terminal_receipt.receipt_codec.family.is_empty()
            || terminal_receipt.event_codec.family.is_empty()
        {
            return Err(EngineError::InvalidPlan(PlanError::MissingCodec("receipt")));
        }
        let effect =
            self.effects
                .get_mut(&terminal_receipt.effect_id)
                .ok_or(EngineError::InvalidPlan(PlanError::UnknownEffectReference(
                    terminal_receipt.effect_id,
                )))?;
        let authority = effect
            .receipt
            .as_ref()
            .map(|receipt| receipt.authority.clone())
            .or_else(|| effect.authority.clone())
            .ok_or(EngineError::InvalidPlan(PlanError::UnknownEffectReference(
                terminal_receipt.effect_id,
            )))?;
        if effect.receipt.is_none() {
            let receipt = ReceiptRecord {
                id: ReceiptId(self.next_receipt_id),
                authority: authority.clone(),
                attempt_id: Some(authority.attempt_id),
                origin: ReceiptOrigin::CancellationArbitration,
                receipt_codec: terminal_receipt.receipt_codec.clone(),
                receipt: terminal_receipt.receipt.clone(),
                generation: self.generation,
            };
            self.next_receipt_id += 1;
            effect.receipt = Some(receipt);
        }
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
        let requires_runtime_acceptance =
            P::receipt_requires_runtime_acceptance(&terminal_receipt.event);
        let delivery = DeliveryItem {
            id: DeliveryId(self.next_delivery_id),
            effect_id: Some(terminal_receipt.effect_id),
            barrier_id: None,
            consumer_kind: "reducer",
            event_codec: terminal_receipt.event_codec.clone(),
            requires_runtime_acceptance,
            payload: DeliveryPayload::Receipt(terminal_receipt.event.clone()),
            status: DeliveryStatus::Pending,
            runtime_acceptance_status: requires_runtime_acceptance
                .then_some(RuntimeAcceptanceStatus::Owed),
            suppression_reason: None,
            accepted_by: None,
        };
        self.next_delivery_id += 1;
        self.deliveries.insert(delivery.id, delivery);
        Ok(())
    }

    fn refresh_barriers(&mut self) {
        for barrier in self.barriers.values_mut() {
            if barrier.status == BarrierStatus::Waiting
                && barrier.required_members.iter().all(|(effect_id, family)| {
                    self.effects.get(effect_id).is_some_and(|effect| {
                        effect.receipt.as_ref().is_some_and(|receipt| {
                            receipt.generation == self.generation
                                && match family {
                                    ReceiptFamily::CurrentGenerationEffect => {
                                        effect.declaration.role == EffectRole::Required
                                            && effect.declaration.generation == self.generation
                                    }
                                    ReceiptFamily::CompensationEffect => {
                                        effect.declaration.role == EffectRole::Compensation
                                    }
                                }
                        })
                    })
                })
            {
                barrier.status = BarrierStatus::Satisfied;
                let item = DeliveryItem {
                    id: DeliveryId(self.next_delivery_id),
                    effect_id: None,
                    barrier_id: Some(barrier.barrier_id),
                    consumer_kind: "reducer",
                    event_codec: barrier.reducer_event_codec.clone(),
                    requires_runtime_acceptance: false,
                    payload: DeliveryPayload::Barrier(barrier.reducer_event.clone()),
                    status: DeliveryStatus::Pending,
                    runtime_acceptance_status: None,
                    suppression_reason: None,
                    accepted_by: None,
                };
                self.next_delivery_id += 1;
                self.deliveries.insert(item.id, item);
            }
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
                requires_runtime_acceptance: declaration.requires_runtime_acceptance(),
                payload: declaration.payload.clone(),
                status: DeliveryStatus::Pending,
                runtime_acceptance_status: declaration
                    .requires_runtime_acceptance()
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
                    due_occurrence: None,
                    active_occurrence: None,
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
        if matches!(
            self.status,
            WorkflowStatus::ManualResolution
                | WorkflowStatus::Incompatible
                | WorkflowStatus::Completed
                | WorkflowStatus::Cancelled
                | WorkflowStatus::Deleted
                | WorkflowStatus::Failed
        ) || authority.workflow_id != self.binding.workflow_id
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
        let attempt = effect
            .attempts
            .iter()
            .find(|attempt| attempt.id == authority.attempt_id)?;
        (attempt.status == AttemptStatus::Begun
            || attempt.status == AttemptStatus::ObservationRecorded)
            .then_some(effect)
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
