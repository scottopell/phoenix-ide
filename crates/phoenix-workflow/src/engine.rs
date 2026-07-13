use std::collections::{BTreeMap, BTreeSet};

use crate::validation::{validate_plan_body, validate_status_transition};
use crate::{
    declared_receipt_family, validate_plan, AtomicInboxConsumeResult, AttemptId, AttemptRecord,
    AttemptStatus, AuthoritativeWorkflow, AuthorityOutcome, BarrierEvaluation, BarrierId,
    BarrierState, BarrierStatus, CancellationRequest, ClaimAuthority, ClaimOutcome, ClaimResult,
    CodecRef, CommitOutcome, CommitResult, DeliveryStatus, DivergenceAction,
    DivergenceResolutionAction, DivergenceSeverity, EffectDecl, EffectId, EffectInvalidationDecl,
    EffectState, EffectStatus, EngineError, ExecutionMode, Generation, InboxDecisionBinding,
    LeaseExpiry, ManualChoice, ManualChoiceKind, ManualEffectOutcome, ManualResolutionCommit,
    ManualResolutionId, ManualResolutionOutcome, ManualResolutionRecord, ObservationId,
    ObservationRecord, OwedAcceptanceDecisionBinding, OwedAcceptanceDisposition, OwedAcceptanceId,
    OwedAcceptanceRecord, PlanError, ProfileRef, ProtocolSelection, ReceiptAcceptance,
    ReceiptFamily, ReceiptId, ReceiptOrigin, ReceiptRecord, ReconciliationDecision,
    ReconciliationOutcome, ReducerDecision, ReducerInboxEvent, ReducerInboxId, ReducerInboxKind,
    ReducerInboxPayload, RenewalResult, ResolutionStatus, ResourceLockGrant,
    RuntimeAcceptanceResult, SemanticAuthority, ShadowComparisonEvidence, ShadowDivergenceId,
    ShadowDivergenceKind, ShadowDivergenceRecord, ShadowDivergenceResolution, ShadowWorkflow,
    StaleObservationRecord, SuppressionReason, Timestamp, TransitionId, TransitionPlan, Version,
    WorkflowBinding, WorkflowId, WorkflowProfile, WorkflowState, WorkflowStatus,
    WorkflowTransition,
};

impl<P: WorkflowProfile> WorkflowState<P> {
    /// Creates an authoritative workflow under an open protocol selection.
    ///
    /// # Errors
    /// Returns [`EngineError::ProtocolNotAccepting`] when admission is closed.
    pub fn new_authoritative(
        workflow_id: WorkflowId,
        profile: &ProfileRef,
        accepted_protocol: &ProtocolSelection,
        snapshot_codec: CodecRef,
        snapshot: P::Snapshot,
    ) -> Result<Self, EngineError> {
        if !accepted_protocol.accepting {
            return Err(EngineError::ProtocolNotAccepting);
        }
        if *profile != accepted_protocol.profile {
            return Err(EngineError::ProfileProtocolMismatch);
        }
        if snapshot_codec.family.is_empty() {
            return Err(EngineError::InvalidPlan(PlanError::MissingCodec(
                "snapshot",
            )));
        }
        Ok(Self {
            binding: WorkflowBinding::Authoritative(AuthoritativeWorkflow {
                workflow_id,
                version: Version(0),
                generation: Generation(0),
                profile: profile.clone(),
                accepted_protocol: accepted_protocol.clone(),
            }),
            semantic_authority: Some(accepted_protocol.authority),
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
        })
    }

