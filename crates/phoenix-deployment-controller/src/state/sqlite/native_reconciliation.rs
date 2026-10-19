//! Resource-sampling recovery completion; rows are diagnostics, never affine authority.
use super::{
    binding_revision, check_authority, key, malformed, native_sampling, params, read_attempt,
    read_binding, required, ActiveAttempt, AttemptId, AttemptStore, CompatibilityBoundary,
    ControllerSqliteStore, ControllerStoreError, DeploymentAttempt, LoadedAttempt,
    NativeGuardianBinding, NativeSamplingStatus, NativeSessionState, NonZeroU64, OptionalExtension,
    PersistenceRevision, Phase, Result, TransactionBehavior, Word,
};
use crate::{CommittedTransition, OsEffect};

/// Exact completion evidence from one recovery checkpoint transaction.
/// Guardian identity and inventory are normalized under the referenced binding/session.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{NativeReconciliationCompletion, ReconciledSessionPermit};
/// fn forge(row: NativeReconciliationCompletion) -> ReconciledSessionPermit { row.into() }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::NativeReconciliationCompletion;
/// fn tamper(mut row: NativeReconciliationCompletion) { row.recovery_revision = row.binding_revision(); }
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::{AttemptId, NativeReconciliationCompletion, PersistenceRevision};
/// let r = PersistenceRevision::first();
/// let forged = NativeReconciliationCompletion::new(AttemptId::new(1), std::num::NonZeroU64::new(1).unwrap(), r, r, r);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReconciliationCompletion {
    attempt_id: AttemptId,
    session_generation: NonZeroU64,
    interruption_revision: PersistenceRevision,
    recovery_revision: PersistenceRevision,
    binding_revision: PersistenceRevision,
}

impl NativeReconciliationCompletion {
    pub(crate) const fn new(
        attempt_id: AttemptId,
        session_generation: NonZeroU64,
        interruption_revision: PersistenceRevision,
        recovery_revision: PersistenceRevision,
        binding_revision: PersistenceRevision,
    ) -> Self {
        Self {
            attempt_id,
            session_generation,
            interruption_revision,
            recovery_revision,
            binding_revision,
        }
    }
    #[must_use]
    pub const fn attempt_id(self) -> AttemptId {
        self.attempt_id
    }
    #[must_use]
    pub const fn session_generation(self) -> NonZeroU64 {
        self.session_generation
    }
    #[must_use]
    pub const fn interruption_revision(self) -> PersistenceRevision {
        self.interruption_revision
    }
    #[must_use]
    pub const fn recovery_revision(self) -> PersistenceRevision {
        self.recovery_revision
    }
    #[must_use]
    pub const fn binding_revision(self) -> PersistenceRevision {
        self.binding_revision
    }
}

/// Diagnostic evidence of a durable activation, not live session or dispatch authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReconciliationActivation {
    completion: NativeReconciliationCompletion,
    binding_revision: PersistenceRevision,
}
impl NativeReconciliationActivation {
    #[must_use]
    pub const fn completion(self) -> NativeReconciliationCompletion {
        self.completion
    }
    #[must_use]
    pub const fn binding_revision(self) -> PersistenceRevision {
        self.binding_revision
    }
}

/// A positive live prepare response, not reconstructible from durable diagnostics.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{PreparedNativeReconciliation, NativeReconciliationCompletion};
/// fn forge(row: NativeReconciliationCompletion) -> PreparedNativeReconciliation { row.into() }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::PreparedNativeReconciliation;
/// fn duplicate(p: PreparedNativeReconciliation) { let _ = p.clone(); }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::PreparedNativeReconciliation;
/// let forged = PreparedNativeReconciliation { session: todo!(), attempt: todo!(), identity: todo!(), deadline: todo!(), revision: todo!() };
/// ```
#[derive(Debug)]
pub struct PreparedNativeReconciliation {
    session: crate::guardian_ipc::WireSession,
    attempt: AttemptId,
    identity: crate::NativeGuardianIdentity,
    deadline: crate::MonotonicTick,
    revision: PersistenceRevision,
}
impl PreparedNativeReconciliation {
    pub(crate) const fn after_live_prepare(
        session: crate::guardian_ipc::WireSession,
        attempt: AttemptId,
        identity: crate::NativeGuardianIdentity,
        deadline: crate::MonotonicTick,
        revision: PersistenceRevision,
    ) -> Self {
        Self {
            session,
            attempt,
            identity,
            deadline,
            revision,
        }
    }
    #[cfg(test)]
    pub(crate) fn for_test(
        binding: &NativeGuardianBinding,
        completion: NativeReconciliationCompletion,
    ) -> Self {
        Self::after_live_prepare(
            crate::guardian_ipc::WireSession {
                instance: 1,
                generation: completion.session_generation().get(),
            },
            completion.attempt_id(),
            binding.identity(),
            binding.deadline(),
            completion.recovery_revision(),
        )
    }
}

