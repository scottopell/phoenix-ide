//! Synchronous in-process policy/guardian coordination; no scheduler or native supervisor.
use std::{collections::VecDeque, convert::Infallible};

use crate::guardian::{AppliedResultAck, PolicySubmissionError, ReconciledSessionPermit};
use crate::{
    ControllerSession, Guardian, GuardianError, GuardianExecutor, GuardianInstanceId, GuardianState,
};

use crate::{
    AttemptId, AttemptStore, ChildSlotId, CommittedTransition, DeploymentAttempt, DispatchBatch,
    DispatchKey, DispatchTicket, EffectCompletion, ExecutedEffect, LoadedAttempt, MonotonicTick,
    OsEffect, OsQueryError, PersistenceRevision, Phase, ProcessIdentity,
    ResourceObservationFailure, Signal, SignalOutcome, Transition, TransitionError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParkReason {
    ResourceObservationFailure {
        sequence: u64,
        failure: ResourceObservationFailure,
    },
    ChildQueryFailure {
        parent: ProcessIdentity,
        failure: OsQueryError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopStatus {
    Running,
    Parked(ParkReason),
    FailStopped,
}

/// Metadata from the last acknowledged write, not a resumable checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopSummary {
    pub attempt_id: AttemptId,
    pub phase: Phase,
    pub revision: PersistenceRevision,
    pub status: LoopStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AckDisposition {
    AcknowledgeNow,
    RetainUntilSignalSettlement,
    RetainParked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopFenceReason {
    PersistenceFailure,
    Interrupted,
    DeadlineReached,
    QueryFailure,
    SignalCapacityExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalResult {
    pub key: DispatchKey,
    pub identity: ProcessIdentity,
    pub signal: Signal,
    pub outcome: SignalOutcome,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LoopProgress {
    Applied {
        key: DispatchKey,
        revision: PersistenceRevision,
        phase: Phase,
        ack: AckDisposition,
    },
    Parked {
        key: DispatchKey,
        revision: PersistenceRevision,
        reason: ParkReason,
        ack: AckDisposition,
    },
    Advanced {
        revision: PersistenceRevision,
        phase: Phase,
    },
    /// The coordinator must fence the old guardian session before submitting any
    /// held ticket. This loop cannot revoke tickets already handed out.
    GuardianMustFence {
        revision: PersistenceRevision,
        phase: Phase,
        reason: LoopFenceReason,
    },
    SignalRoundFinished {
        revision: PersistenceRevision,
        results: Vec<SignalResult>,
    },
}

impl LoopProgress {
    /// Only keys whose evidence or superseding barrier is durably incorporated.
    pub fn acknowledge_keys(&self) -> impl Iterator<Item = DispatchKey> + '_ {
        let key = match self {
            Self::Applied {
                key,
                ack: AckDisposition::AcknowledgeNow,
                ..
            } => Some(*key),
            Self::Applied { .. }
            | Self::Parked { .. }
            | Self::Advanced { .. }
            | Self::GuardianMustFence { .. }
            | Self::SignalRoundFinished { .. } => None,
        };
        let results = match self {
            Self::SignalRoundFinished { results, .. } => results.as_slice(),
            Self::Applied { .. }
            | Self::Parked { .. }
            | Self::Advanced { .. }
            | Self::GuardianMustFence { .. } => &[],
        };
        key.into_iter()
            .chain(results.iter().map(|result| result.key))
    }

    #[must_use]
    pub const fn guardian_fence(&self) -> Option<LoopFenceReason> {
        match self {
            Self::GuardianMustFence { reason, .. } => Some(*reason),
            Self::Parked { .. } => Some(LoopFenceReason::QueryFailure),
            Self::Applied { .. } | Self::Advanced { .. } | Self::SignalRoundFinished { .. } => None,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct EffectMismatchDetail {
    pub key: DispatchKey,
    pub expected: OsEffect,
    pub actual: OsEffect,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LoopError<E> {
    FailStopped,
    Parked(ParkReason),
    NotIssued(DispatchKey),
    EffectMismatch(Box<EffectMismatchDetail>),
    NotSuperseded(DispatchKey),
    RoundInFlight,
    NoSignalRound,
    SignalRoundIncomplete,
    Transition(TransitionError),
    Persistence(E),
}

impl<E> LoopError<E> {
    /// A failed/ambiguous write requires fencing even when the caller still owns
    /// raw tickets. Clearing this loop's ledger does not revoke those tickets.
    #[must_use]
    pub const fn guardian_fence(&self) -> Option<LoopFenceReason> {
        match self {
            Self::Persistence(_) | Self::FailStopped => Some(LoopFenceReason::PersistenceFailure),
            Self::Parked(_) => Some(LoopFenceReason::QueryFailure),
            Self::NotIssued(_)
            | Self::EffectMismatch(_)
            | Self::NotSuperseded(_)
            | Self::RoundInFlight
            | Self::NoSignalRound
            | Self::SignalRoundIncomplete
            | Self::Transition(_) => None,
        }
    }
}

/// Closed policy categories for the guardian bridge, without effects or observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopFailure {
    FailStopped,
    Parked,
    NotIssued,
    EffectMismatch,
    NotSuperseded,
    RoundInFlight,
    NoSignalRound,
    SignalRoundIncomplete,
    Transition,
    RecoveryNotAdmitted,
    GuardianNotBound,
}

#[derive(Debug, Eq, PartialEq)]
pub enum GuardianLoopError<E> {
    Guardian(GuardianError),
    Policy(LoopFailure),
    Persistence(E),
    FencingFailed {
        cause: Box<Self>,
        fencing: GuardianError,
    },
}

impl<E> From<LoopError<E>> for GuardianLoopError<E> {
    fn from(error: LoopError<E>) -> Self {
        let kind = match error {
            LoopError::FailStopped => LoopFailure::FailStopped,
            LoopError::Parked(_) => LoopFailure::Parked,
            LoopError::NotIssued(_) => LoopFailure::NotIssued,
            LoopError::EffectMismatch(_) => LoopFailure::EffectMismatch,
            LoopError::NotSuperseded(_) => LoopFailure::NotSuperseded,
            LoopError::RoundInFlight => LoopFailure::RoundInFlight,
            LoopError::NoSignalRound => LoopFailure::NoSignalRound,
            LoopError::SignalRoundIncomplete => LoopFailure::SignalRoundIncomplete,
            LoopError::Transition(_) => LoopFailure::Transition,
            LoopError::Persistence(error) => return Self::Persistence(error),
        };
        Self::Policy(kind)
    }
}

struct IssuedEffect {
    key: DispatchKey,
    effect: OsEffect,
}

struct Round {
    dispatch_revision: PersistenceRevision,
    unissued: DispatchBatch,
    returned: Option<DispatchTicket>,
    issued: Vec<IssuedEffect>,
    signals: Option<Vec<SignalResult>>,
}

impl Round {
    fn complete(&self) -> bool {
        self.returned.is_none() && self.unissued.is_empty() && self.issued.is_empty()
    }

    fn signal_reservation_request(&self) -> Option<(PersistenceRevision, usize)> {
        self.signals.as_ref()?;
        if self.returned.is_none() && self.unissued.is_empty() {
            return None;
        }
        let count = self
            .unissued
            .len()
            .saturating_add(self.issued.len())
            .saturating_add(usize::from(self.returned.is_some()));
        Some((self.dispatch_revision, count))
    }
}

enum Receipt {
    Applied,
    Parked(ParkReason),
    Signal(SignalResult),
}

/// Owns one committed attempt and its affine dispatch authority. A round's newly
/// committed follow-up batches wait until all original siblings are applied.
///
/// Prefer coordinated methods for guardian fencing and durable ACK boundaries.
///
/// Issued-key membership rejects stale application; revision advances do not
/// invalidate outstanding siblings. It does not authenticate guardian sessions or
/// revoke tickets already handed out. The coordinator must honor fence returns.
pub struct PolicyLoop {
    checkpoint: DeploymentAttempt,
    status: LoopStatus,
    rounds: VecDeque<Round>,
    interrupted_revision: Option<PersistenceRevision>,
    bound_session: Option<ControllerSession>,
    bound_guardian_instance: Option<GuardianInstanceId>,
    recovery_session: Option<ControllerSession>,
}

impl PolicyLoop {
    /// Creates a pure executor loop with no coordinated guardian authority.
    #[must_use]
    pub fn from_committed(committed: CommittedTransition) -> Self {
        let signal_round = has_signal_effects(&committed);
        Self::with_round(committed, signal_round)
    }

    /// Binds a newly committed plan before any coordinated dispatch can occur.
    ///
    /// # Errors
    /// Requires the active guardian's exact attempt, deadline and current session.
    /// Recovery plans require `from_committed_recovery_for_guardian` instead.
    pub fn from_committed_for_guardian<G: GuardianExecutor>(
        committed: CommittedTransition,
        guardian: &Guardian<G>,
        session: ControllerSession,
    ) -> Result<Self, GuardianLoopError<Infallible>> {
        guardian
            .authenticate_policy_session(
                session,
                committed.checkpoint().attempt_id(),
                committed.checkpoint().deadline(),
            )
            .map_err(GuardianLoopError::Guardian)?;
        if guardian.state() != GuardianState::Active || committed.recovery_origin().is_some() {
            return Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted));
        }
        let signal_round = committed.is_owned_signal_round();
        Ok(Self {
            bound_session: Some(session),
            bound_guardian_instance: Some(guardian.instance_id()),
            ..Self::with_round(committed, signal_round)
        })
    }

    /// Binds crash-loaded interruption without another write or recovery preparation.
    ///
    /// # Errors
    /// Requires committed Interrupted evidence and the exact surviving guardian,
    /// original deadline, and current (possibly detached) session.
    pub fn from_loaded_interruption_for_guardian<G: GuardianExecutor>(
        loaded: LoadedAttempt,
        guardian: &Guardian<G>,
        session: ControllerSession,
    ) -> Result<Self, GuardianLoopError<Infallible>> {
        let committed = loaded
            .into_committed_interruption()
            .map_err(|_| GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))?;
        guardian
            .authenticate_policy_binding(
                session,
                committed.checkpoint().attempt_id(),
                committed.checkpoint().deadline(),
            )
            .map_err(GuardianLoopError::Guardian)?;
        Ok(Self {
            bound_session: Some(session),
            bound_guardian_instance: Some(guardian.instance_id()),
            ..Self::with_round(committed, false)
        })
    }

    /// Binds a persisted recovery plan, including one prepared from loaded evidence.
    ///
    /// # Errors
    /// Requires ephemeral recovery origin authority and the exact reconciling session.
    /// Ordinary checkpoints cannot manufacture a fresh-reconciliation barrier.
    pub fn from_committed_recovery_for_guardian<G: GuardianExecutor>(
        committed: CommittedTransition,
        guardian: &Guardian<G>,
        session: ControllerSession,
    ) -> Result<Self, GuardianLoopError<Infallible>> {
        guardian
            .authenticate_policy_session(
                session,
                committed.checkpoint().attempt_id(),
                committed.checkpoint().deadline(),
            )
            .map_err(GuardianLoopError::Guardian)?;
        let floor = guardian
            .reconciliation_floor()
            .ok_or(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))?;
        if guardian.state() != GuardianState::Reconciling
            || committed.recovery_origin() != Some(floor)
            || committed_revision(&committed) < floor
            || !matches!(
                committed.checkpoint().phase(),
                Phase::Recovering | Phase::ResourceSampling
            )
        {
            return Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted));
        }
        Ok(Self {
            bound_session: Some(session),
            bound_guardian_instance: Some(guardian.instance_id()),
            recovery_session: Some(session),
            ..Self::with_round(committed, false)
        })
    }

    /// # Errors
    /// Requires a committed `request_owned_signals` plan before its deadline,
    /// including a plan with zero candidates. Observation plans are rejected.
    pub fn from_committed_signal_round(
        committed: CommittedTransition,
    ) -> Result<Self, LoopError<Infallible>> {
        if !committed.is_owned_signal_round() {
            return Err(LoopError::NoSignalRound);
        }
        Ok(Self::with_round(committed, true))
    }

    fn with_round(committed: CommittedTransition, signal_round: bool) -> Self {
        let interrupted_revision = committed.recovery_origin().or_else(|| {
            (committed.checkpoint().phase() == Phase::Interrupted)
                .then(|| committed_revision(&committed))
        });
        let (checkpoint, batch) = committed.into_dispatch();
        let mut result = Self {
            checkpoint,
            status: LoopStatus::Running,
            rounds: VecDeque::new(),
            interrupted_revision,
            bound_session: None,
            bound_guardian_instance: None,
            recovery_session: None,
        };
        result.enqueue(batch, signal_round);
        result
    }

    /// Last acknowledged metadata, including after fail-stop. An ambiguous failed
    /// write may have advanced the durable store beyond this revision.
    ///
    /// ```compile_fail
    /// use phoenix_deployment_controller::{DeploymentAttempt, PolicyLoop};
    /// fn revive(policy: &PolicyLoop) -> DeploymentAttempt {
    ///     policy.summary().clone()
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use phoenix_deployment_controller::PolicyLoop;
    /// fn checkpoint(policy: &PolicyLoop) { let _ = policy.checkpoint().clone(); }
    /// ```
    ///
    /// # Panics
    /// Panics if the private committed checkpoint lacks a persisted revision.
    #[must_use]
    pub fn summary(&self) -> LoopSummary {
        LoopSummary {
            attempt_id: self.checkpoint.attempt_id(),
            phase: self.checkpoint.phase(),
            revision: self
                .checkpoint
                .persistence_revision()
                .expect("committed revision"),
            status: self.status,
        }
    }

    #[must_use]
    pub const fn status(&self) -> LoopStatus {
        self.status
    }

    /// Outcomes are retained until explicit settlement, then returned in progress.
    #[must_use]
    pub fn signal_results(&self) -> Option<&[SignalResult]> {
        self.rounds.front()?.signals.as_deref()
    }

    /// Issues at most one ticket. No following round is issued while siblings
    /// remain outstanding or while a signal round awaits explicit settlement.
    #[must_use]
    pub fn issue_next(&mut self) -> Option<DispatchTicket> {
        if self.status != LoopStatus::Running {
            return None;
        }
        let round = self.rounds.front_mut()?;
        if let Some(ticket) = round.returned.take() {
            return Some(ticket);
        }
        let ticket = round.unissued.next()?;
        round.issued.push(IssuedEffect {
            key: ticket.key(),
            effect: ticket.effect().clone(),
        });
        Some(ticket)
    }

    /// Only the atomic bridge can return a ticket known not to have executed.
    fn restore_unsubmitted(&mut self, ticket: DispatchTicket) {
        let round = self.rounds.front_mut().expect("issued front round");
        assert!(round.returned.is_none());
        assert!(round
            .issued
            .iter()
            .any(|entry| entry.key == ticket.key() && entry.effect == *ticket.effect()));
        round.returned = Some(ticket);
    }

    /// Convenience for a caller that owns its result. For guardian-retained results,
    /// use `apply_retained` so a parked or failed application cannot lose evidence.
    /// This method drops its input even when its disposition requires retention.
    ///
    /// # Errors
    /// Returns the same errors as `apply_retained`.
    pub fn apply<S: AttemptStore>(
        &mut self,
        executed: ExecutedEffect,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        let progress = self.apply_retained(&executed, store);
        let _ = executed.into_parts();
        progress
    }

    /// Borrows the guardian's sole result and persists before returning any ACK
    /// permission. Checkpoint-only query failures and signal receipts do not make
    /// the typed result durable: keep them retained until classification/settlement.
    ///
    /// # Panics
    /// Panics if private dispatch bookkeeping loses a signal round's accumulator.
    ///
    /// # Errors
    /// Rejects foreign, stale, duplicate, or mismatched completions without a
    /// checkpoint or ledger change. Any persistence failure irreversibly fail-stops
    /// the loop; only a caller-owned reload outside this object can recover.
    pub fn apply_retained<S: AttemptStore>(
        &mut self,
        executed: &ExecutedEffect,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.require_running()?;
        let key = executed.key();
        let issued = self
            .rounds
            .front()
            .and_then(|round| round.issued.iter().position(|entry| entry.key == key))
            .ok_or(LoopError::NotIssued(key))?;
        let expected = &self.rounds[0].issued[issued].effect;
        let actual = executed.completion().effect();
        if *expected != actual {
            return Err(LoopError::EffectMismatch(Box::new(EffectMismatchDetail {
                key,
                expected: expected.clone(),
                actual,
            })));
        }
        let (transition, receipt) = self.plan_completion(key, executed.completion())?;
        let committed = self.persist(transition, store)?;
        let revision = committed_revision(&committed);
        let has_signals = has_signal_effects(&committed);
        let (checkpoint, batch) = committed.into_dispatch();
        self.checkpoint = checkpoint;
        let ack = match receipt {
            Receipt::Parked(reason) => {
                self.rounds.clear();
                self.status = LoopStatus::Parked(reason);
                return Ok(LoopProgress::Parked {
                    key,
                    revision,
                    reason,
                    ack: AckDisposition::RetainParked,
                });
            }
            Receipt::Signal(result) => {
                let results = self.rounds[0]
                    .signals
                    .as_mut()
                    .expect("signal ticket belongs to a signal round");
                results.push(result);
                results.sort_unstable_by_key(|result| result.key.ordinal());
                AckDisposition::RetainUntilSignalSettlement
            }
            Receipt::Applied => AckDisposition::AcknowledgeNow,
        };
        self.rounds[0].issued.remove(issued);
        if self.rounds[0].complete() && self.rounds[0].signals.is_none() {
            self.rounds.pop_front();
        }
        self.enqueue(batch, has_signals);
        Ok(LoopProgress::Applied {
            key,
            revision,
            phase: self.checkpoint.phase(),
            ack,
        })
    }

    /// Explicitly classifies a retained result from this attempt issued strictly
    /// before a committed interruption, including on a replacement loop constructed
    /// from that interruption. The guardian bridge must authenticate its session.
    /// Only the key participates in classification, never its observation/outcome.
    /// The returned key is eligible for ACK; classification is repeatable.
    ///
    /// # Errors
    /// Rejects foreign attempts, results not older than committed interruption, and
    /// fail-stop. Ordinary `apply_retained` still rejects these stale results.
    pub fn classify_superseded_retained(
        &self,
        executed: &ExecutedEffect,
    ) -> Result<DispatchKey, LoopError<Infallible>> {
        if self.status == LoopStatus::FailStopped {
            return Err(LoopError::FailStopped);
        }
        let key = executed.key();
        if key.attempt_id() != self.checkpoint.attempt_id()
            || self
                .interrupted_revision
                .is_none_or(|revision| key.revision().get() >= revision.get())
        {
            return Err(LoopError::NotSuperseded(key));
        }
        Ok(key)
    }

    /// # Panics
    /// Panics if private dispatch bookkeeping loses the validated signal round.
    ///
    /// # Errors
    /// Requires a signal round with every sibling applied. Settlement commits the
    /// state's fresh-verification barrier; no signal outcome substitutes for it.
    pub fn finish_signal_round<S: AttemptStore>(
        &mut self,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.require_running()?;
        let round = self.rounds.front().ok_or(LoopError::NoSignalRound)?;
        if round.signals.is_none() {
            return Err(LoopError::NoSignalRound);
        }
        if !round.complete() {
            return Err(LoopError::SignalRoundIncomplete);
        }
        let transition = self
            .checkpoint
            .clone()
            .cleanup_signals_settled()
            .map_err(LoopError::Transition)?;
        let committed = self.persist(transition, store)?;
        let revision = committed_revision(&committed);
        let results = self
            .rounds
            .pop_front()
            .and_then(|round| round.signals)
            .expect("validated signal round");
        let (checkpoint, batch) = committed.into_dispatch();
        self.checkpoint = checkpoint;
        self.enqueue(batch, false);
        Ok(LoopProgress::SignalRoundFinished { revision, results })
    }

    /// # Errors
    /// Requires no pending round and the state's completed-discovery preconditions.
    pub fn finish_ownership_scan<S: AttemptStore>(
        &mut self,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.advance(store, DeploymentAttempt::ownership_scan_completed)
    }

    /// The caller, never this loop, selects the signal and supplies the clock.
    ///
    /// # Errors
    /// Requires no pending round and the state's exact signal-admission conditions.
    pub fn request_owned_signals<S: AttemptStore>(
        &mut self,
        signal: Signal,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.require_idle()?;
        let transition = self
            .checkpoint
            .clone()
            .request_owned_signals(signal, now)
            .map_err(LoopError::Transition)?;
        let committed = self.persist(transition, store)?;
        let signal_round = committed.is_owned_signal_round();
        let fence = (!signal_round).then_some(if now >= self.checkpoint.deadline() {
            LoopFenceReason::DeadlineReached
        } else {
            LoopFenceReason::SignalCapacityExceeded
        });
        Ok(self.replace(committed, signal_round, fence))
    }

    /// # Errors
    /// Requires no pending round and the state's verification-phase preconditions.
    pub fn finish_verification<S: AttemptStore>(
        &mut self,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.advance(store, |checkpoint| checkpoint.verification_completed(now))
    }

    /// # Errors
    /// Requires no pending round and the state's fresh-reconciliation preconditions.
    pub fn finish_recovery<S: AttemptStore>(
        &mut self,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.advance(store, |checkpoint| {
            checkpoint.recovery_reconciliation_completed(now)
        })
    }

    /// Commits interruption and retires this loop's old ledger. The returned fence
    /// requirement must be honored by the coordinator before any held submission.
    ///
    /// # Errors
    /// Rejects fail-stop or any phase the state cannot interrupt.
    pub fn interrupt<S: AttemptStore>(
        &mut self,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.replace_with(
            store,
            DeploymentAttempt::interrupt,
            Some(LoopFenceReason::Interrupted),
        )
    }

    /// # Errors
    /// Rejects fail-stop or any phase the state cannot recover. A parked active
    /// attempt must first checkpoint interruption; its deadline is not extended.
    pub fn recover<S: AttemptStore>(
        &mut self,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.replace_with(store, DeploymentAttempt::recover, None)
    }

    /// # Errors
    /// Rejects fail-stop, an unexpired deadline, or any state without the exact
    /// deadline transition. Success requires fencing old guardian dispatch without
    /// interpreting it; already handed-out raw tickets are not revoked here.
    pub fn deadline_reached<S: AttemptStore>(
        &mut self,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.replace_with(
            store,
            |checkpoint| checkpoint.deadline_reached(now),
            Some(LoopFenceReason::DeadlineReached),
        )
    }

    /// Issues and submits at most one affine ticket. No ticket is relabeled by callers.
    ///
    /// # Errors
    /// Refusals preserve the exact front-round ticket; execution mismatches fail-stop.
    pub fn dispatch_next_to_guardian<G: GuardianExecutor>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        now: MonotonicTick,
    ) -> Result<Option<DispatchKey>, GuardianLoopError<Infallible>> {
        self.check_binding(guardian, session)?;
        if let Err(error) = self.require_running::<Infallible>() {
            let error = GuardianLoopError::from(error);
            return Err(if self.status == LoopStatus::FailStopped {
                self.fail_stop_guardian(guardian, session, error)
            } else {
                error
            });
        }
        self.check_bridge(guardian, session)?;
        if let Some((revision, count)) = self
            .rounds
            .front()
            .and_then(Round::signal_reservation_request)
        {
            guardian
                .reserve_signal_results(session, revision, count)
                .map_err(GuardianLoopError::Guardian)?;
        }
        let Some(ticket) = self.issue_next() else {
            return Ok(None);
        };
        match guardian.submit_policy_ticket(session, ticket, now) {
            Ok(key) => Ok(Some(key)),
            Err(PolicySubmissionError::Refused { error, ticket }) => {
                self.restore_unsubmitted(ticket);
                Err(GuardianLoopError::Guardian(error))
            }
            Err(PolicySubmissionError::ExecutorCorrelationMismatch) => {
                self.status = LoopStatus::FailStopped;
                Err(GuardianLoopError::Guardian(
                    GuardianError::ExecutorCorrelationMismatch,
                ))
            }
        }
    }

    /// Persists borrowed evidence before ACK; parked queries and signals remain retained.
    ///
    /// # Errors
    /// Rejects unauthenticated or unissued results; failed writes revoke guardian authority.
    pub fn apply_guardian_result<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        key: DispatchKey,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_bridge(guardian, session)?;
        let executed = guardian
            .retained_result(key)
            .ok_or(GuardianLoopError::Guardian(
                GuardianError::ResultNotRetained,
            ))?;
        let result = self.apply_retained(executed, store);
        let progress = self.coordinate_result(guardian, session, result)?;
        self.ack_progress(guardian, session, &progress)?;
        if matches!(progress, LoopProgress::Parked { .. }) {
            if let Err(error) = guardian.controller_lost(session) {
                return Err(self.fail_stop_guardian(
                    guardian,
                    session,
                    GuardianLoopError::Guardian(error),
                ));
            }
        }
        Ok(progress)
    }

    /// Commits settlement before acknowledging the exact signal keys returned by progress.
    ///
    /// # Errors
    /// Every accumulated result must still be retained in this guardian/session.
    pub fn finish_guardian_signal_round<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_bridge(guardian, session)?;
        for result in self.signal_results().unwrap_or_default() {
            if guardian.retained_result(result.key).is_none() {
                return Err(self.fail_stop_guardian(
                    guardian,
                    session,
                    GuardianLoopError::Guardian(GuardianError::ResultNotRetained),
                ));
            }
        }
        let result = self.finish_signal_round(store);
        let progress = self.coordinate_result(guardian, session, result)?;
        self.ack_progress(guardian, session, &progress)?;
        Ok(progress)
    }

    /// Fences first; a failed write cannot leave held tickets executable.
    ///
    /// # Errors
    /// Failed writes escalate the detached guardian to irreversible authority loss.
    pub fn interrupt_with_guardian<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_binding(guardian, session)?;
        guardian
            .controller_lost(session)
            .map_err(GuardianLoopError::Guardian)?;
        let result = self.interrupt(store);
        self.coordinate_result(guardian, session, result)
    }

    /// Irreversibly revokes authority before classifying the deadline; never signals.
    ///
    /// # Errors
    /// A premature call still revokes authority even if the state refuses classification.
    pub fn deadline_with_guardian<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_binding(guardian, session)?;
        guardian
            .authority_lost(session)
            .map_err(GuardianLoopError::Guardian)?;
        let result = self.deadline_reached(now, store);
        self.coordinate_result(guardian, session, result)
    }

    /// Starts a replacement session at this loop's committed interruption revision.
    ///
    /// # Errors
    /// Requires Interrupted, non-failed committed state, the original guardian/session
    /// binding and deadline, and its exact inventory. No OS effects or writes occur.
    pub fn begin_guardian_reconciliation<G: GuardianExecutor>(
        &mut self,
        guardian: &mut Guardian<G>,
        old_session: ControllerSession,
        expected_slots: &[ChildSlotId],
    ) -> Result<ControllerSession, GuardianLoopError<Infallible>> {
        self.check_binding(guardian, old_session)?;
        self.require_running().map_err(GuardianLoopError::from)?;
        let summary = self.summary();
        if summary.phase != Phase::Interrupted
            || self.interrupted_revision != Some(summary.revision)
        {
            return Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted));
        }
        guardian
            .begin_reconciliation(old_session.instance_id(), expected_slots, summary.revision)
            .map_err(GuardianLoopError::Guardian)
    }

    /// After `begin_guardian_reconciliation`, commits fresh observation work.
    ///
    /// # Errors
    /// Requires a committed interruption and the new authenticated reconciling session.
    pub fn recover_with_guardian<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_instance(guardian)?;
        guardian
            .authenticate_policy_session(
                session,
                self.checkpoint.attempt_id(),
                self.checkpoint.deadline(),
            )
            .map_err(GuardianLoopError::Guardian)?;
        if guardian.state() != GuardianState::Reconciling
            || self.checkpoint.phase() != Phase::Interrupted
            || guardian.reconciliation_floor() != Some(self.summary().revision)
            || self.interrupted_revision != guardian.reconciliation_floor()
        {
            return Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted));
        }
        let result = self.recover(store);
        let progress = self.coordinate_result(guardian, session, result)?;
        self.bound_session = Some(session);
        self.recovery_session = Some(session);
        Ok(progress)
    }

    /// Commits the owned-tree fresh barrier before minting activation proof.
    /// Resource sampling can activate from its committed recovery plan before
    /// resuming samples; it has no owned identities, and admission starts discovery.
    ///
    /// # Errors
    /// Incomplete, expired, wrong-session, or failed barriers fail-stop the guardian.
    pub fn finish_recovery_with_guardian<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_instance(guardian)?;
        let result = self.finish_recovery_barrier(guardian, session, now, store);
        result.map_err(|error| self.fail_stop_guardian(guardian, session, error))
    }

    fn finish_recovery_barrier<G: GuardianExecutor, S: AttemptStore>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        now: MonotonicTick,
        store: &mut S,
    ) -> Result<LoopProgress, GuardianLoopError<S::Error>> {
        self.check_bridge(guardian, session)?;
        let floor = guardian
            .reconciliation_floor()
            .ok_or(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted))?;
        if guardian.state() != GuardianState::Reconciling
            || self.recovery_session != Some(session)
            || self.interrupted_revision != Some(floor)
            || self.summary().revision < floor
            || !matches!(
                self.checkpoint.phase(),
                Phase::Recovering | Phase::ResourceSampling
            )
        {
            return Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted));
        }
        if now >= self.checkpoint.deadline() {
            return Err(GuardianLoopError::Guardian(GuardianError::DeadlineExpired));
        }
        self.require_running().map_err(GuardianLoopError::from)?;
        let progress = if self.checkpoint.phase() == Phase::ResourceSampling {
            LoopProgress::Advanced {
                revision: self.summary().revision,
                phase: Phase::ResourceSampling,
            }
        } else {
            self.finish_recovery(now, store)
                .map_err(GuardianLoopError::from)?
        };
        if !matches!(
            self.checkpoint.phase(),
            Phase::ResourceSampling
                | Phase::OwnershipDiscovery
                | Phase::AwaitingBirthEvidence
                | Phase::CleaningOwnedTree
                | Phase::VerifyingCleanup
        ) {
            return Err(GuardianLoopError::Policy(LoopFailure::RecoveryNotAdmitted));
        }
        let permit = ReconciledSessionPermit::after_committed_reconciliation(
            session,
            self.checkpoint.attempt_id(),
            self.checkpoint.deadline(),
            self.summary().revision,
        );
        guardian
            .finish_reconciliation(session, permit, now)
            .map_err(GuardianLoopError::Guardian)?;
        self.recovery_session = None;
        Ok(progress)
    }

    /// ACKs old evidence only by committed interruption, never applies it to fresh work.
    ///
    /// # Errors
    /// Requires the authenticated replacement session and a strictly older same-attempt key.
    pub fn classify_superseded_with_guardian<G: GuardianExecutor>(
        &self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        key: DispatchKey,
    ) -> Result<DispatchKey, GuardianLoopError<Infallible>> {
        self.check_instance(guardian)?;
        guardian
            .authenticate_policy_session(
                session,
                self.checkpoint.attempt_id(),
                self.checkpoint.deadline(),
            )
            .map_err(GuardianLoopError::Guardian)?;
        let executed = guardian
            .retained_result(key)
            .ok_or(GuardianLoopError::Guardian(
                GuardianError::ResultNotRetained,
            ))?;
        self.classify_superseded_retained(executed)
            .map_err(GuardianLoopError::from)?;
        guardian
            .ack_result(session, AppliedResultAck::after_applied(session, key))
            .map_err(GuardianLoopError::Guardian)?;
        Ok(key)
    }

    fn check_bridge<G: GuardianExecutor, E>(
        &self,
        guardian: &Guardian<G>,
        session: ControllerSession,
    ) -> Result<(), GuardianLoopError<E>> {
        self.check_binding(guardian, session)?;
        guardian
            .authenticate_policy_session(
                session,
                self.checkpoint.attempt_id(),
                self.checkpoint.deadline(),
            )
            .map_err(GuardianLoopError::Guardian)
    }

    fn check_binding<G: GuardianExecutor, E>(
        &self,
        guardian: &Guardian<G>,
        session: ControllerSession,
    ) -> Result<(), GuardianLoopError<E>> {
        self.check_instance(guardian)?;
        guardian
            .authenticate_policy_binding(
                session,
                self.checkpoint.attempt_id(),
                self.checkpoint.deadline(),
            )
            .map_err(GuardianLoopError::Guardian)?;
        if self.bound_session != Some(session) {
            return Err(GuardianLoopError::Guardian(GuardianError::StaleSession));
        }
        Ok(())
    }

    fn check_instance<G: GuardianExecutor, E>(
        &self,
        guardian: &Guardian<G>,
    ) -> Result<(), GuardianLoopError<E>> {
        let bound = self
            .bound_guardian_instance
            .ok_or(GuardianLoopError::Policy(LoopFailure::GuardianNotBound))?;
        if bound != guardian.instance_id() {
            return Err(GuardianLoopError::Guardian(GuardianError::InstanceMismatch));
        }
        Ok(())
    }

    fn coordinate_result<G: GuardianExecutor, E>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        result: Result<LoopProgress, LoopError<E>>,
    ) -> Result<LoopProgress, GuardianLoopError<E>> {
        result.map_err(|error| {
            let must_fence = matches!(error, LoopError::Persistence(_) | LoopError::FailStopped);
            let error = GuardianLoopError::from(error);
            if must_fence {
                self.fail_stop_guardian(guardian, session, error)
            } else {
                error
            }
        })
    }

    fn fail_stop_guardian<G: GuardianExecutor, E>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        error: GuardianLoopError<E>,
    ) -> GuardianLoopError<E> {
        self.status = LoopStatus::FailStopped;
        self.rounds.clear();
        match guardian.authority_lost(session) {
            Ok(()) => error,
            Err(fencing) => GuardianLoopError::FencingFailed {
                cause: Box::new(error),
                fencing,
            },
        }
    }

    fn ack_progress<G: GuardianExecutor, E>(
        &mut self,
        guardian: &mut Guardian<G>,
        session: ControllerSession,
        progress: &LoopProgress,
    ) -> Result<(), GuardianLoopError<E>> {
        let keys: Vec<_> = progress.acknowledge_keys().collect();
        for key in &keys {
            if guardian.retained_result(*key).is_none() {
                return Err(self.fail_stop_guardian(
                    guardian,
                    session,
                    GuardianLoopError::Guardian(GuardianError::ResultNotRetained),
                ));
            }
        }
        for key in keys {
            if let Err(error) =
                guardian.ack_result(session, AppliedResultAck::after_applied(session, key))
            {
                return Err(self.fail_stop_guardian(
                    guardian,
                    session,
                    GuardianLoopError::Guardian(error),
                ));
            }
        }
        Ok(())
    }

    fn plan_completion<E>(
        &self,
        key: DispatchKey,
        completion: &EffectCompletion,
    ) -> Result<(Transition, Receipt), LoopError<E>> {
        let checkpoint = self.checkpoint.clone();
        let mut receipt = Receipt::Applied;
        let transition = match completion {
            EffectCompletion::SampleResources {
                result: Ok(sample), ..
            } => checkpoint.record_resource_sample(*sample),
            EffectCompletion::ListChildren {
                parent,
                result: Ok(pids),
            } => checkpoint.record_child_list(*parent, pids.clone()),
            EffectCompletion::AcquireChildIdentity {
                discovery,
                observation,
            } => checkpoint.child_observed(*discovery, *observation),
            EffectCompletion::ObserveIdentity {
                identity,
                observation,
            } => checkpoint.identity_observed(*identity, *observation),
            EffectCompletion::ReverifyRetainedCandidate { verification, .. } => {
                checkpoint.record_candidate_verification(*verification)
            }
            EffectCompletion::SampleResources {
                sequence,
                result: Err(failure),
            } => {
                receipt = Receipt::Parked(ParkReason::ResourceObservationFailure {
                    sequence: *sequence,
                    failure: *failure,
                });
                Ok(checkpoint.checkpoint_only())
            }
            EffectCompletion::ListChildren {
                parent,
                result: Err(failure),
            } => {
                receipt = Receipt::Parked(ParkReason::ChildQueryFailure {
                    parent: *parent,
                    failure: *failure,
                });
                Ok(checkpoint.checkpoint_only())
            }
            EffectCompletion::SignalExactIdentity {
                identity,
                signal,
                outcome,
            } => {
                receipt = Receipt::Signal(SignalResult {
                    key,
                    identity: *identity,
                    signal: *signal,
                    outcome: *outcome,
                });
                Ok(checkpoint.checkpoint_only())
            }
        }
        .map_err(LoopError::Transition)?;
        Ok((transition, receipt))
    }

    fn require_running<E>(&self) -> Result<(), LoopError<E>> {
        match self.status {
            LoopStatus::Running => Ok(()),
            LoopStatus::Parked(reason) => Err(LoopError::Parked(reason)),
            LoopStatus::FailStopped => Err(LoopError::FailStopped),
        }
    }

    fn require_idle<E>(&self) -> Result<(), LoopError<E>> {
        self.require_running()?;
        if self.rounds.is_empty() {
            Ok(())
        } else {
            Err(LoopError::RoundInFlight)
        }
    }

    fn advance<S: AttemptStore>(
        &mut self,
        store: &mut S,
        step: impl FnOnce(DeploymentAttempt) -> Result<Transition, TransitionError>,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        self.require_idle()?;
        self.replace_with(store, step, None)
    }

    fn replace_with<S: AttemptStore>(
        &mut self,
        store: &mut S,
        step: impl FnOnce(DeploymentAttempt) -> Result<Transition, TransitionError>,
        fence: Option<LoopFenceReason>,
    ) -> Result<LoopProgress, LoopError<S::Error>> {
        if self.status == LoopStatus::FailStopped {
            return Err(LoopError::FailStopped);
        }
        let transition = step(self.checkpoint.clone()).map_err(LoopError::Transition)?;
        let committed = self.persist(transition, store)?;
        let has_signals = has_signal_effects(&committed);
        Ok(self.replace(committed, has_signals, fence))
    }

    fn persist<S: AttemptStore>(
        &mut self,
        transition: Transition,
        store: &mut S,
    ) -> Result<CommittedTransition, LoopError<S::Error>> {
        transition.persist(store).map_err(|error| {
            self.rounds.clear();
            self.status = LoopStatus::FailStopped;
            LoopError::Persistence(error)
        })
    }

    fn replace(
        &mut self,
        committed: CommittedTransition,
        signal_round: bool,
        fence: Option<LoopFenceReason>,
    ) -> LoopProgress {
        let revision = committed_revision(&committed);
        if committed.checkpoint().phase() == Phase::Interrupted {
            self.interrupted_revision = Some(revision);
            self.recovery_session = None;
        }
        let (checkpoint, batch) = committed.into_dispatch();
        self.checkpoint = checkpoint;
        self.rounds.clear();
        self.status = LoopStatus::Running;
        self.enqueue(batch, signal_round);
        let phase = self.checkpoint.phase();
        match fence {
            Some(reason) => LoopProgress::GuardianMustFence {
                revision,
                phase,
                reason,
            },
            None => LoopProgress::Advanced { revision, phase },
        }
    }

    fn enqueue(&mut self, batch: DispatchBatch, signal_round: bool) {
        if !batch.is_empty() || signal_round {
            self.rounds.push_back(Round {
                dispatch_revision: self
                    .checkpoint
                    .persistence_revision()
                    .expect("committed dispatch revision"),
                unissued: batch,
                returned: None,
                issued: Vec::new(),
                signals: signal_round.then(Vec::new),
            });
        }
    }
}

fn has_signal_effects(committed: &CommittedTransition) -> bool {
    committed
        .after_persist()
        .iter()
        .any(|effect| matches!(effect, OsEffect::SignalExactIdentity { .. }))
}

fn committed_revision(committed: &CommittedTransition) -> PersistenceRevision {
    committed
        .checkpoint()
        .persistence_revision()
        .expect("committed checkpoint has a revision")
}