    /// Creates a non-authoritative shadow workflow under an open selection.
    ///
    /// # Errors
    /// Returns [`EngineError::ProtocolNotAccepting`] when admission is closed.
    pub fn new_shadow(
        workflow_id: WorkflowId,
        authoritative_workflow_id: WorkflowId,
        profile: &ProfileRef,
        accepted_protocol: &ProtocolSelection,
        snapshot_codec: CodecRef,
        snapshot: P::Snapshot,
    ) -> Result<Self, EngineError> {
        if !accepted_protocol.accepting {
            return Err(EngineError::ProtocolNotAccepting);
        }
        if *profile != accepted_protocol.profile {
            return Err(EngineError::ProfileProtocolMismatch);
        }
        if snapshot_codec.family.is_empty() {
            return Err(EngineError::InvalidPlan(PlanError::MissingCodec(
                "snapshot",
            )));
        }
        Ok(Self {
            binding: WorkflowBinding::Shadow(ShadowWorkflow {
                workflow_id,
                authoritative_workflow_id,
                profile: profile.clone(),
                accepted_protocol: accepted_protocol.clone(),
            }),
            semantic_authority: None,
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
        })
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
        if decision.expected_workflow_version != self.version {
            return Ok(version_conflict_result());
        }
        self.validate_plan_against_state(&decision.plan, barrier_events)?;

        let mut replacement = self.clone();
        let transition = replacement.begin_transition(decision);
        replacement.apply_invalidations(&decision.plan.invalidations);
        replacement.install_effects(&decision.plan);
        replacement.install_barriers(&decision.plan, barrier_events)?;
        if decision.plan.owed_acceptances.is_some() {
            return Err(EngineError::InvalidInbox);
        }
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
        if workflow_status_is_terminal(self.status) {
            return ineligible_claim();
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
        effect.destructive_lock = resource_lock;
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
    ) -> RenewalResult {
        if !new_lease_until.is_live_at(now) {
            return stale_renewal();
        }
        match self.effect_authorized_mut(authority, now) {
            Some(effect) => {
                let Some(claim) = &mut effect.claim else {
                    return stale_renewal();
                };
                if new_lease_until <= claim.lease_until {
                    return stale_renewal();
                }
                claim.lease_until = new_lease_until;
                if let Some(lock) = &mut effect.destructive_lock {
                    lock.lease_until = new_lease_until;
                }
                if let Some(lock) = &mut claim.resource_lock {
                    lock.lease_until = new_lease_until;
                }
                if let Some(attempt) = effect
                    .attempts
                    .iter_mut()
                    .find(|attempt| same_claim_identity(&attempt.authority, authority))
                {
                    attempt.authority = claim.clone();
                }
                RenewalResult {
                    outcome: AuthorityOutcome::Authorized,
                    authority: Some(claim.clone()),
                }
            }
            None => stale_renewal(),
        }
    }

    pub fn take_over_expired_claim(
        &mut self,
        effect_id: EffectId,
        expired_claim: &ClaimAuthority,
        worker_id: &'static str,
        now: Timestamp,
        lease_until: LeaseExpiry,
    ) -> ClaimResult {
        if workflow_status_is_terminal(self.status) {
            return ineligible_claim();
        }
        if !lease_until.is_live_at(now) || self.crashed_workers.contains(&worker_id) {
            return denied_claim();
        }
        let Some(effect) = self.effects.get(&effect_id) else {
            return ineligible_claim();
        };
        let Some(live_claim) = effect.claim.as_ref() else {
            return ineligible_claim();
        };
        if live_claim != expired_claim || expired_claim.lease_until.is_live_at(now) {
            return denied_claim();
        }
        let Ok(ordinal_base) = u32::try_from(effect.attempts.len()) else {
            return ineligible_claim();
        };
        let Some(ordinal) = ordinal_base.checked_add(1) else {
            return ineligible_claim();
        };
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
        let Some(effect) = self.effects.get_mut(&effect_id) else {
            return ineligible_claim();
        };
        effect.destructive_lock = resource_lock;
        effect.claim = Some(authority.clone());
        effect.status = EffectStatus::Claimed;
        effect.pending_reconciliation = true;
        if let Some(last_attempt) = effect.attempts.last_mut() {
            last_attempt.status = AttemptStatus::AuthorityLost;
        }
        let attempt = AttemptRecord {
            id: AttemptId(self.next_attempt_id),
            ordinal,
            authority: authority.clone(),
            status: AttemptStatus::Begun,
        };
        self.next_attempt_id += 1;
        self.next_claim_token += 1;
        effect.attempts.push(attempt.clone());
        ClaimResult {
            outcome: ClaimOutcome::Claimed,
            authority: Some(authority),
            attempt: Some(attempt),
        }
    }

    pub fn record_observation(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
        attempt_id: AttemptId,
        observation_codec: CodecRef,
        observation: P::Observation,
    ) -> AuthorityOutcome {
        if observation_codec.family.is_empty() {
            return AuthorityOutcome::StaleAuthority;
        }
        let observation_id = ObservationId(self.next_observation_id);
        if let Some(effect) = self.effect_authorized_mut(authority, now) {
            if !effect.attempts.iter().any(|attempt| {
                attempt.id == attempt_id && same_claim_identity(&attempt.authority, authority)
            }) {
                return AuthorityOutcome::StaleAuthority;
            }
            effect.observations.push(ObservationRecord {
                id: observation_id,
                authority: authority.clone(),
                attempt_id,
                observation_codec,
                observation,
                authoritative: true,
            });
            if let Some(attempt) = effect
                .attempts
                .iter_mut()
                .find(|attempt| attempt.id == attempt_id)
            {
                attempt.status = AttemptStatus::ObservationRecorded;
            }
            self.next_observation_id += 1;
            return AuthorityOutcome::Authorized;
        }
        if authority.workflow_id == self.binding.workflow_id() {
            if let Some(effect) = self.effects.get_mut(&authority.effect_id) {
                self.next_observation_id += 1;
                effect.stale_observations.push(StaleObservationRecord {
                    id: observation_id,
                    authority: authority.clone(),
                    attempt_id,
                    observation_codec,
                    observation,
                });
            }
        }
        AuthorityOutcome::StaleAuthority
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    /// Accepts a receipt produced under live claimed-worker authority.
    ///
    /// Manual-origin receipts are structurally reserved for [`Self::resolve_manual`]
    /// and are rejected on this claimed-worker path.
    pub fn accept_receipt(
        &mut self,
        authority: &ClaimAuthority,
        now: Timestamp,
        attempt_id: Option<AttemptId>,
        origin: ReceiptOrigin,
        receipt_codec: CodecRef,
        receipt: P::Receipt,
        receipt_event_codec: CodecRef,
        receipt_event: P::ReceiptReducerEvent,
    ) -> ReceiptAcceptance<P> {
        if receipt_codec.family.is_empty() || receipt_event_codec.family.is_empty() {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                receipt_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            };
        }
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
        let Some(attempt_id) = attempt_id else {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                receipt_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            };
        };
        if !effect.attempts.iter().any(|attempt| {
            attempt.id == attempt_id && same_claim_identity(&attempt.authority, authority)
        }) {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                receipt_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            };
        }
        if !matches!(
            origin,
            ReceiptOrigin::Execution | ReceiptOrigin::Adoption | ReceiptOrigin::Reconciliation
        ) || (effect.pending_reconciliation && origin == ReceiptOrigin::Execution)
        {
            return ReceiptAcceptance {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                receipt_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            };
        }
        let receipt_record = ReceiptRecord {
            id: receipt_id,
            authority: authority.clone(),
            attempt_id: Some(attempt_id),
            origin,
            receipt_codec,
            receipt,
            generation,
        };
        effect.receipt = Some(receipt_record.clone());
        effect.claim = None;
        effect.destructive_lock = None;
        effect.status = EffectStatus::Receipted;
        effect.pending_reconciliation = false;
        if let Some(attempt) = effect
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
        {
            attempt.status = AttemptStatus::ReceiptAccepted;
        }