/// One-use proof that activation committed after live preparation.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{CommittedNativeReconciliationActivation, NativeReconciliationActivation};
/// fn forge(row: NativeReconciliationActivation) -> CommittedNativeReconciliationActivation { row.into() }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::CommittedNativeReconciliationActivation;
/// fn duplicate(p: CommittedNativeReconciliationActivation) { let _ = p.clone(); }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::{ControllerSqliteStore, NativeGuardianBinding, NativeReconciliationCompletion, MonotonicTick};
/// fn bypass(s: &mut ControllerSqliteStore, b: &NativeGuardianBinding, c: NativeReconciliationCompletion, now: MonotonicTick) {
///     s.commit_native_reconciliation_activation(b, c, now);
/// }
/// ```
#[derive(Debug)]
pub struct CommittedNativeReconciliationActivation {
    prepared: PreparedNativeReconciliation,
    binding_revision: PersistenceRevision,
}
impl CommittedNativeReconciliationActivation {
    pub(crate) fn into_locator(
        self,
    ) -> (
        crate::guardian_ipc::WireSession,
        AttemptId,
        PersistenceRevision,
        PersistenceRevision,
    ) {
        (
            self.prepared.session,
            self.prepared.attempt,
            self.prepared.revision,
            self.binding_revision,
        )
    }
}

fn is_sampling_interruption(checkpoint: &DeploymentAttempt) -> bool {
    matches!(checkpoint, DeploymentAttempt::Interrupted(interrupted)
        if interrupted.context.compatibility == CompatibilityBoundary::PreMigration
        && matches!(interrupted.resume, ActiveAttempt::ResourceSampling(_)))
}

fn native_deadline_now() -> Result<crate::MonotonicTick> {
    #[cfg(unix)]
    {
        crate::MonotonicTick::native_now()
            .map_err(|_| ControllerStoreError::Malformed("native monotonic clock unavailable"))
    }
    #[cfg(not(unix))]
    {
        Err(ControllerStoreError::Malformed(
            "native monotonic clock unavailable",
        ))
    }
}

