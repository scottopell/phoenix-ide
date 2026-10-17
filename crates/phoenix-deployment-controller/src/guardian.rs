//! Bounded, in-memory custody for one attempt and its original absolute deadline.
//!
//! This is not a native supervisor or a reconnect transport. Runtime integration
//! must commit interruption, reconcile fresh observations, and apply results before
//! minting the crate-private permits. Instance IDs are unique only in this process;
//! they cannot reconstruct custody after process loss.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    mem::ManuallyDrop,
    num::{NonZeroU64, NonZeroUsize},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    AttemptId, DispatchKey, DispatchTicket, ExecutedEffect, MonotonicTick, OsEffect,
    PersistenceRevision, ProcessIdentity,
};

pub const MAX_CHILD_SLOTS: usize = 64;
pub const MAX_RETAINED_RESULTS: usize = crate::MAX_OWNED_SIGNAL_BATCH;

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// In-process correlation, not a PID, an adoption token, or restart authority.
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::GuardianInstanceId;
/// let _ = GuardianInstanceId(std::num::NonZeroU64::MIN);
/// ```
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GuardianInstanceId(NonZeroU64);

impl GuardianInstanceId {
    fn mint() -> Result<Self, GuardianError> {
        NEXT_INSTANCE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()
            .and_then(NonZeroU64::new)
            .map(Self)
            .ok_or(GuardianError::InstanceIdExhausted)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Only a guardian mints sessions. Generation never wraps or returns to zero.
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::{ControllerSession, GuardianInstanceId};
/// use std::num::NonZeroU64;
/// fn forge(instance: GuardianInstanceId) -> ControllerSession {
///     ControllerSession { instance, generation: NonZeroU64::MIN }
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerSession {
    instance: GuardianInstanceId,
    generation: NonZeroU64,
}

impl ControllerSession {
    #[must_use]
    pub const fn instance_id(self) -> GuardianInstanceId {
        self.instance
    }

    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation.get()
    }

    fn checked_next(self) -> Result<Self, GuardianError> {
        self.generation
            .checked_add(1)
            .map(|generation| Self { generation, ..self })
            .ok_or(GuardianError::SessionGenerationExhausted)
    }
}

/// A guardian-local slot name. It exposes no native handle or process identity.
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::{ChildSlotId, GuardianInstanceId};
/// fn forge(instance: GuardianInstanceId) -> ChildSlotId {
///     ChildSlotId { instance, ordinal: std::num::NonZeroUsize::MIN }
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChildSlotId {
    instance: GuardianInstanceId,
    ordinal: NonZeroUsize,
}

impl ChildSlotId {
    #[must_use]
    pub const fn instance_id(self) -> GuardianInstanceId {
        self.instance
    }

    #[must_use]
    pub const fn ordinal(self) -> usize {
        self.ordinal.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardianState {
    Active,
    DetachedFenced,
    Reconciling,
    AuthorityLost,
}

/// Observation only: neither an exit observation nor unavailability releases custody.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildSlotObservation {
    Running,
    Exited,
    Unavailable,
}

/// Trusted execution boundary. There is deliberately no cleanup, adoption, handle
/// extraction, or cancellation operation on this interface.
pub trait GuardianExecutor {
    type ChildSlot;

    /// Consume the ticket through the existing affine executor, returning its
    /// actual correlated result. The slots are borrowed, never transferred.
    #[must_use]
    fn execute(
        &mut self,
        ticket: DispatchTicket,
        slots: &mut BTreeMap<ChildSlotId, Self::ChildSlot>,
    ) -> ExecutedEffect;

    /// Diagnostic native identity of a retained slot, if available. No handle or
    /// execution authority is transferred.
    fn diagnostic_slot_identity(&self, _slot: &Self::ChildSlot) -> Option<ProcessIdentity> {
        None
    }

    /// Trusted semantic contract: do not signal, reap, finalize, remove, replace,
    /// or otherwise mutate a slot or start follow-up work. A shared borrow alone
    /// cannot enforce this against interior mutability or native side effects.
    #[must_use]
    fn poll_observation_only(&self, slot: &Self::ChildSlot) -> ChildSlotObservation;
}

/// Affine execution authority for exactly the session that received the ticket.
/// Only the runtime may bind a raw ticket; callers cannot relabel a bound ticket.
///
/// ```compile_fail
/// use phoenix_deployment_controller::DispatchTicket;
/// use phoenix_deployment_controller::guardian::{ControllerSession, SessionDispatchTicket};
/// fn forge(session: ControllerSession, ticket: DispatchTicket) -> SessionDispatchTicket {
///     SessionDispatchTicket { session, ticket }
/// }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::SessionDispatchTicket;
/// fn duplicate(ticket: SessionDispatchTicket) { let _ = ticket.clone(); }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::DispatchTicket;
/// use phoenix_deployment_controller::guardian::{ControllerSession, Guardian, GuardianExecutor};
/// fn relabel<E: GuardianExecutor>(guardian: &Guardian<E>, session: ControllerSession, raw: DispatchTicket) {
///     let _ = guardian.bind_ticket(session, raw);
/// }
/// ```
#[must_use]
#[derive(Debug)]
pub struct SessionDispatchTicket {
    session: ControllerSession,
    ticket: DispatchTicket,
}

impl SessionDispatchTicket {
    #[must_use]
    pub const fn session(&self) -> ControllerSession {
        self.session
    }

    #[must_use]
    pub const fn key(&self) -> DispatchKey {
        self.ticket.key()
    }

    #[must_use]
    pub const fn effect(&self) -> &OsEffect {
        self.ticket.effect()
    }
}

/// Pre-binding refusal returns the original, unexecuted affine ticket.
#[must_use]
#[derive(Debug)]
pub struct TicketBindingRefusal {
    error: GuardianError,
    ticket: DispatchTicket,
}

impl TicketBindingRefusal {
    #[must_use]
    pub const fn error(&self) -> GuardianError {
        self.error
    }

    #[must_use]
    pub fn into_parts(self) -> (GuardianError, DispatchTicket) {
        let Self { error, ticket } = self;
        (error, ticket)
    }
}

/// Pre-execution refusal preserves the token and its original session binding.
#[must_use]
#[derive(Debug)]
pub struct BoundSubmissionRefusal {
    error: GuardianError,
    token: SessionDispatchTicket,
}

impl BoundSubmissionRefusal {
    #[must_use]
    pub const fn error(&self) -> GuardianError {
        self.error
    }

    pub fn into_parts(self) -> (GuardianError, SessionDispatchTicket) {
        let Self { error, token } = self;
        (error, token)
    }
}

#[must_use]
#[derive(Debug)]
pub enum BoundSubmissionError {
    Refused(Box<BoundSubmissionRefusal>),
    /// Execution consumed the ticket; the actual result is quarantined.
    ExecutorCorrelationMismatch,
}

impl BoundSubmissionError {
    #[must_use]
    pub const fn error(&self) -> GuardianError {
        match self {
            Self::Refused(refusal) => refusal.error(),
            Self::ExecutorCorrelationMismatch => GuardianError::ExecutorCorrelationMismatch,
        }
    }
}

pub(crate) enum PolicySubmissionError {
    Refused {
        error: GuardianError,
        ticket: DispatchTicket,
    },
    ExecutorCorrelationMismatch,
}

/// A mismatched completion is diagnostic evidence, never a normally keyed result.
#[derive(Debug)]
pub struct QuarantinedResult {
    requested_key: DispatchKey,
    requested_effect: OsEffect,
    actual: ExecutedEffect,
}

impl QuarantinedResult {
    #[must_use]
    pub const fn requested_key(&self) -> DispatchKey {
        self.requested_key
    }

    #[must_use]
    pub const fn requested_effect(&self) -> &OsEffect {
        &self.requested_effect
    }

    #[must_use]
    pub const fn actual_result(&self) -> &ExecutedEffect {
        &self.actual
    }
}

/// Runtime proof that interruption and the fresh reconciliation barrier were
/// committed for this exact session, attempt, and original deadline.
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::ReconciledSessionPermit;
/// fn duplicate(permit: ReconciledSessionPermit) { let _ = permit.clone(); }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::{AttemptId, MonotonicTick, PersistenceRevision};
/// use phoenix_deployment_controller::guardian::{ControllerSession, ReconciledSessionPermit};
/// fn forge(session: ControllerSession, attempt: AttemptId, deadline: MonotonicTick, revision: PersistenceRevision) {
///     let _ = ReconciledSessionPermit::after_committed_reconciliation(session, attempt, deadline, revision);
/// }
/// ```
#[must_use]
#[derive(Debug)]
pub struct ReconciledSessionPermit {
    session: ControllerSession,
    attempt: AttemptId,
    deadline: MonotonicTick,
    revision: PersistenceRevision,
}

/// Affine live preparation. Only the guardian can construct this capability.
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::PreparedReconciliationActivation;
/// fn duplicate(p: PreparedReconciliationActivation) { let _ = p.clone(); }
/// ```
#[derive(Debug)]
pub struct PreparedReconciliationActivation {
    permit: ReconciledSessionPermit,
    floor: PersistenceRevision,
}

impl ReconciledSessionPermit {
    /// Runtime integration calls this only after committing interruption and a
    /// complete fresh reconciliation, without extending the original deadline.
    pub(crate) const fn after_committed_reconciliation(
        session: ControllerSession,
        attempt: AttemptId,
        deadline: MonotonicTick,
        revision: PersistenceRevision,
    ) -> Self {
        Self {
            session,
            attempt,
            deadline,
            revision,
        }
    }
}

/// Runtime proof that the keyed result has been durably applied in this session.
/// Instance binding is carried by the session, not a second instance field.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchKey};
/// use phoenix_deployment_controller::guardian::{AppliedResultAck, ControllerSession};
/// fn forge(session: ControllerSession, key: DispatchKey) -> AppliedResultAck {
///     AppliedResultAck { session, key }
/// }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::AppliedResultAck;
/// fn duplicate(ack: AppliedResultAck) { let _ = ack.clone(); }
/// ```
#[must_use]
#[derive(Debug)]
pub struct AppliedResultAck {
    session: ControllerSession,
    key: DispatchKey,
}

impl AppliedResultAck {
    /// Runtime integration calls this only after durably applying this result.
    pub(crate) const fn after_applied(session: ControllerSession, key: DispatchKey) -> Self {
        Self { session, key }
    }
}

/// Closed categories only; no native error, path, or process data is exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardianError {
    InstanceIdExhausted,
    SessionGenerationExhausted,
    ChildSlotCapacityExceeded,
    RetainedResultCapacityExceeded,
    StaleSession,
    StaleDispatchRevision,
    NotActive,
    NotDetached,
    NotReconciling,
    SessionFenced,
    AttemptMismatch,
    InstanceMismatch,
    InventoryMismatch,
    PermitMismatch,
    AckMismatch,
    DuplicateDispatchKey,
    ResultNotRetained,
    UnknownChildSlot,
    DeadlineExpired,
    AuthorityLost,
    ExecutorCorrelationMismatch,
}

impl fmt::Display for GuardianError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InstanceIdExhausted => "guardian_instance_id_exhausted",
            Self::SessionGenerationExhausted => "guardian_session_generation_exhausted",
            Self::ChildSlotCapacityExceeded => "guardian_child_slot_capacity_exceeded",
            Self::RetainedResultCapacityExceeded => "guardian_retained_result_capacity_exceeded",
            Self::StaleSession => "guardian_stale_session",
            Self::StaleDispatchRevision => "guardian_stale_dispatch_revision",
            Self::NotActive => "guardian_not_active",
            Self::NotDetached => "guardian_not_detached",
            Self::NotReconciling => "guardian_not_reconciling",
            Self::SessionFenced => "guardian_session_fenced",
            Self::AttemptMismatch => "guardian_attempt_mismatch",
            Self::InstanceMismatch => "guardian_instance_mismatch",
            Self::InventoryMismatch => "guardian_inventory_mismatch",
            Self::PermitMismatch => "guardian_permit_mismatch",
            Self::AckMismatch => "guardian_ack_mismatch",
            Self::DuplicateDispatchKey => "guardian_duplicate_dispatch_key",
            Self::ResultNotRetained => "guardian_result_not_retained",
            Self::UnknownChildSlot => "guardian_unknown_child_slot",
            Self::DeadlineExpired => "guardian_deadline_expired",
            Self::AuthorityLost => "guardian_authority_lost",
            Self::ExecutorCorrelationMismatch => "guardian_executor_correlation_mismatch",
        })
    }
}