        let inbox_id = ReducerInboxId(self.next_inbox_id);
        self.next_inbox_id += 1;
        let reducer_event = ReducerInboxEvent {
            id: inbox_id,
            effect_id: Some(authority.effect_id),
            barrier_id: None,
            kind: ReducerInboxKind::ReceiptAccepted,
            event_codec: receipt_event_codec,
            requires_runtime_acceptance: P::receipt_requires_runtime_acceptance(&receipt_event),
            payload: ReducerInboxPayload::Receipt(receipt_event),
            delivery_status: if matches!(
                self.status,
                WorkflowStatus::Cancelled | WorkflowStatus::Completed | WorkflowStatus::Failed
            ) {
                DeliveryStatus::Suppressed {
                    reason: SuppressionReason::LifecycleTerminal,
                }
            } else {
                DeliveryStatus::Pending
            },
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
        if self.ensure_executable().is_err() {
            return stale_reconciliation_outcome();
        }
        match self.effect_authorized_mut(authority, now) {
            Some(effect) => {
                if effect.declaration.ambiguity.is_manual_only() {
                    return ReconciliationOutcome {
                        outcome: AuthorityOutcome::Authorized,
                        decision: Some(ReconciliationDecision::RequestManualResolution),
                        manual_resolution: None,
                    };
                }
                effect.status = EffectStatus::RetryWait;
                effect.declaration.next_eligible_at = Some(retry_at);
                effect.claim = None;
                effect.destructive_lock = None;
                effect.pending_reconciliation = false;
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
        if self.ensure_executable().is_err()
            || permitted_choices.is_empty()
            || permitted_choices.iter().any(|choice| {
                choice.codec.family.is_empty()
                    || choice.receipt_codec.family.is_empty()
                    || choice.receipt_event_codec.family.is_empty()
            })
        {
            return stale_reconciliation_outcome();
        }
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
        if let Some(lock) = &mut effect.destructive_lock {
            lock.lease_until = LeaseExpiry(u64::MAX);
        }
        let resolution = ManualResolutionRecord {
            id: resolution_id,
            workflow_version,
            effect_id: authority.effect_id,
            status: ResolutionStatus::Required,
            evidence: effect.observations.clone(),
            permitted_choices,
            accepted_choice: None,
            resolved_by: None,
        };
        effect.pending_reconciliation = true;
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
        resolved_by: &'static str,
        choice: &ManualChoice<P>,
        commit: ManualResolutionCommit<P>,
    ) -> ManualResolutionOutcome<P> {
        if self.ensure_executable().is_err() {
            return invalid_manual_resolution(None);
        }
        let Some(existing) = self
            .manual_resolutions
            .get(&resolution_id)
            .map(clone_manual_resolution)
        else {
            return invalid_manual_resolution(None);
        };
        if existing.status != ResolutionStatus::Required
            || self.version != expected_workflow_version
        {
            return version_conflict_manual_resolution(existing);
        }
        if !manual_choice_permitted(&existing, choice) {
            return invalid_manual_resolution(Some(existing));
        }
        if !self.effect_ready_for_manual_resolution(&existing)
            || commit.transition_codec.family.is_empty()
            || choice.receipt_codec.family.is_empty()
            || choice.receipt_event_codec.family.is_empty()
            || validate_status_transition(self.status, commit.next_status).is_err()
        {
            return invalid_manual_resolution(Some(existing));
        }

        let mut replacement = self.clone();
        let effect_outcome = replacement.apply_manual_resolution(
            resolution_id,
            resolved_by,
            &existing,
            choice,
            commit,
        );
        if matches!(effect_outcome, ManualEffectOutcome::Receipt { .. }) {
            let _ = replacement.evaluate_barriers();
        }
        let resolution = replacement
            .manual_resolutions
            .get(&resolution_id)
            .map(clone_manual_resolution);
        *self = replacement;

        ManualResolutionOutcome {
            outcome: CommitOutcome::Committed,
            resolution,
            effect_outcome: Some(effect_outcome),
        }
    }

    /// Commits a reducer decision and consumes the exact inbox items in one state replacement.
    ///
    /// # Errors
    /// Returns an engine error when the inbox set is invalid or the reducer decision cannot commit.
    pub fn consume_reducer_inbox_atomically(
        &mut self,
        binding: &InboxDecisionBinding<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<AtomicInboxConsumeResult<P>, EngineError> {
        self.ensure_executable()?;
        let decision = &binding.decision;
        if decision.expected_workflow_version != self.version {
            return Ok(AtomicInboxConsumeResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                consumed_inbox_ids: Vec::new(),
                reducer_events: Vec::new(),
            });
        }
        self.validate_plan_against_state(&decision.plan, barrier_events)?;
        let inbox_ids = binding
            .inbox
            .iter()
            .map(|inbox| inbox.id)
            .collect::<Vec<_>>();
        if inbox_ids.iter().copied().collect::<BTreeSet<_>>().len() != inbox_ids.len() {
            return Err(EngineError::InvalidInbox);
        }
        let mut replacement = self.clone();
        for linked_inbox in &binding.inbox {
            let Some(inbox) = replacement.reducer_inbox.get(&linked_inbox.id) else {
                return Ok(AtomicInboxConsumeResult {
                    outcome: CommitOutcome::InvalidPlan,
                    transition: None,
                    consumed_inbox_ids: Vec::new(),
                    reducer_events: Vec::new(),
                });
            };
            if !same_inbox_event(inbox, linked_inbox)
                || inbox.delivery_status != DeliveryStatus::Pending
                || !P::decision_handles_inbox(&inbox.payload, &decision.plan.event)
            {
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
        if workflow_status_is_terminal(replacement.status)
            && decision.plan.owed_acceptances.is_some()
        {
            return Err(EngineError::InvalidInbox);
        }
        replacement.install_owed_acceptances(&decision.plan, &inbox_ids)?;
        for inbox_id in &inbox_ids {
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
            consumed_inbox_ids: inbox_ids,
            reducer_events: Vec::new(),
        })
    }

    /// Commits product runtime state and accepts one owed obligation atomically.
    ///
    /// # Errors
    /// Returns an engine error when the obligation or reducer decision is invalid.
    pub fn runtime_accept_atomically(
        &mut self,
        binding: &OwedAcceptanceDecisionBinding<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        self.runtime_acceptance_atomically(
            binding,
            barrier_events,
            OwedAcceptanceDisposition::Accepted {
                transition: TransitionId(0),
            },
        )
    }

    /// Commits product runtime state and suppresses one owed obligation atomically.
    ///
    /// # Errors
    /// Returns an engine error when the obligation or reducer decision is invalid.
    pub fn suppress_runtime_acceptance_atomically(
        &mut self,
        binding: &OwedAcceptanceDecisionBinding<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
        reason: SuppressionReason,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        self.runtime_acceptance_atomically(
            binding,
            barrier_events,
            OwedAcceptanceDisposition::Suppressed {
                transition: TransitionId(0),
                reason,
            },
        )
    }

    #[must_use]
    pub fn evaluate_barriers(&mut self) -> BarrierEvaluation<P> {
        let mut newly_satisfied = Vec::new();
        let mut reducer_events = Vec::new();
        let barrier_ids: Vec<BarrierId> = self.barriers.keys().copied().collect();
        for barrier_id in barrier_ids {
            let Some((
                barrier_status,
                required_members,
                reducer_event_codec,
                reducer_event_payload,
            )) = self.barriers.get(&barrier_id).map(|barrier| {
                (
                    barrier.status,
                    barrier.required_members.clone(),
                    barrier.reducer_event_codec.clone(),
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
                    event_codec: reducer_event_codec,
                    requires_runtime_acceptance: false,
                    payload: ReducerInboxPayload::Barrier(reducer_event_payload),
                    delivery_status: if workflow_status_is_terminal(self.status) {
                        DeliveryStatus::Suppressed {
                            reason: SuppressionReason::LifecycleTerminal,
                        }
                    } else {
                        DeliveryStatus::Pending
                    },
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
        if request.expected_workflow_version != self.version {
            return Ok(version_conflict_result());
        }
        let deletion_from_terminal = matches!(
            self.status,
            WorkflowStatus::Failed | WorkflowStatus::Cancelled
        ) && request.compensation_plan.next_status
            == WorkflowStatus::DeletionPending;
        if self.status != WorkflowStatus::Active && !deletion_from_terminal {
            return Ok(CommitResult {
                outcome: CommitOutcome::InvalidPlan,
                transition: None,
                reducer_events: Vec::new(),
            });
        }
        self.validate_cancellation_plan_against_state(&request.compensation_plan, barrier_events)?;
        if request.next_snapshot_codec.family.is_empty()
            || request.event_codec.family.is_empty()
            || request
                .reducer_inbox_events
                .iter()
                .any(|event| event.event_codec.family.is_empty())
        {
            return Err(EngineError::InvalidPlan(PlanError::MissingCodec(
                "cancellation",
            )));
        }

        let mut replacement = self.clone();
        replacement.enter_cancellation(request);
        let decision = replacement.cancellation_decision(request);
        validate_status_transition(replacement.status, decision.plan.next_status)
            .map_err(EngineError::InvalidPlan)?;
        let transition = replacement.begin_transition(&decision);
        replacement.suppress_cancellation_work(transition.transition_id);
        replacement.apply_invalidations(&decision.plan.invalidations);
        replacement.install_effects(&decision.plan);
        let reducer_events =
            replacement.materialize_cancellation_inbox_events(&request.reducer_inbox_events);
        replacement.install_barriers(&decision.plan, barrier_events)?;
        if decision.plan.owed_acceptances.is_some() {
            return Err(EngineError::InvalidInbox);
        }
        replacement.refresh_eligibility(Timestamp(0));
        *self = replacement;

        Ok(CommitResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            reducer_events,
        })
    }

    pub fn record_shadow_divergence(
        &mut self,
        kind: ShadowDivergenceKind,
        evidence_identity: String,
        evidence: ShadowComparisonEvidence,
    ) {
        let authoritative_workflow_id = match &self.binding {
            WorkflowBinding::Shadow(workflow) => workflow.authoritative_workflow_id,
            WorkflowBinding::Authoritative(_) => return,
        };
        if self.binding.execution_mode() != ExecutionMode::Shadow {
            return;
        }
        let severity = match kind {
            ShadowDivergenceKind::Snapshot
            | ShadowDivergenceKind::Transition
            | ShadowDivergenceKind::EffectPlan
            | ShadowDivergenceKind::Observation
            | ShadowDivergenceKind::Receipt
            | ShadowDivergenceKind::ReducerEvent
            | ShadowDivergenceKind::Capability
            | ShadowDivergenceKind::UserProjection => DivergenceSeverity::Blocking,
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
            resolution: ShadowDivergenceResolution::Unresolved,
            evidence_identity,
            profile_detail_kind: evidence.profile_detail_kind,
            expected_codec: evidence.expected_codec,
            expected_payload: evidence.expected_payload,
            actual_codec: evidence.actual_codec,
            actual_payload: evidence.actual_payload,
        });
    }

    pub fn resolve_shadow_divergence(
        &mut self,
        divergence_id: ShadowDivergenceId,
        action: DivergenceResolutionAction,
        resolved_by: &'static str,
    ) -> bool {
        let Some(divergence) = self
            .shadow_divergences
            .iter_mut()
            .find(|divergence| divergence.id == divergence_id)
        else {
            return false;
        };
        if matches!(
            divergence.resolution,
            ShadowDivergenceResolution::Resolved { .. }
        ) {
            return false;
        }
        divergence.resolution = ShadowDivergenceResolution::Resolved {
            action,
            resolved_by,
        };
        true
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
        if self.binding.execution_mode() == ExecutionMode::Shadow
            || self.semantic_authority != Some(SemanticAuthority::EngineProtocol)
        {
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
            event_codec: decision.plan.event_codec.clone(),
        };
        self.next_transition_id += 1;
        self.version = next_version;
        self.status = decision.plan.next_status;
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
                if matches!(
                    effect.status,
                    EffectStatus::Blocked
                        | EffectStatus::Eligible
                        | EffectStatus::Claimed
                        | EffectStatus::RetryWait
                        | EffectStatus::AmbiguityWait
                ) {
                    effect.status = EffectStatus::Invalidated;
                }
                effect.claim = None;
                effect.destructive_lock = None;
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
                    reducer_event_codec: barrier.reducer_event_codec.clone(),
                    reducer_event,
                },
            );
        }
        Ok(())
    }

    fn install_owed_acceptances(
        &mut self,
        plan: &TransitionPlan<P>,
        consumed_inbox_ids: &[ReducerInboxId],
    ) -> Result<(), EngineError> {
        if !self.binding.accepted_protocol().runtime_acceptance_enabled {
            if plan.owed_acceptances.is_some()
                || consumed_inbox_ids.iter().any(|inbox_id| {
                    self.reducer_inbox
                        .get(inbox_id)
                        .is_some_and(|event| event.requires_runtime_acceptance)
                })
            {
                return Err(EngineError::InvalidInbox);
            }
            return Ok(());
        }
        let runtime_acceptance_inbox_ids = consumed_inbox_ids
            .iter()
            .copied()
            .filter(|inbox_id| {
                self.reducer_inbox
                    .get(inbox_id)
                    .is_some_and(|event| event.requires_runtime_acceptance)
            })
            .collect::<Vec<_>>();
        let Some(owed_acceptances) = &plan.owed_acceptances else {
            if runtime_acceptance_inbox_ids.is_empty() {
                return Ok(());
            }
            return Err(EngineError::InvalidInbox);
        };
        if owed_acceptances.len() != runtime_acceptance_inbox_ids.len() {
            return Err(EngineError::InvalidInbox);
        }
        let consumed: BTreeSet<ReducerInboxId> =
            runtime_acceptance_inbox_ids.iter().copied().collect();
        if consumed.len() != runtime_acceptance_inbox_ids.len() {
            return Err(EngineError::InvalidInbox);
        }
        let declared: BTreeSet<ReducerInboxId> = owed_acceptances
            .iter()
            .map(|owed| owed.reducer_inbox_id)
            .collect();
        if declared.len() != owed_acceptances.len() || declared != consumed {
            return Err(EngineError::InvalidInbox);
        }
        for owed in owed_acceptances {
            let inbox = self
                .reducer_inbox
                .get(&owed.reducer_inbox_id)
                .ok_or(EngineError::InvalidInbox)?;
            if owed.event_codec != inbox.event_codec
                || !P::owed_acceptance_matches_inbox(&owed.event, &inbox.payload)
            {
                return Err(EngineError::InvalidInbox);
            }
            let id = OwedAcceptanceId(self.next_owed_acceptance_id);
            self.next_owed_acceptance_id += 1;
            self.owed_acceptances.insert(
                id,
                OwedAcceptanceRecord {
                    id,
                    reducer_inbox_id: owed.reducer_inbox_id,
                    source_kind: owed.source_kind,
                    event_codec: owed.event_codec.clone(),
                    event: owed.event.clone(),
                    disposition: OwedAcceptanceDisposition::Owed,
                },
            );
        }
        Ok(())
    }

    fn effect_ready_for_manual_resolution(&self, resolution: &ManualResolutionRecord<P>) -> bool {
        self.effects
            .get(&resolution.effect_id)
            .is_some_and(|effect| {
                effect.declaration.generation == self.generation && effect.receipt.is_none()
            })
    }

    #[allow(clippy::too_many_lines)]
    fn apply_manual_resolution(
        &mut self,
        resolution_id: ManualResolutionId,
        resolved_by: &'static str,
        existing: &ManualResolutionRecord<P>,
        choice: &ManualChoice<P>,
        commit: ManualResolutionCommit<P>,
    ) -> ManualEffectOutcome<P> {
        let ManualResolutionCommit {
            transition_codec,
            transition_event,
            next_status,
        } = commit;
        let transition = WorkflowTransition {
            transition_id: TransitionId(self.next_transition_id),
            from_version: self.version,
            to_version: self.version.next(),
            generation: self.generation,
            event: transition_event,
            event_codec: transition_codec,
        };
        self.next_transition_id += 1;
        self.version = transition.to_version;
        self.status = next_status;
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.version = self.version;
        }
        self.transition_log.push(transition);

        let effect_outcome = match choice.kind {
            ManualChoiceKind::Adopt => {
                let receipt_record = ReceiptRecord {
                    id: ReceiptId(self.next_receipt_id),
                    authority: manual_receipt_authority(
                        self.binding.workflow_id(),
                        self.effects[&existing.effect_id].declared_workflow_version,
                        self.generation,
                        existing.effect_id,
                    ),
                    attempt_id: None,
                    origin: ReceiptOrigin::Manual,
                    receipt_codec: choice.receipt_codec.clone(),
                    receipt: choice.receipt.clone(),
                    generation: self.generation,
                };
                self.next_receipt_id += 1;
                let receipt_inbox_event = ReducerInboxEvent {
                    id: ReducerInboxId(self.next_inbox_id),
                    effect_id: Some(existing.effect_id),
                    barrier_id: None,
                    kind: ReducerInboxKind::ReceiptAccepted,
                    event_codec: choice.receipt_event_codec.clone(),
                    requires_runtime_acceptance: P::receipt_requires_runtime_acceptance(
                        &choice.receipt_event,
                    ),
                    payload: ReducerInboxPayload::Receipt(choice.receipt_event.clone()),
                    delivery_status: if workflow_status_is_terminal(self.status) {
                        DeliveryStatus::Suppressed {
                            reason: SuppressionReason::LifecycleTerminal,
                        }
                    } else {
                        DeliveryStatus::Pending
                    },
                    consumed_by: None,
                };
                self.next_inbox_id += 1;
                if let Some(effect) = self.effects.get_mut(&existing.effect_id) {
                    effect.status = EffectStatus::Receipted;
                    effect.claim = None;
                    effect.destructive_lock = None;
                    effect.pending_reconciliation = false;
                    effect.receipt = Some(receipt_record.clone());
                }
                self.reducer_inbox.insert(
                    receipt_inbox_event.id,
                    clone_reducer_inbox_event(&receipt_inbox_event),
                );
                ManualEffectOutcome::Receipt {
                    receipt: Box::new(receipt_record),
                    reducer_event: Box::new(receipt_inbox_event),
                }
            }
            kind @ (ManualChoiceKind::Retry
            | ManualChoiceKind::Compensate
            | ManualChoiceKind::Fail
            | ManualChoiceKind::Suppress) => {
                if let Some(effect) = self.effects.get_mut(&existing.effect_id) {
                    effect.status = match kind {
                        ManualChoiceKind::Retry => EffectStatus::Eligible,
                        ManualChoiceKind::Compensate
                        | ManualChoiceKind::Fail
                        | ManualChoiceKind::Suppress => EffectStatus::Invalidated,
                        ManualChoiceKind::Adopt => unreachable!(),
                    };
                    effect.claim = None;
                    effect.destructive_lock = None;
                    effect.pending_reconciliation = false;
                }
                match kind {
                    ManualChoiceKind::Retry => ManualEffectOutcome::Retry,
                    ManualChoiceKind::Compensate => ManualEffectOutcome::Compensate,
                    ManualChoiceKind::Fail => ManualEffectOutcome::Failed,
                    ManualChoiceKind::Suppress => ManualEffectOutcome::Suppressed,
                    ManualChoiceKind::Adopt => unreachable!(),
                }
            }
        };

        let mut updated = clone_manual_resolution(existing);
        updated.status = ResolutionStatus::Resolved;
        updated.workflow_version = self.version;
        updated.accepted_choice = Some(clone_manual_choice(choice));
        updated.resolved_by = Some(resolved_by);
        self.manual_resolutions.insert(resolution_id, updated);
        self.refresh_eligibility(Timestamp(0));
        effect_outcome
    }

    fn enter_cancellation(&mut self, request: &CancellationRequest<P>) {
        self.generation = self.generation.next();
        if let WorkflowBinding::Authoritative(workflow) = &mut self.binding {
            workflow.generation = self.generation;
        }
        self.status = WorkflowStatus::Cancelling;
        for effect in self.effects.values_mut() {
            if effect.declaration.generation != self.generation {
                effect.claim = None;
                effect.destructive_lock = None;
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
                next_status: match request.compensation_plan.next_status {
                    WorkflowStatus::Active => WorkflowStatus::Cancelling,
                    status @ (WorkflowStatus::Cancelling
                    | WorkflowStatus::Cancelled
                    | WorkflowStatus::DeletionPending
                    | WorkflowStatus::Completed
                    | WorkflowStatus::Failed) => status,
                },
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

    fn suppress_cancellation_work(&mut self, transition: TransitionId) {
        for inbox in self.reducer_inbox.values_mut() {
            if inbox.delivery_status == DeliveryStatus::Pending {
                inbox.delivery_status = DeliveryStatus::Suppressed {
                    reason: SuppressionReason::Cancelled,
                };
            }
        }
        for resolution in self.manual_resolutions.values_mut() {
            if resolution.status == ResolutionStatus::Required {
                resolution.status = ResolutionStatus::Suppressed {
                    transition,
                    reason: SuppressionReason::Cancelled,
                };
                if let Some(effect) = self.effects.get_mut(&resolution.effect_id) {
                    effect.destructive_lock = None;
                }
            }
        }
        for owed in self.owed_acceptances.values_mut() {
            if owed.disposition == OwedAcceptanceDisposition::Owed {
                owed.disposition = OwedAcceptanceDisposition::Suppressed {
                    transition,
                    reason: SuppressionReason::Cancelled,
                };
            }
        }
    }

    fn materialize_cancellation_inbox_events(
        &mut self,
        declarations: &[crate::ReducerInboxDecl<P>],
    ) -> Vec<ReducerInboxEvent<P>> {
        let mut reducer_events = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let event = ReducerInboxEvent {
                id: ReducerInboxId(self.next_inbox_id),
                effect_id: declaration.effect_id,
                barrier_id: declaration.barrier_id,
                kind: declaration.kind,
                event_codec: declaration.event_codec.clone(),
                requires_runtime_acceptance: declaration.requires_runtime_acceptance,
                payload: match &declaration.payload {
                    ReducerInboxPayload::Receipt(payload) => {
                        ReducerInboxPayload::Receipt(payload.clone())
                    }
                    ReducerInboxPayload::Barrier(payload) => {
                        ReducerInboxPayload::Barrier(payload.clone())
                    }
                },
                delivery_status: if workflow_status_is_terminal(self.status) {
                    DeliveryStatus::Suppressed {
                        reason: SuppressionReason::Cancelled,
                    }
                } else {
                    DeliveryStatus::Pending
                },
                consumed_by: None,
            };
            self.next_inbox_id += 1;
            self.reducer_inbox
                .insert(event.id, clone_reducer_inbox_event(&event));
            reducer_events.push(event);
        }
        reducer_events
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
        binding: &OwedAcceptanceDecisionBinding<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
        disposition: OwedAcceptanceDisposition,
    ) -> Result<RuntimeAcceptanceResult<P>, EngineError> {
        self.ensure_executable()?;
        let owed_id = binding.owed.id;
        let decision = &binding.decision;
        let Some(existing) = self.owed_acceptances.get(&owed_id).cloned() else {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::InvalidPlan,
                transition: None,
                owed_acceptance: None,
            });
        };
        if !same_owed_acceptance_source(&existing, &binding.owed) {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::InvalidPlan,
                transition: None,
                owed_acceptance: Some(existing),
            });
        }
        if existing.disposition != OwedAcceptanceDisposition::Owed {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::Committed,
                transition: None,
                owed_acceptance: Some(existing),
            });
        }
        let handles_owed = match disposition {
            OwedAcceptanceDisposition::Accepted { .. } => {
                P::decision_handles_owed_acceptance(&existing.event, &decision.plan.event)
            }
            OwedAcceptanceDisposition::Suppressed { .. } => {
                P::decision_handles_owed_acceptance_suppression(
                    &existing.event,
                    &decision.plan.event,
                )
            }
            OwedAcceptanceDisposition::Owed => false,
        };
        if !handles_owed {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::InvalidPlan,
                transition: None,
                owed_acceptance: Some(existing),
            });
        }
        if decision.expected_workflow_version != self.version {
            return Ok(RuntimeAcceptanceResult {
                outcome: CommitOutcome::VersionConflict,
                transition: None,
                owed_acceptance: Some(existing),
            });
        }
        self.validate_plan_against_state(&decision.plan, barrier_events)?;
        if matches!(disposition, OwedAcceptanceDisposition::Accepted { .. })
            && !P::runtime_start_allowed(&self.snapshot)
        {
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
        if decision.plan.owed_acceptances.is_some() {
            return Err(EngineError::InvalidInbox);
        }
        let owed = replacement
            .owed_acceptances
            .get_mut(&owed_id)
            .expect("validated owed exists");
        owed.disposition = match disposition {
            OwedAcceptanceDisposition::Accepted { .. } => OwedAcceptanceDisposition::Accepted {
                transition: transition.transition_id,
            },
            OwedAcceptanceDisposition::Suppressed { reason, .. } => {
                OwedAcceptanceDisposition::Suppressed {
                    transition: transition.transition_id,
                    reason,
                }
            }
            OwedAcceptanceDisposition::Owed => unreachable!("resolution must be terminal"),
        };
        let owed_acceptance = owed.clone();
        replacement.refresh_eligibility(Timestamp(0));
        *self = replacement;
        Ok(RuntimeAcceptanceResult {
            outcome: CommitOutcome::Committed,
            transition: Some(transition),
            owed_acceptance: Some(owed_acceptance),
        })
    }

    fn validate_plan_against_state(
        &self,
        plan: &TransitionPlan<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<(), EngineError> {
        self.validate_plan_against_state_for_path(plan, barrier_events, false)
    }

    fn validate_cancellation_plan_against_state(
        &self,
        plan: &TransitionPlan<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    ) -> Result<(), EngineError> {
        self.validate_plan_against_state_for_path(plan, barrier_events, true)
    }

    fn validate_plan_against_state_for_path(
        &self,
        plan: &TransitionPlan<P>,
        barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
        cancellation: bool,
    ) -> Result<(), EngineError> {
        if cancellation {
            let next_status = match plan.next_status {
                WorkflowStatus::Active => WorkflowStatus::Cancelling,
                next @ (WorkflowStatus::Cancelling
                | WorkflowStatus::Cancelled
                | WorkflowStatus::DeletionPending
                | WorkflowStatus::Completed
                | WorkflowStatus::Failed) => next,
            };
            validate_status_transition(WorkflowStatus::Cancelling, next_status)
                .map_err(EngineError::InvalidPlan)?;
            validate_plan_body(plan, barrier_events).map_err(EngineError::InvalidPlan)?;
        } else {
            validate_plan(self.status, plan, barrier_events).map_err(EngineError::InvalidPlan)?;
        }
        let mut family_ambiguity = self
            .effects
            .values()
            .map(|effect| (effect.declaration.family, effect.declaration.ambiguity))
            .collect::<BTreeMap<_, _>>();
        for effect in &plan.effects {
            if self.effects.contains_key(&effect.effect_id) {
                return Err(EngineError::InvalidPlan(PlanError::EffectIdCollision(
                    effect.effect_id,
                )));
            }
            if cancellation && effect.role != crate::EffectRole::Compensation {
                return Err(EngineError::InvalidPlan(
                    PlanError::NonCompensationInCancellation(effect.effect_id),
                ));
            }
            if !cancellation && effect.role == crate::EffectRole::Compensation {
                return Err(EngineError::InvalidPlan(
                    PlanError::CompensationOutsideCancellation(effect.effect_id),
                ));
            }
            if family_ambiguity
                .insert(effect.family, effect.ambiguity)
                .is_some_and(|registered| registered != effect.ambiguity)
            {
                return Err(EngineError::InvalidPlan(
                    PlanError::EffectFamilyAmbiguityMismatch {
                        family: effect.family,
                    },
                ));
            }
            let expected_generation = if cancellation {
                self.generation.next()
            } else {
                self.generation
            };
            if effect.generation != expected_generation {
                return Err(EngineError::InvalidPlan(
                    PlanError::EffectGenerationMismatch {
                        effect_id: effect.effect_id,
                        expected: expected_generation,
                        actual: effect.generation,
                    },
                ));
            }
        }
        for barrier in &plan.barriers {
            if self.barriers.contains_key(&barrier.barrier_id) {
                return Err(EngineError::InvalidPlan(PlanError::BarrierIdCollision(
                    barrier.barrier_id,
                )));
            }
        }
        for invalidation in &plan.invalidations {
            let Some(effect) = self.effects.get(&invalidation.effect_id) else {
                return Err(EngineError::InvalidPlan(
                    PlanError::UnknownInvalidationTarget(invalidation.effect_id),
                ));
            };
            if effect.status == EffectStatus::Receipted {
                return Err(EngineError::InvalidPlan(
                    PlanError::InvalidatesReceiptedEffect(invalidation.effect_id),
                ));
            }
            if !cancellation
                && self.manual_resolutions.values().any(|resolution| {
                    resolution.effect_id == invalidation.effect_id
                        && resolution.status == ResolutionStatus::Required
                })
            {
                return Err(EngineError::InvalidPlan(
                    PlanError::InvalidatesManualResolutionEffect(invalidation.effect_id),
                ));
            }
        }
        Ok(())
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
            && claim.lease_until == authority.lease_until
            && claim.resource_lock == authority.resource_lock
            && effect.destructive_lock == authority.resource_lock
            && authority.generation == self.generation
            && claim.lease_until.is_live_at(now)
        {
            Some(effect)
        } else {
            None
        }
    }
}