fn require_settled_samples(tx: &rusqlite::Transaction<'_>, id: AttemptId) -> Result<()> {
    let mut statement = tx.prepare("SELECT dispatch_revision FROM native_sampling_issues WHERE attempt_id=?1 ORDER BY dispatch_revision")?;
    let revisions = statement
        .query_map([key(id)], |row| row.get::<_, Word>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for revision in revisions {
        let dispatch =
            crate::DispatchKey::from_committed_parts(id, binding_revision(revision.0)?, 0);
        if !matches!(
            native_sampling::read_status(tx, dispatch)?,
            Some(NativeSamplingStatus::SampleAckConfirmed { .. })
        ) {
            return Err(ControllerStoreError::RevisionConflict);
        }
    }
    Ok(())
}

impl ControllerSqliteStore {
    /// Preflights the bounded recovery journey before any live session mutation.
    /// # Errors
    /// Requires Interrupted resource sampling, a binding, and no unsettled sampling issue.
    /// Any failure poisons the handle.
    pub fn preflight_native_sampling_recovery(&mut self, id: AttemptId) -> Result<()> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let checkpoint = required(read_attempt(&tx, id)?, "missing recovery attempt")?;
            let _binding = required(read_binding(&tx, id)?, "missing recovery binding")?;
            if !is_sampling_interruption(&checkpoint) {
                return Err(ControllerStoreError::RevisionConflict);
            }
            require_settled_samples(&tx, id)?;
            tx.commit()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Consumes crash-loaded Interrupted sampling evidence and persists its original
    /// recovery transition plus completion evidence atomically. The returned committed
    /// plan retains its original affine dispatch authority; loading rows cannot recreate it.
    /// # Errors
    /// Wrong state, stale checkpoint/binding/session, unsettled results, or storage failure
    /// drops the plan and poisons the store.
    pub fn commit_native_sampling_recovery_before_deadline(
        &mut self,
        loaded: LoadedAttempt,
        binding: &NativeGuardianBinding,
    ) -> Result<CommittedTransition> {
        let now = native_deadline_now()?;
        self.commit_native_sampling_recovery_at(loaded, binding, now)
    }

    pub(crate) fn commit_native_sampling_recovery_at(
        &mut self,
        loaded: LoadedAttempt,
        binding: &NativeGuardianBinding,
        now: crate::MonotonicTick,
    ) -> Result<CommittedTransition> {
        self.ensure_usable()?;
        let result = (|| {
            let interrupted = loaded.checkpoint;
            if !is_sampling_interruption(&interrupted) {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let transition = interrupted
                .clone()
                .recover()
                .map_err(|_| ControllerStoreError::RevisionConflict)?;
            let mut persistence = SamplingRecoveryStore {
                store: &mut *self,
                interrupted,
                binding,
                now,
            };
            transition.persist(&mut persistence)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Consumes live preparation and commits durable activation before live activation.
    /// # Errors
    /// Any mismatch, replay, or storage failure rolls back and poisons the handle.
    pub fn commit_native_reconciliation_activation(
        &mut self,
        expected: &NativeGuardianBinding,
        completion: NativeReconciliationCompletion,
        prepared: PreparedNativeReconciliation,
    ) -> Result<(
        NativeGuardianBinding,
        NativeReconciliationActivation,
        CommittedNativeReconciliationActivation,
    )> {
        let now = native_deadline_now()?;
        self.commit_native_reconciliation_activation_at(expected, completion, prepared, now)
    }

    pub(crate) fn commit_native_reconciliation_activation_at(
        &mut self,
        expected: &NativeGuardianBinding,
        completion: NativeReconciliationCompletion,
        prepared: PreparedNativeReconciliation,
        now: crate::MonotonicTick,
    ) -> Result<(
        NativeGuardianBinding,
        NativeReconciliationActivation,
        CommittedNativeReconciliationActivation,
    )> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_authority(&tx)?;
            let id = expected.attempt_id();
            let checkpoint = required(read_attempt(&tx, id)?, "missing activation attempt")?;
            let current = expected.current_session();
            let exact_completion: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_reconciliation_completions WHERE attempt_id=?1 AND session_generation=?2 AND interruption_revision=?3 AND recovery_revision=?4 AND binding_revision=?5)",
                params![key(id), Word(completion.session_generation.get()), Word(completion.interruption_revision.get()), Word(completion.recovery_revision.get()), Word(completion.binding_revision.get())],
                |row| row.get(0),
            )?;
            if now >= checkpoint.deadline()
                || prepared.attempt != id
                || prepared.session.generation != current.generation.get()
                || prepared.identity != expected.identity()
                || prepared.deadline != expected.deadline()
                || prepared.revision != completion.recovery_revision
                || !exact_completion
                || completion.attempt_id != id
                || read_binding(&tx, id)?.as_ref() != Some(expected)
                || checkpoint.phase() != Phase::ResourceSampling
                || checkpoint.persistence_revision() != Some(completion.recovery_revision)
                || checkpoint.deadline() != expected.deadline()
                || completion.interruption_revision.checked_next()
                    != Some(completion.recovery_revision)
                || expected.revision() != completion.binding_revision
                || expected.dispatch_floor() != completion.interruption_revision
                || current.generation != completion.session_generation
                || current.interruption_revision != completion.interruption_revision
                || current.state != NativeSessionState::Reconciling
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            require_settled_samples(&tx, id)?;
            let revision = expected
                .revision()
                .checked_next()
                .ok_or(ControllerStoreError::RevisionExhausted)?;
            let session_changed = tx.execute(
                "UPDATE guardian_sessions SET state='active' WHERE attempt_id=?1 AND generation=?2 AND state='reconciling' AND interruption_revision=?3",
                params![key(id), Word(current.generation.get()), Word(current.interruption_revision.get())],
            )?;
            let binding_changed = tx.execute(
                "UPDATE guardian_bindings SET revision=?2,dispatch_floor=?3 WHERE attempt_id=?1 AND revision=?4",
                params![key(id), Word(revision.get()), Word(completion.recovery_revision.get()), Word(expected.revision().get())],
            )?;
            if session_changed != 1 || binding_changed != 1 {
                return Err(ControllerStoreError::RevisionConflict);
            }
            tx.execute(
                "INSERT INTO native_reconciliation_activations VALUES (?1,?2,?3)",
                params![
                    key(id),
                    Word(current.generation.get()),
                    Word(revision.get())
                ],
            )?;
            let active = required(read_binding(&tx, id)?, "missing activated binding")?;
            let mut wanted = expected.clone();
            wanted.revision = revision;
            wanted.dispatch_floor = completion.recovery_revision;
            wanted
                .sessions
                .last_mut()
                .ok_or(ControllerStoreError::RevisionConflict)?
                .state = NativeSessionState::Active;
            if active != wanted {
                return malformed("activation did not round-trip losslessly");
            }
            tx.commit()?;
            Ok((
                active,
                NativeReconciliationActivation {
                    completion,
                    binding_revision: revision,
                },
                CommittedNativeReconciliationActivation {
                    prepared,
                    binding_revision: revision,
                },
            ))
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Reads historical activation diagnostics without recreating live authority.
    /// # Errors
    /// Invalid schema, malformed evidence, or storage errors poison the handle.
    pub fn load_native_reconciliation_activation(
        &mut self,
        id: AttemptId,
        generation: NonZeroU64,
    ) -> Result<Option<NativeReconciliationActivation>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let row = tx.query_row(
                "SELECT c.interruption_revision,c.recovery_revision,c.binding_revision,a.binding_revision FROM native_reconciliation_activations a JOIN native_reconciliation_completions c USING(attempt_id,session_generation) WHERE a.attempt_id=?1 AND a.session_generation=?2",
                params![key(id), Word(generation.get())],
                |r| Ok((r.get::<_, Word>(0)?.0,r.get::<_, Word>(1)?.0,r.get::<_, Word>(2)?.0,r.get::<_, Word>(3)?.0)),
            ).optional()?;
            let activation = row
                .map(|(interruption, recovery, prepared, active)| -> Result<_> {
                    let completion = NativeReconciliationCompletion::new(
                        id,
                        generation,
                        binding_revision(interruption)?,
                        binding_revision(recovery)?,
                        binding_revision(prepared)?,
                    );
                    let binding_revision = binding_revision(active)?;
                    if completion.interruption_revision.checked_next()
                        != Some(completion.recovery_revision)
                        || completion.binding_revision.checked_next() != Some(binding_revision)
                    {
                        return malformed("contradictory activation evidence");
                    }
                    Ok(NativeReconciliationActivation {
                        completion,
                        binding_revision,
                    })
                })
                .transpose()?;
            tx.commit()?;
            Ok(activation)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Reads activation/checkpoint/binding diagnostics in one snapshot.
    /// # Errors
    /// Refuses contradictory or stale current activation evidence.
    pub fn read_native_reconciliation_activation_evidence(
        &mut self,
        id: AttemptId,
        generation: NonZeroU64,
    ) -> Result<
        Option<(
            LoadedAttempt,
            NativeGuardianBinding,
            NativeReconciliationActivation,
        )>,
    > {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let row = tx.query_row(
                "SELECT c.interruption_revision,c.recovery_revision,c.binding_revision,a.binding_revision FROM native_reconciliation_activations a JOIN native_reconciliation_completions c USING(attempt_id,session_generation) WHERE a.attempt_id=?1 AND a.session_generation=?2",
                params![key(id), Word(generation.get())],
                |r| Ok((r.get::<_, Word>(0)?.0,r.get::<_, Word>(1)?.0,r.get::<_, Word>(2)?.0,r.get::<_, Word>(3)?.0)),
            ).optional()?;
            let Some((interruption, recovery, prepared, active)) = row else {
                tx.commit()?;
                return Ok(None);
            };
            let completion = NativeReconciliationCompletion::new(
                id,
                generation,
                binding_revision(interruption)?,
                binding_revision(recovery)?,
                binding_revision(prepared)?,
            );
            let activation = NativeReconciliationActivation {
                completion,
                binding_revision: binding_revision(active)?,
            };
            let checkpoint = required(read_attempt(&tx, id)?, "missing activated attempt")?;
            let binding = required(read_binding(&tx, id)?, "missing activated binding")?;
            let current = binding.current_session();
            if checkpoint.phase() != Phase::ResourceSampling
                || checkpoint.persistence_revision() != Some(completion.recovery_revision)
                || completion.interruption_revision.checked_next()
                    != Some(completion.recovery_revision)
                || completion.binding_revision.checked_next() != Some(activation.binding_revision)
                || binding.revision() != activation.binding_revision
                || binding.dispatch_floor() != completion.recovery_revision
                || current.generation != generation
                || current.state != NativeSessionState::Active
                || current.interruption_revision != completion.interruption_revision
            {
                return malformed("stale or contradictory reconciliation activation");
            }
            require_settled_samples(&tx, id)?;
            tx.commit()?;
            Ok(Some((LoadedAttempt { checkpoint }, binding, activation)))
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Reads one exact completion together with checkpoint and binding in a single snapshot.
    /// # Errors
    /// Malformed or stale evidence poisons the handle; missing completion returns None.
    pub fn read_native_reconciliation_evidence(
        &mut self,
        id: AttemptId,
        generation: NonZeroU64,
    ) -> Result<
        Option<(
            LoadedAttempt,
            NativeGuardianBinding,
            NativeReconciliationCompletion,
        )>,
    > {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let row = tx.query_row(
                "SELECT interruption_revision,recovery_revision,binding_revision FROM native_reconciliation_completions WHERE attempt_id=?1 AND session_generation=?2",
                params![key(id), Word(generation.get())],
                |row| Ok((row.get::<_, Word>(0)?.0, row.get::<_, Word>(1)?.0, row.get::<_, Word>(2)?.0)),
            ).optional()?;
            let Some((interruption, recovery, binding_revision_word)) = row else {
                tx.commit()?;
                return Ok(None);
            };
            let completion = NativeReconciliationCompletion::new(
                id,
                generation,
                binding_revision(interruption)?,
                binding_revision(recovery)?,
                binding_revision(binding_revision_word)?,
            );
            let checkpoint = required(read_attempt(&tx, id)?, "missing completed attempt")?;
            let binding = required(read_binding(&tx, id)?, "missing completed binding")?;
            let current = binding.current_session();
            if checkpoint.phase() != Phase::ResourceSampling
                || checkpoint.persistence_revision() != Some(completion.recovery_revision)
                || completion.interruption_revision.checked_next()
                    != Some(completion.recovery_revision)
                || binding.revision() != completion.binding_revision
                || binding.dispatch_floor() != completion.interruption_revision
                || current.generation != generation
                || current.state != NativeSessionState::Reconciling
                || current.interruption_revision != completion.interruption_revision
            {
                return malformed("stale or contradictory reconciliation completion");
            }
            require_settled_samples(&tx, id)?;
            tx.commit()?;
            Ok(Some((LoadedAttempt { checkpoint }, binding, completion)))
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

struct SamplingRecoveryStore<'a> {
    store: &'a mut ControllerSqliteStore,
    interrupted: DeploymentAttempt,
    binding: &'a NativeGuardianBinding,
    now: crate::MonotonicTick,
}
impl AttemptStore for SamplingRecoveryStore<'_> {
    type Error = ControllerStoreError;
    fn persist(&mut self, checkpoint: &DeploymentAttempt) -> Result<PersistenceRevision> {
        self.store.persist_checkpoint(
            checkpoint,
            None,
            Some((&self.interrupted, self.binding, self.now)),
        )
    }
}

pub(super) fn validate_recovery(
    tx: &rusqlite::Transaction<'_>,
    durable: Option<&DeploymentAttempt>,
    interrupted: &DeploymentAttempt,
    checkpoint: &DeploymentAttempt,
    binding: &NativeGuardianBinding,
    now: crate::MonotonicTick,
) -> Result<()> {
    let current = binding.current_session();
    if now >= checkpoint.deadline()
        || durable != Some(interrupted)
        || !is_sampling_interruption(interrupted)
        || checkpoint.phase() != Phase::ResourceSampling
        || checkpoint.attempt_id() != binding.attempt_id()
        || checkpoint.deadline() != binding.deadline()
        || interrupted.persistence_revision() != Some(current.interruption_revision)
        || binding.dispatch_floor() != current.interruption_revision
        || current.state != NativeSessionState::Reconciling
        || read_binding(tx, binding.attempt_id())?.as_ref() != Some(binding)
    {
        return Err(ControllerStoreError::RevisionConflict);
    }
    let transition = interrupted
        .clone()
        .recover()
        .map_err(|_| ControllerStoreError::RevisionConflict)?;
    if &transition.checkpoint != checkpoint
        || !matches!(
            transition.after_persist.as_slice(),
            [OsEffect::SampleResources { .. }]
        )
    {
        return Err(ControllerStoreError::RevisionConflict);
    }
    require_settled_samples(tx, binding.attempt_id())
}

pub(super) fn insert_completion(
    tx: &rusqlite::Transaction<'_>,
    binding: &NativeGuardianBinding,
    recovery: PersistenceRevision,
) -> Result<()> {
    let session = binding.current_session();
    if session.interruption_revision.checked_next() != Some(recovery) {
        return Err(ControllerStoreError::RevisionConflict);
    }
    tx.execute(
        "INSERT INTO native_reconciliation_completions VALUES (?1,?2,?3,?4,?5)",
        params![
            key(binding.attempt_id()),
            Word(session.generation.get()),
            Word(session.interruption_revision.get()),
            Word(recovery.get()),
            Word(binding.revision().get())
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