impl std::error::Error for GuardianError {}

/// Rejected construction keeps every input reachable until explicit recovery.
/// Dropping this wrapper never calls executor or slot destructors; callers must
/// use `into_parts` to recover custody instead of discarding the failure.
#[must_use]
pub struct GuardianBuildFailure<E: GuardianExecutor> {
    error: GuardianError,
    executor: ManuallyDrop<E>,
    slots: ManuallyDrop<Vec<E::ChildSlot>>,
}

impl<E: GuardianExecutor> GuardianBuildFailure<E> {
    fn new(error: GuardianError, executor: E, slots: Vec<E::ChildSlot>) -> Self {
        Self {
            error,
            executor: ManuallyDrop::new(executor),
            slots: ManuallyDrop::new(slots),
        }
    }

    #[must_use]
    pub const fn error(&self) -> GuardianError {
        self.error
    }

    #[must_use]
    pub fn into_parts(self) -> (GuardianError, E, Vec<E::ChildSlot>) {
        let Self {
            error,
            executor,
            slots,
        } = self;
        (
            error,
            ManuallyDrop::into_inner(executor),
            ManuallyDrop::into_inner(slots),
        )
    }
}

impl<E: GuardianExecutor> fmt::Debug for GuardianBuildFailure<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GuardianBuildFailure")
            .field("error", &self.error)
            .field("slot_count", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CustodySummary {
    pub instance: GuardianInstanceId,
    pub attempt: AttemptId,
    pub original_deadline: MonotonicTick,
    pub state: GuardianState,
    pub child_slots: usize,
    pub retained_results: usize,
    pub quarantined_result: bool,
}

/// Explicitly forfeited execution authority, retaining opaque custody and evidence.
/// No native finalization is implemented, and no implicit cleanup is performed.
#[must_use]
pub struct ForfeitedCustody<E: GuardianExecutor> {
    guardian: Guardian<E>,
}

impl<E: GuardianExecutor> ForfeitedCustody<E> {
    #[must_use]
    pub fn summary(&self) -> CustodySummary {
        self.guardian.summary()
    }

    pub fn inventory(&self) -> impl ExactSizeIterator<Item = ChildSlotId> + '_ {
        self.guardian.inventory()
    }

    pub fn retained_results(&self) -> impl ExactSizeIterator<Item = &ExecutedEffect> {
        self.guardian.retained_results()
    }

    #[must_use]
    pub const fn quarantined_result(&self) -> Option<&QuarantinedResult> {
        self.guardian.quarantined_result()
    }
}

/// Custody cannot be cloned, extracted, or implicitly cleaned up.
///
/// Executor and slots are `ManuallyDrop`: their destructors cannot perform native
/// cleanup through this boundary. Keep the guardian or its explicit `forfeit`
/// result reachable. Native finalization and native guardian integration are not
/// implemented here; dropping custody cannot claim cleanup.
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::{Guardian, GuardianExecutor};
/// fn duplicate<E: GuardianExecutor>(guardian: Guardian<E>) { let _ = guardian.clone(); }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::{Guardian, GuardianExecutor};
/// fn extract<E: GuardianExecutor>(guardian: Guardian<E>) { let _ = guardian.slots; }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian::{Guardian, GuardianExecutor};
/// fn extract<E: GuardianExecutor>(guardian: Guardian<E>) { let _ = guardian.into_parts(); }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SignalResultReservation {
    session: ControllerSession,
    revision: PersistenceRevision,
    remaining: usize,
}

#[must_use]
pub struct Guardian<E: GuardianExecutor> {
    session: ControllerSession,
    state: GuardianState,
    attempt: AttemptId,
    deadline: MonotonicTick,
    minimum_dispatch_revision: Option<PersistenceRevision>,
    reconciliation_floor: Option<PersistenceRevision>,
    latest_reconciliation_revision: Option<PersistenceRevision>,
    signal_result_reservation: Option<SignalResultReservation>,
    executor: ManuallyDrop<E>,
    slots: ManuallyDrop<BTreeMap<ChildSlotId, E::ChildSlot>>,
    retained: Vec<ExecutedEffect>,
    quarantine: Option<QuarantinedResult>,
}

impl<E: GuardianExecutor> Guardian<E> {
    /// Accept opaque, already-owned slots; mint the instance and its first session.
    /// No PID/PGID, label, socket, or historical inventory can construct a slot here.
    ///
    /// # Errors
    /// Returns a capacity or instance-exhaustion failure with all original inputs.
    /// No executor calls or native cleanup occur.
    ///
    /// # Panics
    /// Panics if a bounded slot index cannot fit a nonzero ordinal.
    pub fn new(
        attempt: AttemptId,
        original_deadline: MonotonicTick,
        executor: E,
        initial_slots: Vec<E::ChildSlot>,
    ) -> Result<(Self, ControllerSession), GuardianBuildFailure<E>> {
        Self::new_with_instance_source(
            attempt,
            original_deadline,
            executor,
            initial_slots,
            GuardianInstanceId::mint,
        )
    }

    fn new_with_instance_source(
        attempt: AttemptId,
        original_deadline: MonotonicTick,
        executor: E,
        initial_slots: Vec<E::ChildSlot>,
        mint_instance: fn() -> Result<GuardianInstanceId, GuardianError>,
    ) -> Result<(Self, ControllerSession), GuardianBuildFailure<E>> {
        if initial_slots.len() > MAX_CHILD_SLOTS {
            return Err(GuardianBuildFailure::new(
                GuardianError::ChildSlotCapacityExceeded,
                executor,
                initial_slots,
            ));
        }
        let instance = match mint_instance() {
            Ok(instance) => instance,
            Err(error) => return Err(GuardianBuildFailure::new(error, executor, initial_slots)),
        };
        let session = ControllerSession {
            instance,
            generation: NonZeroU64::MIN,
        };
        let mut slots = BTreeMap::new();
        for (index, slot) in initial_slots.into_iter().enumerate() {
            let ordinal = NonZeroUsize::MIN
                .checked_add(index)
                .expect("bounded child slot ordinal");
            slots.insert(ChildSlotId { instance, ordinal }, slot);
        }
        Ok((
            Self {
                session,
                state: GuardianState::Active,
                attempt,
                deadline: original_deadline,
                minimum_dispatch_revision: None,
                reconciliation_floor: None,
                latest_reconciliation_revision: None,
                signal_result_reservation: None,
                executor: ManuallyDrop::new(executor),
                slots: ManuallyDrop::new(slots),
                retained: Vec::with_capacity(MAX_RETAINED_RESULTS),
                quarantine: None,
            },
            session,
        ))
    }

    /// Revoke this object's execution surface without native cleanup or extraction.
    #[must_use = "keep forfeited custody reachable; native finalization is not implemented"]
    pub fn forfeit(mut self) -> ForfeitedCustody<E> {
        self.lose_authority();
        ForfeitedCustody { guardian: self }
    }

    #[must_use]
    pub fn summary(&self) -> CustodySummary {
        CustodySummary {
            instance: self.instance_id(),
            attempt: self.attempt,
            original_deadline: self.deadline,
            state: self.state,
            child_slots: self.slots.len(),
            retained_results: self.retained.len(),
            quarantined_result: self.quarantine.is_some(),
        }
    }

    #[must_use]
    pub const fn instance_id(&self) -> GuardianInstanceId {
        self.session.instance_id()
    }