fn same_claim_identity(left: &ClaimAuthority, right: &ClaimAuthority) -> bool {
    left == right
}

fn same_inbox_payload<P: WorkflowProfile>(
    left: &ReducerInboxPayload<P>,
    right: &ReducerInboxPayload<P>,
) -> bool {
    match (left, right) {
        (ReducerInboxPayload::Receipt(left), ReducerInboxPayload::Receipt(right)) => left == right,
        (ReducerInboxPayload::Barrier(left), ReducerInboxPayload::Barrier(right)) => left == right,
        _ => false,
    }
}

fn same_inbox_event<P: WorkflowProfile>(
    left: &ReducerInboxEvent<P>,
    right: &ReducerInboxEvent<P>,
) -> bool {
    left.id == right.id
        && left.effect_id == right.effect_id
        && left.barrier_id == right.barrier_id
        && left.kind == right.kind
        && left.event_codec == right.event_codec
        && left.requires_runtime_acceptance == right.requires_runtime_acceptance
        && same_inbox_payload(&left.payload, &right.payload)
        && left.delivery_status == right.delivery_status
        && left.consumed_by == right.consumed_by
}

fn stale_renewal() -> RenewalResult {
    RenewalResult {
        outcome: AuthorityOutcome::StaleAuthority,
        authority: None,
    }
}

fn stale_reconciliation_outcome<P: WorkflowProfile>() -> ReconciliationOutcome<P> {
    ReconciliationOutcome {
        outcome: AuthorityOutcome::StaleAuthority,
        decision: None,
        manual_resolution: None,
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
        effect_outcome: None,
    }
}

fn version_conflict_manual_resolution<P: WorkflowProfile>(
    resolution: ManualResolutionRecord<P>,
) -> ManualResolutionOutcome<P> {
    ManualResolutionOutcome {
        outcome: CommitOutcome::VersionConflict,
        resolution: Some(resolution),
        effect_outcome: None,
    }
}

fn same_owed_acceptance_source<E: Eq>(
    existing: &OwedAcceptanceRecord<E>,
    supplied: &OwedAcceptanceRecord<E>,
) -> bool {
    existing.id == supplied.id
        && existing.reducer_inbox_id == supplied.reducer_inbox_id
        && existing.source_kind == supplied.source_kind
        && existing.event_codec == supplied.event_codec
        && existing.event == supplied.event
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

const fn workflow_status_is_terminal(status: WorkflowStatus) -> bool {
    matches!(
        status,
        WorkflowStatus::Cancelled | WorkflowStatus::Completed | WorkflowStatus::Failed
    )
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