    #[must_use]
    pub const fn state(&self) -> GuardianState {
        self.state
    }

    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.attempt
    }

    #[must_use]
    pub const fn original_deadline(&self) -> MonotonicTick {
        self.deadline
    }

    #[must_use]
    pub const fn reconciliation_floor(&self) -> Option<PersistenceRevision> {
        self.reconciliation_floor
    }

    pub fn inventory(&self) -> impl ExactSizeIterator<Item = ChildSlotId> + '_ {
        self.slots.keys().copied()
    }

    /// Ordered native identities and liveness, borrowed from original custody.
    /// Missing identities remain explicit rather than being silently omitted.
    #[must_use]
    pub fn diagnostic_slots(&self) -> Vec<(Option<ProcessIdentity>, ChildSlotObservation)> {
        self.slots
            .values()
            .map(|slot| {
                (
                    self.executor.diagnostic_slot_identity(slot),
                    self.executor.poll_observation_only(slot),
                )
            })
            .collect()
    }

    #[must_use]
    pub fn retained_result(&self, key: DispatchKey) -> Option<&ExecutedEffect> {
        self.retained.iter().find(|result| result.key() == key)
    }

    pub fn retained_results(&self) -> impl ExactSizeIterator<Item = &ExecutedEffect> {
        self.retained.iter()
    }

    /// Capacity check for the runtime before issuing another affine ticket.
    /// Binding is not a reservation: submission checks capacity again.
    #[must_use]
    pub fn can_retain_result(&self) -> bool {
        self.retained.len() < MAX_RETAINED_RESULTS
    }

    /// At most one mismatch can occur, because it permanently loses authority.
    #[must_use]
    pub const fn quarantined_result(&self) -> Option<&QuarantinedResult> {
        self.quarantine.as_ref()
    }

    /// Available even when detached or authority is permanently lost.
    ///
    /// # Errors
    /// Returns [`GuardianError::UnknownChildSlot`] for a slot outside this custody.
    pub fn poll_observation_only(
        &self,
        slot: ChildSlotId,
    ) -> Result<ChildSlotObservation, GuardianError> {
        let slot = self
            .slots
            .get(&slot)
            .ok_or(GuardianError::UnknownChildSlot)?;
        Ok(self.executor.poll_observation_only(slot))
    }

    /// Runtime-only binding, before handing any ticket to an external caller.
    /// Deadline is checked at submission, not when issuing this token.
    ///
    /// # Errors
    /// Preserves the original raw ticket on every refusal.
    #[cfg(test)]
    pub(crate) fn bind_ticket(
        &self,
        session: ControllerSession,
        ticket: DispatchTicket,
    ) -> Result<SessionDispatchTicket, TicketBindingRefusal> {
        if let Err(error) = self.require_dispatch(session, ticket.key(), ticket.effect()) {
            return Err(TicketBindingRefusal { error, ticket });
        }
        Ok(SessionDispatchTicket { session, ticket })
    }

    /// Consume session-bound authority, retaining a normally correlated result.
    /// Duplicate keys are refused only while their result is retained; this is
    /// not a lifetime or cross-crash exactly-once guarantee.
    ///
    /// # Errors
    /// Every pre-execution refusal returns the unchanged bound token. Execution
    /// mismatches lose authority and retain separate quarantine evidence instead.
    pub fn submit_bound(
        &mut self,
        token: SessionDispatchTicket,
        now: MonotonicTick,
    ) -> Result<DispatchKey, BoundSubmissionError> {
        if let Err(error) = self.require_dispatch(token.session, token.key(), token.effect()) {
            return Err(BoundSubmissionError::Refused(Box::new(
                BoundSubmissionRefusal { error, token },
            )));
        }
        if let Err(error) = self.require_deadline(now) {
            return Err(BoundSubmissionError::Refused(Box::new(
                BoundSubmissionRefusal { error, token },
            )));
        }
        let SessionDispatchTicket { ticket, session } = token;
        let key = ticket.key();
        let effect = ticket.effect().clone();
        let result = self.executor.execute(ticket, &mut self.slots);
        if result.key() != key || result.completion().effect() != effect {
            self.quarantine = Some(QuarantinedResult {
                requested_key: key,
                requested_effect: effect,
                actual: result,
            });
            self.lose_authority();
            return Err(BoundSubmissionError::ExecutorCorrelationMismatch);
        }
        self.consume_signal_reservation(session, key, &effect);
        self.retained.push(result);
        Ok(key)
    }

    /// Reserves custody for an entire committed signal round before its first effect.
    /// Repeating the exact reservation is idempotent while the round is in flight.
    ///
    /// # Errors
    /// Requires the current active session, a non-stale dispatch revision, and enough
    /// unreserved result capacity for every remaining signal.
    pub(crate) fn reserve_signal_results(
        &mut self,
        session: ControllerSession,
        revision: PersistenceRevision,
        count: usize,
    ) -> Result<(), GuardianError> {
        self.require_session(session)?;
        self.require_state(GuardianState::Active, GuardianError::NotActive)?;
        if self
            .minimum_dispatch_revision
            .is_some_and(|minimum| revision < minimum)
        {
            return Err(GuardianError::StaleDispatchRevision);
        }
        if let Some(existing) = self.signal_result_reservation {
            return if existing.session == session && existing.revision == revision {
                Ok(())
            } else {
                Err(GuardianError::RetainedResultCapacityExceeded)
            };
        }
        if count > MAX_RETAINED_RESULTS.saturating_sub(self.retained.len()) {
            return Err(GuardianError::RetainedResultCapacityExceeded);
        }
        if count != 0 {
            self.signal_result_reservation = Some(SignalResultReservation {
                session,
                revision,
                remaining: count,
            });
        }
        Ok(())
    }

    /// # Errors
    /// Refuses stale session, deadline, key, revision, or unavailable result custody.
    pub(crate) fn validate_native_sampling_admission(
        &self,
        session: ControllerSession,
        key: DispatchKey,
        sequence: u64,
        now: MonotonicTick,
    ) -> Result<(), GuardianError> {
        self.require_dispatch(session, key, &OsEffect::SampleResources { sequence })?;
        if now >= self.deadline {
            return Err(GuardianError::DeadlineExpired);
        }
        Ok(())
    }

    pub(crate) fn submit_native_sampling(
        &mut self,
        admission: crate::guardian_ipc::NativeSamplingAdmission,
        now: MonotonicTick,
    ) -> Result<DispatchKey, PolicySubmissionError> {
        let (session, ticket) = admission.into_ticket();
        self.submit_policy_ticket(session, ticket, now)
    }

    /// Atomic bridge: a raw ticket never escapes with a new public session label.
    pub(crate) fn submit_policy_ticket(
        &mut self,
        session: ControllerSession,
        ticket: DispatchTicket,
        now: MonotonicTick,
    ) -> Result<DispatchKey, PolicySubmissionError> {
        match self.submit_bound(SessionDispatchTicket { session, ticket }, now) {
            Ok(key) => Ok(key),
            Err(BoundSubmissionError::Refused(refusal)) => {
                let (error, token) = (*refusal).into_parts();
                Err(PolicySubmissionError::Refused {
                    error,
                    ticket: token.ticket,
                })
            }
            Err(BoundSubmissionError::ExecutorCorrelationMismatch) => {
                Err(PolicySubmissionError::ExecutorCorrelationMismatch)
            }
        }
    }

    pub(crate) fn authenticate_policy_session(
        &self,
        session: ControllerSession,
        attempt: AttemptId,
        deadline: MonotonicTick,
    ) -> Result<(), GuardianError> {
        self.authenticate_policy_binding(session, attempt, deadline)?;
        match self.state {
            GuardianState::Active | GuardianState::Reconciling => Ok(()),
            GuardianState::DetachedFenced => Err(GuardianError::SessionFenced),
            GuardianState::AuthorityLost => Err(GuardianError::AuthorityLost),
        }
    }

    pub(crate) fn authenticate_policy_binding(
        &self,
        session: ControllerSession,
        attempt: AttemptId,
        deadline: MonotonicTick,
    ) -> Result<(), GuardianError> {
        self.require_session(session)?;
        if attempt != self.attempt {
            return Err(GuardianError::AttemptMismatch);
        }
        if deadline != self.deadline {
            return Err(GuardianError::PermitMismatch);
        }
        Ok(())
    }

    fn require_dispatch(
        &self,
        session: ControllerSession,
        key: DispatchKey,
        effect: &OsEffect,
    ) -> Result<(), GuardianError> {
        self.require_session(session)?;
        match (self.state, effect) {
            (GuardianState::Active, _)
            | (
                GuardianState::Reconciling,
                OsEffect::SampleResources { .. }
                | OsEffect::ListChildren { .. }
                | OsEffect::AcquireChildIdentity { .. }
                | OsEffect::ObserveIdentity { .. }
                | OsEffect::ReverifyRetainedCandidate { .. },
            ) => {}
            (GuardianState::AuthorityLost, _) => return Err(GuardianError::AuthorityLost),
            _ => return Err(GuardianError::NotActive),
        }
        if key.attempt_id() != self.attempt {
            return Err(GuardianError::AttemptMismatch);
        }
        if self
            .minimum_dispatch_revision
            .is_some_and(|minimum| key.revision() < minimum)
            || self
                .reconciliation_floor
                .is_some_and(|floor| key.revision() < floor)
        {
            return Err(GuardianError::StaleDispatchRevision);
        }
        if self.retained_result(key).is_some() {
            return Err(GuardianError::DuplicateDispatchKey);
        }
        let reserved_for_ticket = matches!(effect, OsEffect::SignalExactIdentity { .. })
            && self.signal_result_reservation.is_some_and(|claim| {
                claim.session == session && claim.revision == key.revision() && claim.remaining != 0
            });
        let claimed_capacity = self
            .signal_result_reservation
            .map_or(0, |claim| claim.remaining);
        if !self.can_retain_result()
            || (!reserved_for_ticket
                && self.retained.len().saturating_add(claimed_capacity) >= MAX_RETAINED_RESULTS)
        {
            return Err(GuardianError::RetainedResultCapacityExceeded);
        }
        Ok(())
    }

    fn consume_signal_reservation(
        &mut self,
        session: ControllerSession,
        key: DispatchKey,
        effect: &OsEffect,
    ) {
        let Some(claim) = self.signal_result_reservation.as_mut() else {
            return;
        };
        if !matches!(effect, OsEffect::SignalExactIdentity { .. })
            || claim.session != session
            || claim.revision != key.revision()
        {
            return;
        }
        claim.remaining -= 1;
        if claim.remaining == 0 {
            self.signal_result_reservation = None;
        }
    }

    /// Fence exactly the current controller. No native calls or custody transfers.
    ///
    /// # Errors
    /// Returns stale-session or terminal-authority categories without changing custody.
    pub fn controller_lost(&mut self, session: ControllerSession) -> Result<(), GuardianError> {
        self.require_session(session)?;
        if self.state == GuardianState::AuthorityLost {
            return Err(GuardianError::AuthorityLost);
        }
        self.state = GuardianState::DetachedFenced;
        self.reconciliation_floor = None;
        self.signal_result_reservation = None;
        Ok(())
    }

    /// Positively match this instance and its complete inventory, in any order.
    /// Reconciliation admits observation tickets only, never process signals.
    /// Prefer `PolicyLoop::begin_guardian_reconciliation`, which supplies the
    /// revision from its committed interruption rather than caller metadata.
    ///
    /// # Errors
    /// Returns state, instance, inventory, stale-revision, or generation-exhaustion categories.
    pub fn begin_reconciliation(
        &mut self,
        expected_instance: GuardianInstanceId,
        expected_slots: &[ChildSlotId],
        committed_interruption_revision: PersistenceRevision,
    ) -> Result<ControllerSession, GuardianError> {
        self.require_state(GuardianState::DetachedFenced, GuardianError::NotDetached)?;
        if expected_instance != self.instance_id() {
            return Err(GuardianError::InstanceMismatch);
        }
        if expected_slots.len() != self.slots.len() {
            return Err(GuardianError::InventoryMismatch);
        }
        let expected: BTreeSet<_> = expected_slots.iter().copied().collect();
        if expected.len() != self.slots.len()
            || !self.slots.keys().all(|slot| expected.contains(slot))
        {
            return Err(GuardianError::InventoryMismatch);
        }
        if self
            .minimum_dispatch_revision
            .is_some_and(|minimum| committed_interruption_revision < minimum)
            || self
                .latest_reconciliation_revision
                .is_some_and(|latest| committed_interruption_revision <= latest)
        {
            return Err(GuardianError::StaleDispatchRevision);
        }
        let next = match self.session.checked_next() {
            Ok(next) => next,
            Err(error) => {
                self.lose_authority();
                return Err(error);
            }
        };
        self.session = next;
        self.state = GuardianState::Reconciling;
        self.reconciliation_floor = Some(committed_interruption_revision);
        self.latest_reconciliation_revision = Some(committed_interruption_revision);
        Ok(next)
    }

    /// Consume runtime reconciliation proof, checking the unchanged deadline last.
    ///
    /// # Errors
    /// Returns stale-session, state, permit, or deadline categories without native calls.
    pub fn finish_reconciliation(
        &mut self,
        session: ControllerSession,
        permit: ReconciledSessionPermit,
        now: MonotonicTick,
    ) -> Result<(), GuardianError> {
        let prepared = self.prepare_reconciliation(session, permit, now)?;
        self.activate_reconciliation(session, prepared, now)
    }

    /// Checks reconciliation without installing dispatch authority.
    /// # Errors
    /// Refuses a stale session, state, proof, floor, or expired original deadline.
    pub(crate) fn prepare_reconciliation(
        &mut self,
        session: ControllerSession,
        permit: ReconciledSessionPermit,
        now: MonotonicTick,
    ) -> Result<PreparedReconciliationActivation, GuardianError> {
        self.validate_reconciliation(session, &permit, now)?;
        Ok(PreparedReconciliationActivation {
            permit,
            floor: self
                .reconciliation_floor
                .ok_or(GuardianError::PermitMismatch)?,
        })
    }

    fn validate_reconciliation(
        &mut self,
        session: ControllerSession,
        permit: &ReconciledSessionPermit,
        now: MonotonicTick,
    ) -> Result<(), GuardianError> {
        self.require_session(session)?;
        self.require_state(GuardianState::Reconciling, GuardianError::NotReconciling)?;
        if permit.session != session
            || permit.attempt != self.attempt
            || permit.deadline != self.deadline
        {
            return Err(GuardianError::PermitMismatch);
        }
        let floor = self
            .reconciliation_floor
            .ok_or(GuardianError::PermitMismatch)?;
        if permit.revision < floor {
            return Err(GuardianError::StaleDispatchRevision);
        }
        self.require_deadline(now)
    }

    /// Consumes live preparation only after the caller has durable activation evidence.
    /// # Errors
    /// Rechecks exact session/state/proof and the original deadline.
    pub(crate) fn activate_reconciliation(
        &mut self,
        session: ControllerSession,
        prepared: PreparedReconciliationActivation,
        now: MonotonicTick,
    ) -> Result<(), GuardianError> {
        let PreparedReconciliationActivation { permit, floor } = prepared;
        if self.reconciliation_floor != Some(floor) {
            return Err(GuardianError::PermitMismatch);
        }
        self.validate_reconciliation(session, &permit, now)?;
        let revision = permit.revision;
        self.minimum_dispatch_revision = Some(
            self.minimum_dispatch_revision
                .map_or(revision, |minimum| minimum.max(revision)),
        );
        self.reconciliation_floor = None;
        self.state = GuardianState::Active;
        Ok(())
    }

    /// Forget only a result whose application the runtime has committed.
    /// No result or native slot is returned to the caller.
    ///
    /// # Errors
    /// Returns session, authority, fence, ack-binding, or missing-result categories.
    pub(crate) fn ack_result(
        &mut self,
        session: ControllerSession,
        AppliedResultAck {
            session: ack_session,
            key,
        }: AppliedResultAck,
    ) -> Result<(), GuardianError> {
        self.require_session(session)?;
        match self.state {
            GuardianState::Active | GuardianState::Reconciling => {}
            GuardianState::DetachedFenced => return Err(GuardianError::SessionFenced),
            GuardianState::AuthorityLost => return Err(GuardianError::AuthorityLost),
        }
        if ack_session != session || key.attempt_id() != self.attempt {
            return Err(GuardianError::AckMismatch);
        }
        let index = self
            .retained
            .iter()
            .position(|result| result.key() == key)
            .ok_or(GuardianError::ResultNotRetained)?;
        self.retained.swap_remove(index);
        Ok(())
    }

    /// Irreversible fail-stop. Inventory and retained results remain readable;
    /// this does not cancel, signal, finalize, or adopt anything.
    ///
    /// # Errors
    /// A stale session cannot fail-stop its replacement.
    pub fn authority_lost(&mut self, session: ControllerSession) -> Result<(), GuardianError> {
        self.require_session(session)?;
        self.lose_authority();
        Ok(())
    }

    fn lose_authority(&mut self) {
        self.state = GuardianState::AuthorityLost;
        self.reconciliation_floor = None;
        self.signal_result_reservation = None;
    }

    fn require_session(&self, session: ControllerSession) -> Result<(), GuardianError> {
        if session != self.session {
            return Err(GuardianError::StaleSession);
        }
        Ok(())
    }

    fn require_state(
        &self,
        expected: GuardianState,
        refused: GuardianError,
    ) -> Result<(), GuardianError> {
        if self.state == GuardianState::AuthorityLost {
            return Err(GuardianError::AuthorityLost);
        }
        if self.state != expected {
            return Err(refused);
        }
        Ok(())
    }

    fn require_deadline(&mut self, now: MonotonicTick) -> Result<(), GuardianError> {
        if now >= self.deadline {
            self.lose_authority();
            return Err(GuardianError::DeadlineExpired);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
        convert::Infallible,
        num::{NonZeroU32, NonZeroU64, NonZeroUsize},
        rc::Rc,
    };

    use super::{
        AppliedResultAck, BoundSubmissionError, ChildSlotId, ChildSlotObservation,
        ControllerSession, Guardian, GuardianError, GuardianExecutor, GuardianInstanceId,
        GuardianState, ReconciledSessionPermit, SessionDispatchTicket, SignalResultReservation,
        MAX_CHILD_SLOTS, MAX_RETAINED_RESULTS,
    };

    use crate::{
        AttemptId, AttemptStore, BirthToken, CandidateDiscovery, CandidateVerification,
        ChildIdentityObservation, ControllerOs, CpuContentionBasisPoints, DeploymentAttempt,
        DispatchKey, DispatchTicket, EffectCompletion, EffectExecutor, ExecutedEffect,
        GuardianLoopError, IdentityObservation, MemoryPressure, MonotonicTick, ObservationFailure,
        OsEffect, OsQueryError, PersistenceRevision, PolicyLoop, ProcessId, ProcessIdentity,
        ResourceCapacity, ResourceContention, ResourceObservationFailure, ResourcePolicy,
        ResourceSample, RetainedCandidateIdentity, Signal, SignalOutcome, Transition,
    };

    #[derive(Default)]
    struct Audit {
        executions: RefCell<Vec<DispatchKey>>,
        signals: Cell<usize>,
        polls: RefCell<Vec<usize>>,
        executed_inventory: RefCell<Vec<Vec<usize>>>,
        slot_drops: Cell<usize>,
        executor_drops: Cell<usize>,
    }

    struct MemorySlot {
        tag: usize,
        observation: ChildSlotObservation,
        audit: Rc<Audit>,
    }

    impl Drop for MemorySlot {
        fn drop(&mut self) {
            self.audit.slot_drops.set(self.audit.slot_drops.get() + 1);
        }
    }

    struct MemoryOs(Rc<Audit>);

    impl ControllerOs for MemoryOs {
        fn sample_resources(
            &mut self,
            sequence: u64,
        ) -> Result<ResourceSample, ResourceObservationFailure> {
            Ok(resource_sample(sequence))
        }

        fn list_children(
            &mut self,
            _parent: ProcessIdentity,
        ) -> Result<Vec<ProcessId>, OsQueryError> {
            Ok(Vec::new())
        }

        fn acquire_child_identity(
            &mut self,
            _discovery: CandidateDiscovery,
        ) -> ChildIdentityObservation {
            ChildIdentityObservation::Unresolved(ObservationFailure::TemporarilyUnavailable)
        }

        fn observe_identity(&mut self, _identity: ProcessIdentity) -> IdentityObservation {
            IdentityObservation::SameIdentity {
                current_parent: None,
            }
        }

        fn signal_exact_identity(
            &mut self,
            _identity: ProcessIdentity,
            _signal: Signal,
        ) -> SignalOutcome {
            self.0.signals.set(self.0.signals.get() + 1);
            SignalOutcome::Delivered
        }

        fn reverify_retained_candidate(
            &mut self,
            _candidate: RetainedCandidateIdentity,
        ) -> CandidateVerification {
            CandidateVerification::Exact
        }
    }

    struct MemoryExecutor {
        os: MemoryOs,
    }

    impl Drop for MemoryExecutor {
        fn drop(&mut self) {
            let audit = &self.os.0;
            audit.executor_drops.set(audit.executor_drops.get() + 1);
        }
    }

    impl GuardianExecutor for MemoryExecutor {
        type ChildSlot = MemorySlot;

        fn execute(
            &mut self,
            ticket: DispatchTicket,
            slots: &mut BTreeMap<ChildSlotId, Self::ChildSlot>,
        ) -> ExecutedEffect {
            self.os.0.executions.borrow_mut().push(ticket.key());
            self.os
                .0
                .executed_inventory
                .borrow_mut()
                .push(slots.values().map(|slot| slot.tag).collect());
            EffectExecutor::new(&mut self.os).execute(ticket)
        }

        fn poll_observation_only(&self, slot: &Self::ChildSlot) -> ChildSlotObservation {
            self.os.0.polls.borrow_mut().push(slot.tag);
            slot.observation
        }
    }

    struct MemoryStore {
        next: PersistenceRevision,
    }

    impl AttemptStore for MemoryStore {
        type Error = Infallible;

        fn persist(
            &mut self,
            _checkpoint: &DeploymentAttempt,
        ) -> Result<PersistenceRevision, Self::Error> {
            let issued = self.next;
            self.next = issued.checked_next().unwrap();
            Ok(issued)
        }
    }

    fn resource_sample(sequence: u64) -> ResourceSample {
        ResourceSample {
            sequence,
            capacity: ResourceCapacity {
                available_memory_bytes: 1_000,
                available_storage_bytes: 1_000,
            },
            contention: ResourceContention {
                cpu: CpuContentionBasisPoints::new(0).unwrap(),
                memory_pressure: MemoryPressure::Nominal,
            },
        }
    }

    fn root_identity() -> ProcessIdentity {
        ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(42).unwrap()),
            BirthToken::new(1, 0).unwrap(),
        )
    }

    fn start(attempt: AttemptId) -> Transition {
        DeploymentAttempt::start(
            attempt,
            tick(100),
            ResourcePolicy::new(
                NonZeroUsize::MIN,
                100,
                100,
                CpuContentionBasisPoints::new(5_000).unwrap(),
                MemoryPressure::Warning,
            ),
            root_identity(),
        )
    }

    fn tick(value: u64) -> MonotonicTick {
        MonotonicTick::new(value)
    }

    fn sample_ticket(attempt: AttemptId, revision: u64) -> DispatchTicket {
        // Independent in-memory commits can deliberately collide in correlation;
        // the ticket itself still comes only from the real affine dispatch boundary.
        let mut store = MemoryStore {
            next: PersistenceRevision::new(NonZeroU64::new(revision).unwrap()),
        };
        start(attempt)
            .persist(&mut store)
            .unwrap()
            .into_dispatch()
            .1
            .next()
            .unwrap()
    }

    fn commit<S: AttemptStore>(transition: Transition, store: &mut S) -> DeploymentAttempt
    where
        S::Error: std::fmt::Debug,
    {
        transition.persist(store).unwrap().into_dispatch().0
    }

    fn signal_plan<S: AttemptStore>(attempt: AttemptId, store: &mut S) -> crate::CommittedTransition
    where
        S::Error: std::fmt::Debug,
    {
        let checkpoint = commit(start(attempt), store);
        let checkpoint = commit(
            checkpoint
                .record_resource_sample(resource_sample(0))
                .unwrap(),
            store,
        );
        let checkpoint = commit(
            checkpoint.child_scan_completed(root_identity()).unwrap(),
            store,
        );
        let checkpoint = commit(checkpoint.ownership_scan_completed().unwrap(), store);
        let checkpoint = commit(
            checkpoint
                .identity_observed(
                    root_identity(),
                    IdentityObservation::SameIdentity {
                        current_parent: None,
                    },
                )
                .unwrap(),
            store,
        );
        checkpoint
            .request_owned_signals(Signal::Terminate, tick(1))
            .unwrap()
            .persist(store)
            .unwrap()
    }

    fn signal_ticket(attempt: AttemptId) -> DispatchTicket {
        let mut store = MemoryStore {
            next: PersistenceRevision::first(),
        };
        signal_plan(attempt, &mut store)
            .into_dispatch()
            .1
            .next()
            .unwrap()
    }

    fn inputs(audit: &Rc<Audit>, count: usize) -> (MemoryExecutor, Vec<MemorySlot>) {
        let slots = (1..=count)
            .map(|tag| MemorySlot {
                tag,
                observation: if tag.is_multiple_of(2) {
                    ChildSlotObservation::Exited
                } else {
                    ChildSlotObservation::Running
                },
                audit: Rc::clone(audit),
            })
            .collect();
        (
            MemoryExecutor {
                os: MemoryOs(Rc::clone(audit)),
            },
            slots,
        )
    }

    fn fixture(
        attempt: AttemptId,
        deadline: MonotonicTick,
    ) -> (Guardian<MemoryExecutor>, ControllerSession, Rc<Audit>) {
        let audit = Rc::new(Audit::default());
        let (executor, slots) = inputs(&audit, 2);
        let (guardian, session) = Guardian::new(attempt, deadline, executor, slots).unwrap();
        (guardian, session, audit)
    }

    fn reconnect(
        guardian: &mut Guardian<MemoryExecutor>,
        old: ControllerSession,
    ) -> ControllerSession {
        guardian.controller_lost(old).unwrap();
        let ids: Vec<_> = guardian.inventory().collect();
        let revision = guardian
            .minimum_dispatch_revision
            .map_or_else(PersistenceRevision::first, |minimum| {
                minimum.checked_next().unwrap()
            });
        guardian
            .begin_reconciliation(guardian.instance_id(), &ids, revision)
            .unwrap()
    }

    fn permit(
        guardian: &Guardian<MemoryExecutor>,
        session: ControllerSession,
        revision: PersistenceRevision,
    ) -> ReconciledSessionPermit {
        ReconciledSessionPermit::after_committed_reconciliation(
            session,
            guardian.attempt_id(),
            guardian.original_deadline(),
            revision,
        )
    }

    fn assert_no_cleanup(audit: &Audit) {
        assert_eq!(audit.slot_drops.get(), 0);
        assert_eq!(audit.executor_drops.get(), 0);
    }

    fn issue(
        guardian: &Guardian<MemoryExecutor>,
        session: ControllerSession,
        ticket: DispatchTicket,
    ) -> SessionDispatchTicket {
        guardian.bind_ticket(session, ticket).unwrap()
    }

    fn refused(
        result: Result<DispatchKey, BoundSubmissionError>,
    ) -> (GuardianError, SessionDispatchTicket) {
        match result.expect_err("execution must be refused") {
            BoundSubmissionError::Refused(refusal) => (*refusal).into_parts(),
            BoundSubmissionError::ExecutorCorrelationMismatch => panic!("unexpected execution"),
        }
    }

    #[test]
    fn detached_custody_keeps_results_and_refused_bound_authority() {
        let attempt = AttemptId::new(1);
        let (mut guardian, session, audit) = fixture(attempt, tick(100));
        let sample = issue(&guardian, session, sample_ticket(attempt, 1));
        let key = guardian.submit_bound(sample, tick(1)).unwrap();
        let unsent = issue(&guardian, session, signal_ticket(attempt));
        assert!(matches!(
            unsent.effect(),
            OsEffect::SignalExactIdentity { .. }
        ));
        let signal_key = unsent.key();
        let signal_effect = unsent.effect().clone();
        let inventory: Vec<_> = guardian.inventory().collect();
        guardian.controller_lost(session).unwrap();
        guardian.controller_lost(session).unwrap();
        let (error, returned) = refused(guardian.submit_bound(unsent, tick(2)));
        assert_eq!(error, GuardianError::NotActive);
        assert_eq!(returned.key(), signal_key);
        assert_eq!(returned.effect(), &signal_effect);
        assert_eq!(returned.session(), session);
        let raw = sample_ticket(attempt, 2);
        let raw_key = raw.key();
        let (error, raw) = guardian.bind_ticket(session, raw).unwrap_err().into_parts();
        assert_eq!(error, GuardianError::NotActive);
        assert_eq!(raw.key(), raw_key);
        assert_eq!(
            guardian.poll_observation_only(inventory[0]),
            Ok(ChildSlotObservation::Running)
        );
        assert_eq!(
            guardian.poll_observation_only(inventory[1]),
            Ok(ChildSlotObservation::Exited)
        );
        assert_eq!(guardian.inventory().collect::<Vec<_>>(), inventory);
        assert_eq!(
            guardian.retained_result(key).unwrap().completion(),
            &EffectCompletion::SampleResources {
                sequence: 0,
                result: Ok(resource_sample(0)),
            }
        );
        assert_eq!(
            guardian.ack_result(session, AppliedResultAck::after_applied(session, key)),
            Err(GuardianError::SessionFenced)
        );
        assert_eq!(*audit.executions.borrow(), [key]);
        assert_eq!(*audit.polls.borrow(), [1, 2]);
        assert_eq!(*audit.executed_inventory.borrow(), [vec![1, 2]]);
        assert_eq!(audit.signals.get(), 0);
        let custody = guardian.forfeit();
        assert_eq!(custody.retained_results().len(), 1);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn reconnect_cannot_relabel_old_tokens_or_accept_stale_proofs() {
        let attempt = AttemptId::new(2);
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        let sample = issue(&guardian, old, sample_ticket(attempt, 1));
        let key = guardian.submit_bound(sample, tick(1)).unwrap();
        let old_token = issue(&guardian, old, signal_ticket(attempt));
        let old_key = old_token.key();
        let current = reconnect(&mut guardian, old);
        assert_eq!(current.instance_id(), old.instance_id());
        assert_eq!(current.generation(), old.generation() + 1);
        let (error, old_token) = refused(guardian.submit_bound(old_token, tick(2)));
        assert_eq!(error, GuardianError::StaleSession);
        assert_eq!(old_token.session(), old);
        assert_eq!(old_token.key(), old_key);
        let raw = sample_ticket(attempt, 2);
        let raw_key = raw.key();
        let (error, raw) = guardian.bind_ticket(old, raw).unwrap_err().into_parts();
        assert_eq!(error, GuardianError::StaleSession);
        assert_eq!(raw.key(), raw_key);
        let observation = guardian.bind_ticket(current, raw).unwrap();
        assert_eq!(observation.key(), raw_key);
        assert_eq!(observation.session(), current);
        assert_eq!(
            guardian.ack_result(old, AppliedResultAck::after_applied(old, key)),
            Err(GuardianError::StaleSession)
        );
        assert_eq!(
            guardian.ack_result(current, AppliedResultAck::after_applied(old, key)),
            Err(GuardianError::AckMismatch)
        );
        let stale = permit(&guardian, old, PersistenceRevision::first());
        assert_eq!(
            guardian.finish_reconciliation(old, stale, tick(2)),
            Err(GuardianError::StaleSession)
        );
        let stale = permit(&guardian, old, PersistenceRevision::first());
        assert_eq!(
            guardian.finish_reconciliation(current, stale, tick(2)),
            Err(GuardianError::PermitMismatch)
        );
        assert_eq!(
            guardian.controller_lost(old),
            Err(GuardianError::StaleSession)
        );
        assert_eq!(
            guardian.authority_lost(old),
            Err(GuardianError::StaleSession)
        );
        assert_eq!(guardian.state(), GuardianState::Reconciling);
        guardian
            .ack_result(current, AppliedResultAck::after_applied(current, key))
            .unwrap();
        assert!(guardian.retained_result(key).is_none());
        assert_eq!(
            guardian.ack_result(current, AppliedResultAck::after_applied(current, key)),
            Err(GuardianError::ResultNotRetained)
        );
        let proof = permit(&guardian, current, PersistenceRevision::first());
        guardian
            .finish_reconciliation(current, proof, tick(3))
            .unwrap();
        let (error, old_token) = refused(guardian.submit_bound(old_token, tick(3)));
        assert_eq!(error, GuardianError::StaleSession);
        assert_eq!(old_token.session(), old);
        assert_eq!(
            guardian.authority_lost(old),
            Err(GuardianError::StaleSession)
        );
        assert_eq!(guardian.state(), GuardianState::Active);
        let fresh = issue(&guardian, current, signal_ticket(attempt));
        guardian.submit_bound(fresh, tick(3)).unwrap();
        assert_eq!(audit.signals.get(), 1);
        assert_eq!(audit.executions.borrow().len(), 2);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn reconciliation_revision_floor_preserves_refused_tickets_and_admits_equal_or_newer() {
        let attempt = AttemptId::new(20);
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        assert_eq!(guardian.minimum_dispatch_revision, None);
        let raw = sample_ticket(attempt, 1);
        let raw_key = raw.key();
        let raw_effect = raw.effect().clone();
        let current = reconnect(&mut guardian, old);
        let bound = issue(&guardian, current, sample_ticket(attempt, 2));
        let bound_key = bound.key();
        let bound_effect = bound.effect().clone();
        let floor = bound_key.revision().checked_next().unwrap();
        let proof = permit(&guardian, current, floor);
        guardian
            .finish_reconciliation(current, proof, tick(3))
            .unwrap();
        assert_eq!(guardian.minimum_dispatch_revision, Some(floor));
        let (error, raw) = guardian.bind_ticket(current, raw).unwrap_err().into_parts();
        assert_eq!(error, GuardianError::StaleDispatchRevision);
        assert_eq!(raw.key(), raw_key);
        assert_eq!(raw.effect(), &raw_effect);
        let (error, bound) = refused(guardian.submit_bound(bound, tick(3)));
        assert_eq!(error, GuardianError::StaleDispatchRevision);
        assert_eq!(bound.key(), bound_key);
        assert_eq!(bound.effect(), &bound_effect);
        assert_eq!(bound.session(), current);
        assert!(audit.executions.borrow().is_empty());
        assert_eq!(guardian.retained_results().len(), 0);
        assert_eq!(guardian.state(), GuardianState::Active);
        for revision in [floor, floor.checked_next().unwrap()] {
            let ticket = issue(&guardian, current, sample_ticket(attempt, revision.get()));
            let key = ticket.key();
            assert_eq!(guardian.submit_bound(ticket, tick(3)).unwrap(), key);
            assert!(guardian.retained_result(key).is_some());
        }
        assert_eq!(audit.executions.borrow().len(), 2);
        assert_eq!(audit.signals.get(), 0);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn reconciling_floor_rejects_historical_observations_and_activation_proof() {
        let attempt = AttemptId::new(22);
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        let stale = sample_ticket(attempt, 2);
        let key = stale.key();
        let floor = key.revision().checked_next().unwrap();
        guardian.controller_lost(old).unwrap();
        let ids: Vec<_> = guardian.inventory().collect();
        let current = guardian
            .begin_reconciliation(guardian.instance_id(), &ids, floor)
            .unwrap();
        assert_eq!(guardian.reconciliation_floor(), Some(floor));
        assert_eq!(guardian.minimum_dispatch_revision, None);
        let (error, stale) = guardian
            .bind_ticket(current, stale)
            .unwrap_err()
            .into_parts();
        assert_eq!(error, GuardianError::StaleDispatchRevision);
        assert_eq!(stale.key(), key);
        let token = SessionDispatchTicket {
            session: current,
            ticket: stale,
        };
        let (error, token) = refused(guardian.submit_bound(token, tick(2)));
        assert_eq!(error, GuardianError::StaleDispatchRevision);
        assert_eq!(token.key(), key);
        assert_eq!(token.session(), current);
        let proof = permit(&guardian, current, key.revision());
        assert_eq!(
            guardian.finish_reconciliation(current, proof, tick(2)),
            Err(GuardianError::StaleDispatchRevision)
        );
        assert_eq!(guardian.state(), GuardianState::Reconciling);
        assert_eq!(guardian.reconciliation_floor(), Some(floor));
        assert_eq!(guardian.minimum_dispatch_revision, None);
        assert!(audit.executions.borrow().is_empty());
        for revision in [floor, floor.checked_next().unwrap()] {
            let token = issue(&guardian, current, sample_ticket(attempt, revision.get()));
            guardian.submit_bound(token, tick(2)).unwrap();
        }
        let proof = permit(&guardian, current, floor);
        guardian
            .finish_reconciliation(current, proof, tick(3))
            .unwrap();
        assert_eq!(guardian.minimum_dispatch_revision, Some(floor));
        assert_eq!(guardian.reconciliation_floor(), None);
        assert_eq!(guardian.state(), GuardianState::Active);
        assert_eq!(audit.executions.borrow().len(), 2);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn sampling_finish_rechecks_origin_even_when_the_session_and_revision_match() {
        let attempt = AttemptId::new(24);
        let mut store = MemoryStore {
            next: PersistenceRevision::first(),
        };
        let committed = start(attempt).persist(&mut store).unwrap();
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        let mut policy =
            crate::PolicyLoop::from_committed_for_guardian(committed, &guardian, old).unwrap();
        policy
            .interrupt_with_guardian(&mut guardian, old, &mut store)
            .unwrap();
        let ids: Vec<_> = guardian.inventory().collect();
        let current = policy
            .begin_guardian_reconciliation(&mut guardian, old, &ids)
            .unwrap();
        policy
            .recover_with_guardian(&mut guardian, current, &mut store)
            .unwrap();
        let revision = policy.summary().revision;
        assert_eq!(policy.summary().phase, crate::Phase::ResourceSampling);
        assert!(guardian.reconciliation_floor().unwrap() < revision);
        // Inject a changed floor to isolate finish-time origin validation from session checks.
        guardian.reconciliation_floor = Some(revision);
        assert_eq!(
            policy.finish_recovery_with_guardian(&mut guardian, current, tick(1), &mut store),
            Err(crate::GuardianLoopError::Policy(
                crate::LoopFailure::RecoveryNotAdmitted
            ))
        );
        assert_eq!(guardian.state(), GuardianState::AuthorityLost);
        assert_eq!(guardian.reconciliation_floor(), None);
        assert_eq!(guardian.minimum_dispatch_revision, None);
        assert_eq!(policy.summary().revision, revision);
        assert!(audit.executions.borrow().is_empty());
        assert_eq!(audit.signals.get(), 0);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn unfinished_reconciliation_requires_a_newer_interruption_revision() {
        let attempt = AttemptId::new(25);
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        guardian.controller_lost(old).unwrap();
        let ids: Vec<_> = guardian.inventory().collect();
        let floor = PersistenceRevision::first();
        let first = guardian
            .begin_reconciliation(guardian.instance_id(), &ids, floor)
            .unwrap();

        guardian.controller_lost(first).unwrap();
        assert_eq!(guardian.reconciliation_floor(), None);
        assert_eq!(
            guardian.begin_reconciliation(guardian.instance_id(), &ids, floor),
            Err(GuardianError::StaleDispatchRevision)
        );
        assert_eq!(guardian.state(), GuardianState::DetachedFenced);

        let next_floor = floor.checked_next().unwrap();
        let second = guardian
            .begin_reconciliation(guardian.instance_id(), &ids, next_floor)
            .unwrap();
        assert_eq!(second.generation(), first.generation() + 1);
        assert_eq!(guardian.reconciliation_floor(), Some(next_floor));
        assert!(audit.executions.borrow().is_empty());
        assert_no_cleanup(&audit);
    }

    #[test]
    fn repeated_reconciliation_cannot_lower_the_installed_dispatch_floor() {
        let attempt = AttemptId::new(23);
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        let current = reconnect(&mut guardian, old);
        let minimum = PersistenceRevision::first().checked_next().unwrap();
        let proof = permit(&guardian, current, minimum);
        guardian
            .finish_reconciliation(current, proof, tick(1))
            .unwrap();
        guardian.controller_lost(current).unwrap();
        let ids: Vec<_> = guardian.inventory().collect();
        assert_eq!(
            guardian.begin_reconciliation(
                guardian.instance_id(),
                &ids,
                PersistenceRevision::first()
            ),
            Err(GuardianError::StaleDispatchRevision)
        );
        assert_eq!(guardian.state(), GuardianState::DetachedFenced);
        assert_eq!(guardian.reconciliation_floor(), None);
        assert_eq!(guardian.minimum_dispatch_revision, Some(minimum));
        let floor = minimum.checked_next().unwrap();
        let next = guardian
            .begin_reconciliation(guardian.instance_id(), &ids, floor)
            .unwrap();
        assert_eq!(next.generation(), current.generation() + 1);
        let stale = permit(&guardian, next, minimum);
        assert_eq!(
            guardian.finish_reconciliation(next, stale, tick(2)),
            Err(GuardianError::StaleDispatchRevision)
        );
        assert_eq!(guardian.minimum_dispatch_revision, Some(minimum));
        assert_eq!(guardian.reconciliation_floor(), Some(floor));
        let proof = permit(&guardian, next, floor);
        guardian
            .finish_reconciliation(next, proof, tick(2))
            .unwrap();
        assert_eq!(guardian.minimum_dispatch_revision, Some(floor));
        assert_eq!(guardian.reconciliation_floor(), None);
        let final_floor = floor.checked_next().unwrap();
        let final_session = reconnect(&mut guardian, next);
        assert_eq!(guardian.reconciliation_floor(), Some(final_floor));
        guardian.authority_lost(final_session).unwrap();
        assert_eq!(guardian.reconciliation_floor(), None);
        assert_eq!(guardian.minimum_dispatch_revision, Some(floor));
        assert!(audit.executions.borrow().is_empty());
        assert_no_cleanup(&audit);
    }

    #[test]
    fn duplicate_refusal_is_only_while_result_is_retained() {
        let attempt = AttemptId::new(3);
        let (mut guardian, session, audit) = fixture(attempt, tick(100));
        let first = issue(&guardian, session, sample_ticket(attempt, 1));
        let duplicate = issue(&guardian, session, sample_ticket(attempt, 1));
        let key = guardian.submit_bound(first, tick(1)).unwrap();
        let (error, duplicate) = refused(guardian.submit_bound(duplicate, tick(1)));
        assert_eq!(error, GuardianError::DuplicateDispatchKey);
        assert_eq!(duplicate.key(), key);
        let (error, raw) = guardian
            .bind_ticket(session, sample_ticket(attempt, 1))
            .unwrap_err()
            .into_parts();
        assert_eq!(error, GuardianError::DuplicateDispatchKey);
        assert_eq!(raw.key(), key);
        let wrong_attempt = sample_ticket(AttemptId::new(999), 2);
        let foreign_key = wrong_attempt.key();
        let (error, raw) = guardian
            .bind_ticket(session, wrong_attempt)
            .unwrap_err()
            .into_parts();
        assert_eq!(error, GuardianError::AttemptMismatch);
        assert_eq!(raw.key(), foreign_key);
        assert_eq!(*audit.executions.borrow(), [key]);
        guardian
            .ack_result(session, AppliedResultAck::after_applied(session, key))
            .unwrap();
        assert_eq!(guardian.submit_bound(duplicate, tick(2)).unwrap(), key);
        assert_eq!(*audit.executions.borrow(), [key, key]);
        assert_eq!(guardian.retained_results().len(), 1);
    }

    #[test]
    fn capacity_refusals_return_exact_raw_and_bound_tickets() {
        let attempt = AttemptId::new(4);
        let (mut guardian, session, audit) = fixture(attempt, tick(100));
        let overflow_revision = u64::try_from(MAX_RETAINED_RESULTS).unwrap() + 1;
        let overflow = issue(
            &guardian,
            session,
            sample_ticket(attempt, overflow_revision),
        );
        let overflow_key = overflow.key();
        let overflow_effect = overflow.effect().clone();
        for revision in 1..overflow_revision {
            assert!(guardian.can_retain_result());
            let ticket = issue(&guardian, session, sample_ticket(attempt, revision));
            guardian.submit_bound(ticket, tick(1)).unwrap();
        }
        assert!(!guardian.can_retain_result());
        let raw = sample_ticket(attempt, overflow_revision + 1);
        let raw_key = raw.key();
        let raw_effect = raw.effect().clone();
        let (error, raw) = guardian.bind_ticket(session, raw).unwrap_err().into_parts();
        assert_eq!(error, GuardianError::RetainedResultCapacityExceeded);
        assert_eq!(raw.key(), raw_key);
        assert_eq!(raw.effect(), &raw_effect);
        let (error, overflow) = refused(guardian.submit_bound(overflow, tick(1)));
        assert_eq!(error, GuardianError::RetainedResultCapacityExceeded);
        assert_eq!(overflow.key(), overflow_key);
        assert_eq!(overflow.effect(), &overflow_effect);
        assert_eq!(overflow.session(), session);
        assert_eq!(audit.executions.borrow().len(), MAX_RETAINED_RESULTS);
        let first = guardian.retained_results().next().unwrap().key();
        assert_eq!(guardian.retained_result(first).unwrap().key(), first);
        guardian
            .ack_result(session, AppliedResultAck::after_applied(session, first))
            .unwrap();
        assert!(guardian.can_retain_result());
        assert_eq!(
            guardian.submit_bound(overflow, tick(2)).unwrap(),
            overflow_key
        );
        let second = guardian
            .retained_results()
            .find(|result| result.key() != overflow_key)
            .unwrap()
            .key();
        guardian
            .ack_result(session, AppliedResultAck::after_applied(session, second))
            .unwrap();
        let rebound = issue(&guardian, session, raw);
        assert_eq!(guardian.submit_bound(rebound, tick(2)).unwrap(), raw_key);
        assert_eq!(guardian.retained_results().len(), MAX_RETAINED_RESULTS);
        assert_eq!(audit.executions.borrow().len(), MAX_RETAINED_RESULTS + 2);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn reservation_uses_available_not_nominal_result_capacity() {
        let attempt = AttemptId::new(26);
        let (mut guardian, session, audit) = fixture(attempt, tick(100));
        let retained = issue(&guardian, session, sample_ticket(attempt, 1));
        guardian.submit_bound(retained, tick(1)).unwrap();
        let signal = signal_ticket(attempt);
        let revision = signal.key().revision();

        assert_eq!(
            guardian.reserve_signal_results(session, revision, MAX_RETAINED_RESULTS),
            Err(GuardianError::RetainedResultCapacityExceeded)
        );
        assert_eq!(guardian.retained_results().len(), 1);
        assert_eq!(audit.signals.get(), 0);

        guardian
            .reserve_signal_results(session, revision, MAX_RETAINED_RESULTS - 1)
            .unwrap();
        assert_eq!(
            guardian.signal_result_reservation,
            Some(SignalResultReservation {
                session,
                revision,
                remaining: MAX_RETAINED_RESULTS - 1,
            })
        );
        let unrelated = sample_ticket(attempt, 2);
        let unrelated_key = unrelated.key();
        let (error, unrelated) = guardian
            .bind_ticket(session, unrelated)
            .unwrap_err()
            .into_parts();
        assert_eq!(error, GuardianError::RetainedResultCapacityExceeded);
        assert_eq!(unrelated.key(), unrelated_key);
        assert!(matches!(
            unrelated.effect(),
            OsEffect::SampleResources { .. }
        ));
        guardian.controller_lost(session).unwrap();
        assert_eq!(guardian.signal_result_reservation, None);
        assert_eq!(audit.signals.get(), 0);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn policy_bridge_reserves_before_issuing_the_first_signal() {
        let attempt = AttemptId::new(27);
        let (mut guardian, session, audit) = fixture(attempt, tick(100));
        let first_revision = 1_000_u64;
        for offset in 0..u64::try_from(MAX_RETAINED_RESULTS).unwrap() {
            let ticket = issue(
                &guardian,
                session,
                sample_ticket(attempt, first_revision + offset),
            );
            guardian.submit_bound(ticket, tick(1)).unwrap();
        }
        let executions_before = audit.executions.borrow().len();
        let mut store = MemoryStore {
            next: PersistenceRevision::first(),
        };
        let committed = signal_plan(attempt, &mut store);
        let mut policy =
            PolicyLoop::from_committed_for_guardian(committed, &guardian, session).unwrap();

        assert_eq!(
            policy.dispatch_next_to_guardian(&mut guardian, session, tick(2)),
            Err(GuardianLoopError::Guardian(
                GuardianError::RetainedResultCapacityExceeded
            ))
        );
        assert_eq!(audit.executions.borrow().len(), executions_before);
        assert_eq!(audit.signals.get(), 0);
        assert_eq!(guardian.retained_results().len(), MAX_RETAINED_RESULTS);
        let ticket = policy.issue_next().unwrap();
        assert!(matches!(
            ticket.effect(),
            OsEffect::SignalExactIdentity { .. }
        ));
        assert_no_cleanup(&audit);
    }

    struct RefusingStore {
        inner: crate::ControllerSqliteStore,
        fail: bool,
    }

    impl AttemptStore for RefusingStore {
        type Error = crate::ControllerStoreError;

        fn persist(
            &mut self,
            checkpoint: &DeploymentAttempt,
        ) -> Result<PersistenceRevision, Self::Error> {
            if self.fail {
                Err(crate::ControllerStoreError::RevisionConflict)
            } else {
                self.inner.persist(checkpoint)
            }
        }
    }

    #[test]
    fn failed_policy_interruption_or_deadline_refuses_the_exact_held_bound_signal() {
        for deadline in [false, true] {
            let attempt = AttemptId::new(20);
            let mut store = RefusingStore {
                inner: crate::ControllerSqliteStore::in_memory().unwrap(),
                fail: false,
            };
            let committed = signal_plan(attempt, &mut store);
            let (mut guardian, session, audit) = fixture(attempt, tick(100));
            let instance = guardian.instance_id();
            let inventory: Vec<_> = guardian.inventory().collect();
            let mut policy =
                crate::PolicyLoop::from_committed_for_guardian(committed, &guardian, session)
                    .unwrap();
            let raw = policy.issue_next().unwrap();
            let token = issue(&guardian, session, raw);
            let key = token.key();
            let effect = token.effect().clone();
            let revision = policy.summary().revision;
            store.fail = true;
            let result = if deadline {
                policy.deadline_with_guardian(&mut guardian, session, tick(100), &mut store)
            } else {
                policy.interrupt_with_guardian(&mut guardian, session, &mut store)
            };
            assert!(matches!(
                result,
                Err(crate::GuardianLoopError::Persistence(_))
            ));
            assert_eq!(guardian.state(), GuardianState::AuthorityLost);
            assert_eq!(policy.status(), crate::LoopStatus::FailStopped);
            let (error, token) = refused(guardian.submit_bound(token, tick(100)));
            assert_eq!(error, GuardianError::AuthorityLost);
            assert_eq!(token.key(), key);
            assert_eq!(token.effect(), &effect);
            assert_eq!(token.session(), session);
            let (error, token) = refused(guardian.submit_bound(token, tick(101)));
            assert_eq!(error, GuardianError::AuthorityLost);
            assert_eq!(token.key(), key);
            assert_eq!(token.effect(), &effect);
            assert_eq!(token.session(), session);
            assert_eq!(guardian.instance_id(), instance);
            assert_eq!(guardian.inventory().collect::<Vec<_>>(), inventory);
            assert_eq!(guardian.original_deadline(), tick(100));
            assert_eq!(guardian.retained_results().len(), 0);
            assert_eq!(
                store
                    .inner
                    .load_attempt(attempt)
                    .unwrap()
                    .unwrap()
                    .persistence_revision(),
                revision
            );
            assert!(audit.executions.borrow().is_empty());
            assert_eq!(audit.signals.get(), 0);
            assert_no_cleanup(&audit);
        }
    }

    #[test]
    fn reconciling_signal_refusal_preserves_exact_token_without_executor_calls() {
        let attempt = AttemptId::new(21);
        let mut store = crate::ControllerSqliteStore::in_memory().unwrap();
        let committed = signal_plan(attempt, &mut store);
        let revision = committed.checkpoint().persistence_revision().unwrap();
        let (_, mut batch) = committed.into_dispatch();
        let raw = batch.next().unwrap();
        let key = raw.key();
        let effect = raw.effect().clone();
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        let current = reconnect(&mut guardian, old);
        // Inject the private boundary input to exercise its independent signal fence.
        let mut token = SessionDispatchTicket {
            session: current,
            ticket: raw,
        };
        for _ in 0..3 {
            let (error, returned) = refused(guardian.submit_bound(token, tick(1)));
            assert_eq!(error, GuardianError::NotActive);
            assert_eq!(returned.key(), key);
            assert_eq!(returned.effect(), &effect);
            assert_eq!(returned.session(), current);
            token = returned;
        }
        assert_eq!(guardian.state(), GuardianState::Reconciling);
        assert_eq!(guardian.retained_results().len(), 0);
        assert!(audit.executions.borrow().is_empty());
        assert_eq!(audit.signals.get(), 0);
        assert_eq!(
            store
                .load_attempt(attempt)
                .unwrap()
                .unwrap()
                .persistence_revision(),
            revision
        );
        assert_no_cleanup(&audit);
    }

    #[test]
    fn deadline_is_checked_at_submit_and_refusal_preserves_the_token() {
        let attempt = AttemptId::new(5);
        for now in [100, 101] {
            let (mut guardian, session, audit) = fixture(attempt, tick(100));
            let token = issue(&guardian, session, signal_ticket(attempt));
            let key = token.key();
            let effect = token.effect().clone();
            let (error, token) = refused(guardian.submit_bound(token, tick(now)));
            assert_eq!(error, GuardianError::DeadlineExpired);
            assert_eq!(token.key(), key);
            assert_eq!(token.effect(), &effect);
            assert_eq!(token.session(), session);
            assert_eq!(guardian.state(), GuardianState::AuthorityLost);
            let (error, _) = refused(guardian.submit_bound(token, tick(99)));
            assert_eq!(error, GuardianError::AuthorityLost);
            assert!(audit.executions.borrow().is_empty());
            assert_eq!(audit.signals.get(), 0);
            assert_no_cleanup(&audit);
        }
    }

    #[test]
    fn preparation_installs_no_authority_and_activation_rechecks_deadline_and_fence() {
        for change in ["activate", "deadline", "fence", "floor"] {
            let attempt = AttemptId::new(60);
            let (mut guardian, old, audit) = fixture(attempt, tick(100));
            let current = reconnect(&mut guardian, old);
            let floor = guardian.reconciliation_floor;
            let minimum = guardian.minimum_dispatch_revision;
            let proof = permit(&guardian, current, PersistenceRevision::first());
            let prepared = guardian
                .prepare_reconciliation(current, proof, tick(99))
                .unwrap();
            assert_eq!(guardian.state(), GuardianState::Reconciling);
            assert_eq!(guardian.reconciliation_floor, floor);
            assert_eq!(guardian.minimum_dispatch_revision, minimum);
            match change {
                "deadline" => {
                    assert_eq!(
                        guardian.activate_reconciliation(current, prepared, tick(100)),
                        Err(GuardianError::DeadlineExpired)
                    );
                }
                "fence" => {
                    guardian.controller_lost(current).unwrap();
                    assert!(guardian
                        .activate_reconciliation(current, prepared, tick(99))
                        .is_err());
                }
                "floor" => {
                    guardian.reconciliation_floor =
                        Some(PersistenceRevision::first().checked_next().unwrap());
                    assert!(guardian
                        .activate_reconciliation(current, prepared, tick(99))
                        .is_err());
                }
                "activate" => {
                    guardian
                        .activate_reconciliation(current, prepared, tick(99))
                        .unwrap();
                    assert_eq!(guardian.state(), GuardianState::Active);
                    assert_eq!(guardian.reconciliation_floor, None);
                }
                _ => unreachable!(),
            }
            if change != "activate" {
                assert_ne!(guardian.state(), GuardianState::Active);
            }
            assert!(audit.executions.borrow().is_empty());
            assert_no_cleanup(&audit);
        }
    }

    #[test]
    fn reconciliation_requires_exact_proof_and_original_deadline() {
        let attempt = AttemptId::new(6);
        let (mut guardian, old, audit) = fixture(attempt, tick(100));
        let current = reconnect(&mut guardian, old);
        for (attempt, deadline) in [(AttemptId::new(999), 100), (attempt, 99), (attempt, 101)] {
            let wrong = ReconciledSessionPermit::after_committed_reconciliation(
                current,
                attempt,
                tick(deadline),
                PersistenceRevision::first(),
            );
            assert_eq!(
                guardian.finish_reconciliation(current, wrong, tick(99)),
                Err(GuardianError::PermitMismatch)
            );
            assert_eq!(guardian.state(), GuardianState::Reconciling);
            assert_eq!(guardian.original_deadline(), tick(100));
        }
        let proof = permit(&guardian, current, PersistenceRevision::first());
        guardian
            .finish_reconciliation(current, proof, tick(99))
            .unwrap();
        let proof = permit(&guardian, current, PersistenceRevision::first());
        assert_eq!(
            guardian.finish_reconciliation(current, proof, tick(99)),
            Err(GuardianError::NotReconciling)
        );
        let ticket = issue(&guardian, current, signal_ticket(attempt));
        guardian.submit_bound(ticket, tick(99)).unwrap();
        assert_eq!(audit.signals.get(), 1);
        for expired in [100, 101] {
            let (mut guardian, old, audit) = fixture(attempt, tick(100));
            let current = reconnect(&mut guardian, old);
            let proof = permit(&guardian, current, PersistenceRevision::first());
            assert_eq!(
                guardian.finish_reconciliation(current, proof, tick(expired)),
                Err(GuardianError::DeadlineExpired)
            );
            assert_eq!(guardian.state(), GuardianState::AuthorityLost);
            assert!(audit.executions.borrow().is_empty());
            assert_no_cleanup(&audit);
        }
    }

    #[test]
    fn inventory_requires_every_exact_slot_but_accepts_reordering() {
        let attempt = AttemptId::new(7);
        let (mut guardian, session, audit) = fixture(attempt, tick(100));
        let (other, _, _) = fixture(attempt, tick(100));
        let ids: Vec<_> = guardian.inventory().collect();
        let foreign: Vec<_> = other.inventory().collect();
        let instance = guardian.instance_id();
        assert_eq!(
            guardian.begin_reconciliation(instance, &ids, PersistenceRevision::first()),
            Err(GuardianError::NotDetached)
        );
        guardian.controller_lost(session).unwrap();
        assert_eq!(
            guardian.begin_reconciliation(other.instance_id(), &ids, PersistenceRevision::first()),
            Err(GuardianError::InstanceMismatch)
        );
        for wrong in [
            vec![],
            vec![ids[0]],
            vec![ids[0], ids[1], foreign[0]],
            vec![ids[0], foreign[1]],
            foreign.clone(),
            vec![ids[0], ids[0]],
        ] {
            assert_eq!(
                guardian.begin_reconciliation(instance, &wrong, PersistenceRevision::first()),
                Err(GuardianError::InventoryMismatch)
            );
            assert_eq!(guardian.state(), GuardianState::DetachedFenced);
            assert_eq!(guardian.inventory().collect::<Vec<_>>(), ids);
        }
        assert_eq!(
            guardian.poll_observation_only(foreign[0]),
            Err(GuardianError::UnknownChildSlot)
        );
        guardian
            .begin_reconciliation(instance, &[ids[1], ids[0]], PersistenceRevision::first())
            .unwrap();
        assert_eq!(guardian.state(), GuardianState::Reconciling);
        let other_custody = other.forfeit();
        assert_eq!(other_custody.inventory().len(), 2);
        assert!(audit.executions.borrow().is_empty());
        assert_no_cleanup(&audit);
    }

    #[test]
    fn maximum_child_slot_inventory_is_accepted_without_cleanup() {
        let audit = Rc::new(Audit::default());
        let (executor, slots) = inputs(&audit, MAX_CHILD_SLOTS);
        let (guardian, _) = Guardian::new(AttemptId::new(7), tick(100), executor, slots).unwrap();
        assert_eq!(guardian.inventory().len(), MAX_CHILD_SLOTS);
        let custody = guardian.forfeit();
        assert_eq!(custody.summary().child_slots, MAX_CHILD_SLOTS);
        assert_no_cleanup(&audit);
    }

    #[test]
    fn capacity_is_validated_before_mint_and_all_rejections_are_recoverable() {
        let attempt = AttemptId::new(7);
        let audit = Rc::new(Audit::default());
        let (executor, slots) = inputs(&audit, MAX_CHILD_SLOTS + 1);
        let failure =
            Guardian::new_with_instance_source(attempt, tick(100), executor, slots, || {
                panic!("oversized inventory must be refused before minting")
            })
            .err()
            .expect("capacity refusal");
        let (error, executor, mut slots) = failure.into_parts();
        assert_eq!(error, GuardianError::ChildSlotCapacityExceeded);
        assert_eq!(slots.len(), MAX_CHILD_SLOTS + 1);
        assert_no_cleanup(&audit);
        let extra = slots.pop().unwrap();
        let failure =
            Guardian::new_with_instance_source(attempt, tick(100), executor, slots, || {
                Err(GuardianError::InstanceIdExhausted)
            })
            .err()
            .expect("instance exhaustion returns custody");
        let (error, executor, slots) = failure.into_parts();
        assert_eq!(error, GuardianError::InstanceIdExhausted);
        assert_eq!(slots.len(), MAX_CHILD_SLOTS);
        assert!(Rc::ptr_eq(&executor.os.0, &audit));
        assert!(slots
            .iter()
            .enumerate()
            .all(|(index, slot)| slot.tag == index + 1));
        assert_no_cleanup(&audit);
        drop(extra);
        drop(slots);
        drop(executor);
        assert_eq!(audit.slot_drops.get(), MAX_CHILD_SLOTS + 1);
        assert_eq!(audit.executor_drops.get(), 1);
    }

    struct MisdirectedExecutor {
        os: MemoryOs,
        mismatch_on: DispatchKey,
        actual: Option<ExecutedEffect>,
        received: Option<DispatchTicket>,
    }

    impl GuardianExecutor for MisdirectedExecutor {
        type ChildSlot = ();

        fn execute(
            &mut self,
            ticket: DispatchTicket,
            _slots: &mut BTreeMap<ChildSlotId, ()>,
        ) -> ExecutedEffect {
            if ticket.key() == self.mismatch_on {
                self.received = Some(ticket);
                self.actual.take().expect("one quarantined execution")
            } else {
                EffectExecutor::new(&mut self.os).execute(ticket)
            }
        }

        fn poll_observation_only(&self, _slot: &()) -> ChildSlotObservation {
            ChildSlotObservation::Exited
        }
    }

    #[test]
    fn wrong_key_or_same_key_wrong_effect_is_quarantined_outside_normal_results() {
        let attempt = AttemptId::new(8);
        for wrong_effect in [false, true] {
            let audit = Rc::new(Audit::default());
            let mut os = MemoryOs(Rc::clone(&audit));
            let (request, actual) = if wrong_effect {
                let actual_ticket = signal_ticket(attempt);
                let request = sample_ticket(attempt, actual_ticket.key().revision().get());
                (request, EffectExecutor::new(&mut os).execute(actual_ticket))
            } else {
                (
                    sample_ticket(attempt, 2),
                    EffectExecutor::new(&mut os).execute(sample_ticket(attempt, 1)),
                )
            };
            let requested_key = request.key();
            let requested_effect = request.effect().clone();
            let actual_key = actual.key();
            let actual_effect = actual.completion().effect();
            assert_eq!(requested_key == actual_key, wrong_effect);
            let executor = MisdirectedExecutor {
                os,
                mismatch_on: requested_key,
                actual: Some(actual),
                received: None,
            };
            let (mut guardian, session) =
                Guardian::new(attempt, tick(100), executor, vec![()]).unwrap();
            let normal = guardian
                .bind_ticket(session, sample_ticket(attempt, 1))
                .unwrap();
            let normal_key = guardian.submit_bound(normal, tick(1)).unwrap();
            let token = guardian.bind_ticket(session, request).unwrap();
            assert!(matches!(
                guardian.submit_bound(token, tick(2)),
                Err(BoundSubmissionError::ExecutorCorrelationMismatch)
            ));
            assert_eq!(guardian.state(), GuardianState::AuthorityLost);
            assert_eq!(guardian.retained_results().len(), 1);
            assert_eq!(
                guardian.retained_result(normal_key).unwrap().key(),
                normal_key
            );
            assert!(guardian.retained_result(requested_key).is_none());
            let quarantine = guardian.quarantined_result().unwrap();
            assert_eq!(quarantine.requested_key(), requested_key);
            assert_eq!(quarantine.requested_effect(), &requested_effect);
            assert_eq!(quarantine.actual_result().key(), actual_key);
            assert_eq!(
                quarantine.actual_result().completion().effect(),
                actual_effect
            );
            assert_eq!(
                guardian.ack_result(
                    session,
                    AppliedResultAck::after_applied(session, normal_key)
                ),
                Err(GuardianError::AuthorityLost)
            );
            let raw = sample_ticket(attempt, 99);
            assert_eq!(
                guardian.bind_ticket(session, raw).unwrap_err().error(),
                GuardianError::AuthorityLost
            );
            let custody = guardian.forfeit();
            assert!(custody.summary().quarantined_result);
            assert_eq!(
                custody.quarantined_result().unwrap().requested_key(),
                requested_key
            );
            assert_eq!(custody.retained_results().len(), 1);
        }
    }

    #[test]
    fn session_generation_is_nonzero_and_checked() {
        let first = ControllerSession {
            instance: GuardianInstanceId::mint().unwrap(),
            generation: NonZeroU64::MIN,
        };
        assert_eq!(first.generation(), 1);
        assert_eq!(first.checked_next().unwrap().generation(), 2);
        let last = ControllerSession {
            generation: NonZeroU64::new(u64::MAX).unwrap(),
            ..first
        };
        assert_eq!(
            last.checked_next(),
            Err(GuardianError::SessionGenerationExhausted)
        );
    }
}
