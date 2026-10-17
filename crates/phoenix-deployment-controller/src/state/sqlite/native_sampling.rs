use std::num::NonZeroU64;

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{
    binding_revision, check_authority, key, malformed, read_attempt, read_binding, read_header,
    required, ControllerSqliteStore, ControllerStoreError, Result, Word,
};
use crate::{
    CpuContentionBasisPoints, DeploymentAttempt, DispatchKey, DispatchTicket, LoadedAttempt,
    MemoryPressure, MonotonicTick, NativeGuardianBinding, NativeSessionState, OsEffect,
    PersistenceRevision, ResourceCapacity, ResourceContention, ResourceObservationFailure,
    ResourceSample,
};

/// A committed native sampling identity, not dispatch or result authority.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchTicket, NativeSamplingIssue};
/// fn reconstruct(issue: NativeSamplingIssue) -> DispatchTicket { issue.into() }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSamplingIssue {
    key: DispatchKey,
    session_generation: NonZeroU64,
    guardian_identity: crate::NativeGuardianIdentity,
    sequence: u64,
}

impl NativeSamplingIssue {
    #[must_use]
    pub const fn key(self) -> DispatchKey {
        self.key
    }
    #[must_use]
    pub const fn session_generation(self) -> NonZeroU64 {
        self.session_generation
    }
    #[must_use]
    pub const fn guardian_identity(self) -> crate::NativeGuardianIdentity {
        self.guardian_identity
    }
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

/// The exact typed outcome, separate from any executable or acknowledgement capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSamplingCompletion {
    Sample(ResourceSample),
    Failure(ResourceObservationFailure),
}

/// Runner-local application custody after the committed ticket requested admission.
#[derive(Debug)]
pub struct NativeSamplingDispatch {
    ticket: DispatchTicket,
    issue: NativeSamplingIssue,
}

impl NativeSamplingDispatch {
    pub(crate) fn new(ticket: DispatchTicket, issue: NativeSamplingIssue) -> Self {
        Self { ticket, issue }
    }

    pub(crate) const fn issue(&self) -> NativeSamplingIssue {
        self.issue
    }

    pub(crate) fn into_parts(self) -> (DispatchTicket, NativeSamplingIssue) {
        (self.ticket, self.issue)
    }
}

/// Same-guardian retained sampling observation verified by the authenticated client.
/// It is process-local application evidence, not a dispatch ticket or ACK permit.
#[derive(Debug)]
pub struct VerifiedNativeSamplingCompletion {
    dispatch: NativeSamplingDispatch,
    completion: NativeSamplingCompletion,
}

impl VerifiedNativeSamplingCompletion {
    pub(crate) const fn new(
        dispatch: NativeSamplingDispatch,
        completion: NativeSamplingCompletion,
    ) -> Self {
        Self {
            dispatch,
            completion,
        }
    }

    #[must_use]
    pub const fn issue(&self) -> NativeSamplingIssue {
        self.dispatch.issue
    }

    #[must_use]
    pub const fn completion(&self) -> NativeSamplingCompletion {
        self.completion
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        DispatchTicket,
        NativeSamplingIssue,
        NativeSamplingCompletion,
    ) {
        let (ticket, issue) = self.dispatch.into_parts();
        (ticket, issue, self.completion)
    }
}

/// Affine confirmation proof minted only after a correlated positive guardian response.
/// Stored diagnostics and wire DTOs cannot construct this value.
#[derive(Debug)]
pub struct VerifiedNativeSamplingAckConfirmation {
    issue: NativeSamplingIssue,
    sample: ResourceSample,
    applying_revision: PersistenceRevision,
    confirming_session_generation: NonZeroU64,
}

impl VerifiedNativeSamplingAckConfirmation {
    pub(crate) const fn new(
        issue: NativeSamplingIssue,
        sample: ResourceSample,
        applying_revision: PersistenceRevision,
        confirming_session_generation: NonZeroU64,
    ) -> Self {
        Self {
            issue,
            sample,
            applying_revision,
            confirming_session_generation,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        NativeSamplingIssue,
        ResourceSample,
        PersistenceRevision,
        NonZeroU64,
    ) {
        let Self {
            issue,
            sample,
            applying_revision,
            confirming_session_generation,
        } = self;
        (
            issue,
            sample,
            applying_revision,
            confirming_session_generation,
        )
    }
}

/// Store-validated, one-use interrupted sampling evidence. It grants no dispatch or ACK.
///
/// ```compile_fail
/// use phoenix_deployment_controller::NativeSamplingRecoveryCandidate;
/// fn duplicate(candidate: NativeSamplingRecoveryCandidate) { let _ = candidate.clone(); }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchTicket, NativeSamplingRecoveryCandidate};
/// fn dispatch(candidate: NativeSamplingRecoveryCandidate) -> DispatchTicket { candidate.into() }
/// ```
#[derive(Debug)]
pub struct NativeSamplingRecoveryCandidate {
    loaded: LoadedAttempt,
    binding: NativeGuardianBinding,
    issue: NativeSamplingIssue,
}

impl NativeSamplingRecoveryCandidate {
    #[must_use]
    pub const fn issue(&self) -> NativeSamplingIssue {
        self.issue
    }

    #[must_use]
    pub const fn binding(&self) -> &NativeGuardianBinding {
        &self.binding
    }
}

/// Positive same-guardian live observation; no executable or ACK authority.
///
/// ```compile_fail
/// use phoenix_deployment_controller::VerifiedRecoveredNativeSampling;
/// fn duplicate(verified: VerifiedRecoveredNativeSampling) { let _ = verified.clone(); }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchTicket, VerifiedRecoveredNativeSampling};
/// fn dispatch(verified: VerifiedRecoveredNativeSampling) -> DispatchTicket { verified.into() }
/// ```
#[derive(Debug)]
pub struct VerifiedRecoveredNativeSampling {
    candidate: NativeSamplingRecoveryCandidate,
    sample: ResourceSample,
}

impl VerifiedRecoveredNativeSampling {
    pub(crate) const fn after_live_verification(
        candidate: NativeSamplingRecoveryCandidate,
        sample: ResourceSample,
    ) -> Self {
        Self { candidate, sample }
    }
}

/// Durable diagnostic classification; loading it never reconstructs authority.
///
/// ```compile_fail
/// use phoenix_deployment_controller::{AppliedResultAck, NativeSamplingStatus};
/// fn reconstruct(status: NativeSamplingStatus) -> AppliedResultAck { status.into() }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSamplingStatus {
    IssuedPendingApplication {
        issue: NativeSamplingIssue,
    },
    AppliedPendingAck {
        issue: NativeSamplingIssue,
        sample: ResourceSample,
        applying_revision: PersistenceRevision,
    },
    SampleAckConfirmed {
        issue: NativeSamplingIssue,
        sample: ResourceSample,
        applying_revision: PersistenceRevision,
        confirming_session_generation: NonZeroU64,
    },
    ParkedRetained {
        issue: NativeSamplingIssue,
        failure: ResourceObservationFailure,
        applying_revision: PersistenceRevision,
    },
}

fn issue_params(issue: NativeSamplingIssue) -> (Vec<u8>, Word, i64) {
    (
        key(issue.key.attempt_id()),
        Word(issue.key.revision().get()),
        i64::try_from(issue.key.ordinal()).expect("validated ordinal"),
    )
}

fn unique_recovery_issue(
    tx: &rusqlite::Transaction<'_>,
    attempt: crate::AttemptId,
) -> Result<NativeSamplingIssue> {
    let mut statement = tx.prepare(
        "SELECT dispatch_revision, ordinal FROM native_sampling_issues WHERE attempt_id=?1",
    )?;
    let rows = statement.query_map([key(attempt)], |row| {
        Ok((row.get::<_, Word>(0)?.0, row.get::<_, i64>(1)?))
    })?;
    let mut candidate = None;
    for row in rows {
        let (revision, ordinal) = row?;
        let dispatch = DispatchKey::from_committed_parts(
            attempt,
            binding_revision(revision)?,
            usize::try_from(ordinal)
                .map_err(|_| ControllerStoreError::Malformed("invalid native ordinal"))?,
        );
        match read_status(tx, dispatch)? {
            Some(NativeSamplingStatus::IssuedPendingApplication { issue }) => {
                if candidate.replace(issue).is_some() {
                    return Err(ControllerStoreError::RevisionConflict);
                }
            }
            Some(NativeSamplingStatus::SampleAckConfirmed { .. }) => {}
            _ => return Err(ControllerStoreError::RevisionConflict),
        }
    }
    candidate.ok_or(ControllerStoreError::RevisionConflict)
}

pub(super) fn validate_recovery_candidate(
    tx: &rusqlite::Transaction<'_>,
    interrupted: &DeploymentAttempt,
    expected: &NativeGuardianBinding,
    issue: NativeSamplingIssue,
    now: MonotonicTick,
) -> Result<()> {
    let session = expected.current_session();
    if now >= interrupted.deadline()
        || unique_recovery_issue(tx, interrupted.attempt_id())? != issue
        || interrupted.attempt_id() != issue.key().attempt_id()
        || interrupted.persistence_revision() != Some(session.interruption_revision)
        || interrupted.interrupted_sampling_sequence() != Some(issue.sequence())
        || interrupted.deadline() != expected.deadline()
        || expected.attempt_id() != issue.key().attempt_id()
        || expected.identity() != issue.guardian_identity()
        || session.state != NativeSessionState::Reconciling
        || issue.session_generation() >= session.generation
        || issue.key().revision() >= session.interruption_revision
        || expected.dispatch_floor() != session.interruption_revision
        || read_attempt(tx, issue.key().attempt_id())?.as_ref() != Some(interrupted)
        || read_binding(tx, issue.key().attempt_id())?.as_ref() != Some(expected)
        || read_status(tx, issue.key())?
            != Some(NativeSamplingStatus::IssuedPendingApplication { issue })
    {
        return Err(ControllerStoreError::RevisionConflict);
    }
    Ok(())
}

pub(super) fn native_deadline_now() -> Result<MonotonicTick> {
    #[cfg(unix)]
    {
        MonotonicTick::native_now()
            .map_err(|_| ControllerStoreError::Malformed("native monotonic clock unavailable"))
    }
    #[cfg(not(unix))]
    {
        Err(ControllerStoreError::Malformed(
            "native monotonic clock unavailable",
        ))
    }
}

impl ControllerSqliteStore {
    /// Commits the exact active session's `SampleResources` issue without issuing a ticket.
    /// # Errors
    /// Any mismatch or storage failure poisons this handle; reopen to classify evidence.
    pub fn commit_native_sampling_issue(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
    ) -> Result<NativeSamplingIssue> {
        self.ensure_usable()?;
        let result = (|| {
            let OsEffect::SampleResources { sequence } = *ticket.effect() else {
                return Err(ControllerStoreError::RevisionConflict);
            };
            let dispatch = ticket.key();
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_authority(&tx)?;
            let binding = required(
                read_binding(&tx, dispatch.attempt_id())?,
                "missing native binding",
            )?;
            let session = binding.current_session();
            let header = required(
                read_header(&tx, &key(dispatch.attempt_id()))?,
                "missing attempt",
            )?;
            if &binding != expected
                || expected.attempt_id() != dispatch.attempt_id()
                || session.state != NativeSessionState::Active
                || binding.dispatch_floor() > dispatch.revision()
                || header.revision != dispatch.revision()
                || header.phase != crate::Phase::ResourceSampling
                || dispatch.ordinal() != 0
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let pending: Option<Word> = tx
                .query_row(
                    "SELECT pending_sequence FROM sampling_obligations WHERE attempt_id=?1",
                    [key(dispatch.attempt_id())],
                    |row| row.get(0),
                )
                .optional()?;
            if pending.map(|word| word.0) != Some(sequence) {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let issue = NativeSamplingIssue {
                key: dispatch,
                session_generation: session.generation,
                guardian_identity: expected.identity(),
                sequence,
            };
            let (id, revision, ordinal) = issue_params(issue);
            tx.execute(
                "INSERT INTO native_sampling_issues VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    revision,
                    ordinal,
                    Word(session.generation.get()),
                    Word(sequence)
                ],
            )?;
            tx.commit()?;
            Ok(issue)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Durably retires only an exact issue proven never submitted by the client.
    /// No executable authority is returned.
    /// # Errors
    /// Mismatch, settled/absent evidence, or storage failure rolls back and poisons the handle.
    #[cfg(unix)]
    pub fn cancel_unsubmitted_native_sampling(
        &mut self,
        unsubmitted: crate::native_guardian::UnsubmittedNativeSampling,
    ) -> Result<()> {
        self.ensure_usable()?;
        let (ticket, issue) = unsubmitted.into_parts();
        let result = (|| {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_authority(&tx)?;
            if ticket.key() != issue.key()
                || !matches!(ticket.effect(), OsEffect::SampleResources { sequence }
                    if *sequence == issue.sequence())
                || read_status(&tx, issue.key())?
                    != Some(NativeSamplingStatus::IssuedPendingApplication { issue })
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let (id, revision, ordinal) = issue_params(issue);
            if tx.execute(
                "DELETE FROM native_sampling_issues WHERE attempt_id=?1 AND dispatch_revision=?2 AND ordinal=?3 AND session_generation=?4 AND sequence=?5",
                params![id, revision, ordinal, Word(issue.session_generation().get()), Word(issue.sequence())],
            )? != 1 || read_status(&tx, issue.key())?.is_some() {
                return Err(ControllerStoreError::RevisionConflict);
            }
            tx.commit()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Loads normalized, typed sampling evidence without granting native authority.
    /// # Errors
    /// Malformed rows or storage failures poison this handle.
    pub fn load_native_sampling_status(
        &mut self,
        dispatch: DispatchKey,
    ) -> Result<Option<NativeSamplingStatus>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let result = read_status(&tx, dispatch)?;
            tx.commit()?;
            Ok(result)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Finds the sole successful application awaiting ACK, without recreating execution authority.
    /// # Errors
    /// Any malformed, partial, or ambiguous native row poisons this handle.
    pub fn pending_native_sampling_ack(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(DispatchKey, NativeSamplingStatus)>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let mut statement = tx.prepare(
                "SELECT dispatch_revision, ordinal FROM native_sampling_issues WHERE attempt_id=?1",
            )?;
            let rows = statement.query_map(params![key(attempt)], |row| {
                Ok((row.get::<_, Word>(0)?.0, row.get::<_, i64>(1)?))
            })?;
            let mut pending = None;
            for row in rows {
                let (revision, ordinal) = row?;
                let dispatch = DispatchKey::from_committed_parts(
                    attempt,
                    binding_revision(revision)?,
                    usize::try_from(ordinal)
                        .map_err(|_| ControllerStoreError::Malformed("invalid native ordinal"))?,
                );
                match read_status(&tx, dispatch)? {
                    Some(status @ NativeSamplingStatus::AppliedPendingAck { .. }) => {
                        if pending.replace((dispatch, status)).is_some() {
                            return malformed("multiple pending native ACKs");
                        }
                    }
                    Some(_) => {}
                    None => return malformed("missing native issue"),
                }
            }
            drop(statement);
            tx.commit()?;
            Ok(pending)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Reads the checkpoint, binding, and exact sampling status from one `SQLite` snapshot.
    /// # Errors
    /// Malformed rows or storage failures poison this handle.
    pub fn read_native_sampling_evidence(
        &mut self,
        dispatch: DispatchKey,
    ) -> Result<Option<(LoadedAttempt, NativeGuardianBinding, NativeSamplingStatus)>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let attempt = read_attempt(&tx, dispatch.attempt_id())?;
            let binding = read_binding(&tx, dispatch.attempt_id())?;
            let status = read_status(&tx, dispatch)?;
            let evidence = match (attempt, binding, status) {
                (Some(checkpoint), Some(binding), Some(status)) => {
                    Some((LoadedAttempt { checkpoint }, binding, status))
                }
                (Some(_), Some(_), None) | (None, None, None) => None,
                _ => return malformed("incomplete native sampling evidence"),
            };
            tx.commit()?;
            Ok(evidence)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Consumes crash-loaded evidence only when the exact pending issue, interrupted
    /// checkpoint, binding, and reconciling session are still durable.
    /// # Errors
    /// Any mismatch or storage failure poisons this handle.
    pub fn prepare_native_sampling_recovery(
        &mut self,
        loaded: LoadedAttempt,
    ) -> Result<NativeSamplingRecoveryCandidate> {
        self.prepare_native_sampling_recovery_with_clock(loaded, native_deadline_now)
    }

    #[cfg(test)]
    pub(crate) fn prepare_native_sampling_recovery_at(
        &mut self,
        loaded: LoadedAttempt,
        now: MonotonicTick,
    ) -> Result<NativeSamplingRecoveryCandidate> {
        self.prepare_native_sampling_recovery_with_clock(loaded, || Ok(now))
    }

    fn prepare_native_sampling_recovery_with_clock(
        &mut self,
        loaded: LoadedAttempt,
        mut clock: impl FnMut() -> Result<MonotonicTick>,
    ) -> Result<NativeSamplingRecoveryCandidate> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_authority(&tx)?;
            let id = loaded.checkpoint.attempt_id();
            let binding = required(read_binding(&tx, id)?, "missing native binding")?;
            let issue = unique_recovery_issue(&tx, id)?;
            validate_recovery_candidate(&tx, &loaded.checkpoint, &binding, issue, clock()?)?;
            if clock()? >= loaded.checkpoint.deadline() {
                return Err(ControllerStoreError::RevisionConflict);
            }
            tx.commit()?;
            Ok(NativeSamplingRecoveryCandidate {
                loaded,
                binding,
                issue,
            })
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Applies only a live-verified successful nonterminal sample; returns diagnostics,
    /// never a dispatch or ACK permit.
    /// # Errors
    /// Terminal outcomes, duplicate applications, mismatches, and storage failures
    /// roll back and poison this handle.
    pub fn apply_recovered_native_sampling(
        &mut self,
        verified: VerifiedRecoveredNativeSampling,
    ) -> Result<NativeSamplingStatus> {
        self.apply_recovered_native_sampling_with_clock(verified, native_deadline_now)
    }

    #[cfg(test)]
    pub(crate) fn apply_recovered_native_sampling_at(
        &mut self,
        verified: VerifiedRecoveredNativeSampling,
        now: MonotonicTick,
    ) -> Result<NativeSamplingStatus> {
        self.apply_recovered_native_sampling_with_clock(verified, || Ok(now))
    }

    fn apply_recovered_native_sampling_with_clock(
        &mut self,
        verified: VerifiedRecoveredNativeSampling,
        mut clock: impl FnMut() -> Result<MonotonicTick>,
    ) -> Result<NativeSamplingStatus> {
        self.ensure_usable()?;
        let result = (|| {
            let VerifiedRecoveredNativeSampling { candidate, sample } = verified;
            let NativeSamplingRecoveryCandidate {
                loaded,
                binding,
                issue,
            } = candidate;
            let checkpoint = loaded
                .checkpoint
                .clone()
                .fold_interrupted_sampling_sample(sample)
                .ok_or(ControllerStoreError::RevisionConflict)?;
            let revision = self.persist_checkpoint_with_sample(
                &checkpoint,
                Some((&issue, &NativeSamplingCompletion::Sample(sample))),
                None,
                Some((&loaded.checkpoint, &binding, &issue)),
                &mut clock,
            )?;
            Ok(NativeSamplingStatus::AppliedPendingAck {
                issue,
                sample,
                applying_revision: revision,
            })
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Atomically confirms an authenticated, positively acknowledged successful sample.
    /// # Errors
    /// Any mismatch, duplicate, or ambiguous write poisons this handle.
    pub fn confirm_native_sampling_ack(
        &mut self,
        verified: VerifiedNativeSamplingAckConfirmation,
    ) -> Result<NativeSamplingStatus> {
        self.ensure_usable()?;
        let result = (|| {
            let (issue, sample, applying_revision, confirming_session_generation) =
                verified.into_parts();
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_authority(&tx)?;
            let binding = required(
                read_binding(&tx, issue.key.attempt_id())?,
                "missing binding",
            )?;
            let header = required(
                read_header(&tx, &key(issue.key.attempt_id()))?,
                "missing attempt",
            )?;
            if binding.identity() != issue.guardian_identity
                || binding.current_session().generation != confirming_session_generation
                || confirming_session_generation < issue.session_generation
                || !matches!(
                    binding.current_session().state,
                    NativeSessionState::Active | NativeSessionState::Reconciling
                )
                || binding.deadline() != header.deadline
                || header.revision < applying_revision
                || read_status(&tx, issue.key)?
                    != Some(NativeSamplingStatus::AppliedPendingAck {
                        issue,
                        sample,
                        applying_revision,
                    })
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let (id, revision, ordinal) = issue_params(issue);
            tx.execute("INSERT INTO native_sampling_ack_confirmations (attempt_id,dispatch_revision,ordinal,applying_revision,confirming_session_generation) VALUES (?1,?2,?3,?4,?5)",
                params![id, revision, ordinal, Word(applying_revision.get()), Word(confirming_session_generation.get())])?;
            let status = NativeSamplingStatus::SampleAckConfirmed {
                issue,
                sample,
                applying_revision,
                confirming_session_generation,
            };
            if read_status(&tx, issue.key)? != Some(status) {
                return malformed("native confirmation did not round-trip");
            }
            tx.commit()?;
            Ok(status)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub(crate) fn persist_native_sampling(
        &mut self,
        checkpoint: &DeploymentAttempt,
        issue: &NativeSamplingIssue,
        completion: &NativeSamplingCompletion,
    ) -> Result<PersistenceRevision> {
        self.ensure_usable()?;
        let result = self.persist_checkpoint(checkpoint, Some((issue, completion)), None);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

pub(super) fn insert_application(
    tx: &rusqlite::Transaction<'_>,
    issue: &NativeSamplingIssue,
    completion: &NativeSamplingCompletion,
    applying_revision: PersistenceRevision,
) -> Result<()> {
    let status = required(read_status(tx, issue.key)?, "missing native issue")?;
    if status != (NativeSamplingStatus::IssuedPendingApplication { issue: *issue }) {
        return Err(ControllerStoreError::RevisionConflict);
    }
    let (id, revision, ordinal) = issue_params(*issue);
    let (outcome, memory, storage, cpu, pressure) = match completion {
        NativeSamplingCompletion::Sample(sample) if sample.sequence == issue.sequence => (
            "sample",
            Some(Word(sample.capacity.available_memory_bytes)),
            Some(Word(sample.capacity.available_storage_bytes)),
            Some(i64::from(sample.contention.cpu.get())),
            Some(match sample.contention.memory_pressure {
                MemoryPressure::Nominal => "nominal",
                MemoryPressure::Warning => "warning",
                MemoryPressure::Critical => "critical",
            }),
        ),
        NativeSamplingCompletion::Sample(_) => return Err(ControllerStoreError::RevisionConflict),
        NativeSamplingCompletion::Failure(failure) => (
            match failure {
                ResourceObservationFailure::TemporarilyUnavailable => "temporarily_unavailable",
                ResourceObservationFailure::Unsupported => "unsupported",
                ResourceObservationFailure::Malformed => "malformed",
            },
            None,
            None,
            None,
            None,
        ),
    };
    tx.execute("INSERT INTO native_sampling_applications (attempt_id,dispatch_revision,ordinal,applying_revision,outcome,available_memory,available_storage,cpu,pressure) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, revision, ordinal, Word(applying_revision.get()), outcome, memory, storage, cpu, pressure])?;
    let expected = match completion {
        NativeSamplingCompletion::Sample(sample) => NativeSamplingStatus::AppliedPendingAck {
            issue: *issue,
            sample: *sample,
            applying_revision,
        },
        NativeSamplingCompletion::Failure(failure) => NativeSamplingStatus::ParkedRetained {
            issue: *issue,
            failure: *failure,
            applying_revision,
        },
    };
    if read_status(tx, issue.key)? != Some(expected) {
        return malformed("native application did not round-trip");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) fn read_status(
    connection: &rusqlite::Connection,
    dispatch: DispatchKey,
) -> Result<Option<NativeSamplingStatus>> {
    let id = key(dispatch.attempt_id());
    let ordinal =
        i64::try_from(dispatch.ordinal()).map_err(|_| ControllerStoreError::RevisionConflict)?;
    let row = connection.query_row(
        "SELECT i.session_generation,i.sequence,a.applying_revision,a.outcome,a.available_memory,a.available_storage,a.cpu,a.pressure,c.confirming_session_generation FROM native_sampling_issues i LEFT JOIN native_sampling_applications a ON a.attempt_id=i.attempt_id AND a.dispatch_revision=i.dispatch_revision AND a.ordinal=i.ordinal LEFT JOIN native_sampling_ack_confirmations c ON c.attempt_id=i.attempt_id AND c.dispatch_revision=i.dispatch_revision AND c.ordinal=i.ordinal WHERE i.attempt_id=?1 AND i.dispatch_revision=?2 AND i.ordinal=?3",
        params![id, Word(dispatch.revision().get()), ordinal], |row| Ok((
            row.get::<_, Word>(0)?.0, row.get::<_, Word>(1)?.0,
            row.get::<_, Option<Word>>(2)?, row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<Word>>(4)?, row.get::<_, Option<Word>>(5)?,
            row.get::<_, Option<i64>>(6)?, row.get::<_, Option<String>>(7)?, row.get::<_, Option<Word>>(8)?,
        ))
    ).optional()?;
    let Some((generation, sequence, applying, outcome, memory, storage, cpu, pressure, confirming)) =
        row
    else {
        return Ok(None);
    };
    let session_generation = required(NonZeroU64::new(generation), "zero native generation")?;
    if dispatch.ordinal() != 0 {
        return malformed("invalid native sampling ordinal");
    }
    let binding = required(
        read_binding(connection, dispatch.attempt_id())?,
        "missing native issue binding",
    )?;
    let issue = NativeSamplingIssue {
        key: dispatch,
        session_generation,
        guardian_identity: binding.identity(),
        sequence,
    };
    let session = required(
        binding
            .sessions()
            .iter()
            .find(|s| s.generation == issue.session_generation),
        "missing issuing session",
    )?;
    let header = required(
        read_header(connection, &id)?,
        "missing native issue attempt",
    )?;
    if session.interruption_revision > dispatch.revision() || dispatch.revision() > header.revision
    {
        return malformed("invalid native issue revision");
    }
    let Some(applying) = applying else {
        if confirming.is_some()
            || outcome.is_some()
            || memory.is_some()
            || storage.is_some()
            || cpu.is_some()
            || pressure.is_some()
        {
            return malformed("partial native application");
        }
        return Ok(Some(NativeSamplingStatus::IssuedPendingApplication {
            issue,
        }));
    };
    let applying_revision = binding_revision(applying.0)?;
    if applying_revision <= dispatch.revision() || applying_revision > header.revision {
        return malformed("invalid native applying revision");
    }
    let completion = match outcome.as_deref() {
        Some("sample") => NativeSamplingCompletion::Sample(ResourceSample {
            sequence,
            capacity: ResourceCapacity {
                available_memory_bytes: required(memory, "missing native memory")?.0,
                available_storage_bytes: required(storage, "missing native storage")?.0,
            },
            contention: ResourceContention {
                cpu: CpuContentionBasisPoints::new(
                    u16::try_from(required(cpu, "missing native cpu")?)
                        .map_err(|_| ControllerStoreError::Malformed("invalid native cpu"))?,
                )
                .map_err(|_| ControllerStoreError::Malformed("invalid native cpu"))?,
                memory_pressure: match pressure.as_deref() {
                    Some("nominal") => MemoryPressure::Nominal,
                    Some("warning") => MemoryPressure::Warning,
                    Some("critical") => MemoryPressure::Critical,
                    _ => return malformed("invalid native pressure"),
                },
            },
        }),
        Some("temporarily_unavailable") => {
            NativeSamplingCompletion::Failure(ResourceObservationFailure::TemporarilyUnavailable)
        }
        Some("unsupported") => {
            NativeSamplingCompletion::Failure(ResourceObservationFailure::Unsupported)
        }
        Some("malformed") => {
            NativeSamplingCompletion::Failure(ResourceObservationFailure::Malformed)
        }
        _ => return malformed("invalid native outcome"),
    };
    if !matches!(completion, NativeSamplingCompletion::Sample(_))
        && (memory.is_some() || storage.is_some() || cpu.is_some() || pressure.is_some())
    {
        return malformed("failure with sample columns");
    }
    Ok(Some(match completion {
        NativeSamplingCompletion::Sample(sample) => {
            if let Some(generation) = confirming {
                let confirming_session_generation =
                    required(NonZeroU64::new(generation.0), "zero confirming generation")?;
                if confirming_session_generation < issue.session_generation() {
                    return malformed("confirming session predates native issue");
                }
                let confirming_session = required(
                    binding
                        .sessions()
                        .iter()
                        .find(|s| s.generation == confirming_session_generation),
                    "missing confirming session",
                )?;
                if confirming_session.interruption_revision > header.revision {
                    return malformed("invalid confirming session revision");
                }
                NativeSamplingStatus::SampleAckConfirmed {
                    issue,
                    sample,
                    applying_revision,
                    confirming_session_generation,
                }
            } else {
                NativeSamplingStatus::AppliedPendingAck {
                    issue,
                    sample,
                    applying_revision,
                }
            }
        }
        NativeSamplingCompletion::Failure(failure) => {
            if confirming.is_some() {
                return malformed("confirmed failed native sample");
            }
            NativeSamplingStatus::ParkedRetained {
                issue,
                failure,
                applying_revision,
            }
        }
    }))
}

#[cfg(test)]
mod pending_lookup_tests {
    use super::*;
    use crate::{
        BirthToken, GuardianLifetimeNonce, LoopProgress, PolicyLoop, ProcessId, ProcessIdentity,
        ResourcePolicy,
    };
    use std::num::{NonZeroU32, NonZeroUsize};

    #[test]
    fn pending_ack_lookup_returns_only_success_and_poisons_malformed_rows() {
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let attempt = crate::AttemptId::new(71);
        let process = |pid| {
            ProcessIdentity::new(
                ProcessId::new(NonZeroU32::new(pid).unwrap()),
                BirthToken::new(1, 0).unwrap(),
            )
        };
        let committed = DeploymentAttempt::start(
            attempt,
            crate::MonotonicTick::new(100),
            ResourcePolicy::new(
                NonZeroUsize::new(2).unwrap(),
                0,
                0,
                CpuContentionBasisPoints::new(0).unwrap(),
                MemoryPressure::Critical,
            ),
            process(42),
        )
        .persist(&mut store)
        .unwrap();
        let binding = store
            .commit_initial_binding(
                attempt,
                crate::MonotonicTick::new(100),
                PersistenceRevision::first(),
                crate::NativeGuardianIdentity {
                    nonce: GuardianLifetimeNonce::new([1; 16]).unwrap(),
                    process: process(100),
                },
                &[],
            )
            .unwrap();
        assert_eq!(store.pending_native_sampling_ack(attempt).unwrap(), None);
        let mut policy = PolicyLoop::from_committed(committed);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        assert_eq!(store.pending_native_sampling_ack(attempt).unwrap(), None);
        let sample = ResourceSample {
            sequence: issue.sequence(),
            capacity: ResourceCapacity {
                available_memory_bytes: 100,
                available_storage_bytes: 100,
            },
            contention: ResourceContention {
                cpu: CpuContentionBasisPoints::new(0).unwrap(),
                memory_pressure: MemoryPressure::Nominal,
            },
        };
        let verified = VerifiedNativeSamplingCompletion::new(
            NativeSamplingDispatch::new(ticket, issue),
            NativeSamplingCompletion::Sample(sample),
        );
        assert!(matches!(
            policy.apply_native_sampling(verified, &mut store).unwrap(),
            LoopProgress::Applied { .. }
        ));
        assert!(
            matches!(store.pending_native_sampling_ack(attempt).unwrap(),
            Some((key, NativeSamplingStatus::AppliedPendingAck { sample: stored, .. })) if key == issue.key() && stored == sample)
        );
        store
            .connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE native_sampling_applications SET outcome='corrupt' WHERE attempt_id=?1",
                params![key(attempt)],
            )
            .unwrap();
        assert!(matches!(
            store.pending_native_sampling_ack(attempt),
            Err(ControllerStoreError::Malformed(_))
        ));
        assert!(matches!(
            store.pending_native_sampling_ack(attempt),
            Err(ControllerStoreError::Poisoned)
        ));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::{
        AttemptStore, BirthToken, GuardianLifetimeNonce, MemoryPressure, MonotonicTick,
        NativeGuardianIdentity, Phase, PolicyLoop, ProcessId, ProcessIdentity, ResourcePolicy,
    };
    use std::num::{NonZeroU32, NonZeroUsize};

    fn fixture(
        required: usize,
    ) -> (
        ControllerSqliteStore,
        LoadedAttempt,
        NativeGuardianBinding,
        NativeSamplingIssue,
        ResourceSample,
    ) {
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let id = crate::AttemptId::new(721);
        let process = ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(42).unwrap()),
            BirthToken::new(1, 0).unwrap(),
        );
        let committed = DeploymentAttempt::start(
            id,
            MonotonicTick::new(500),
            ResourcePolicy::new(
                NonZeroUsize::new(required).unwrap(),
                10,
                10,
                CpuContentionBasisPoints::new(100).unwrap(),
                MemoryPressure::Critical,
            ),
            process,
        )
        .persist(&mut store)
        .unwrap();
        let binding = store
            .commit_initial_binding(
                id,
                MonotonicTick::new(500),
                PersistenceRevision::first(),
                NativeGuardianIdentity {
                    nonce: GuardianLifetimeNonce::new([7; 16]).unwrap(),
                    process,
                },
                &[],
            )
            .unwrap();
        let checkpoint = committed.checkpoint().clone();
        let mut policy = PolicyLoop::from_committed(committed);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        let binding = store.commit_detached_binding(&binding, &[]).unwrap();
        let interrupted = checkpoint.interrupt().unwrap().persist(&mut store).unwrap();
        let binding = store
            .commit_fresh_interruption_binding(
                &binding,
                interrupted.checkpoint().persistence_revision().unwrap(),
            )
            .unwrap();
        let loaded = store.load_attempt(id).unwrap().unwrap();
        let sample = ResourceSample {
            sequence: issue.sequence(),
            capacity: ResourceCapacity {
                available_memory_bytes: 100,
                available_storage_bytes: 100,
            },
            contention: ResourceContention {
                cpu: CpuContentionBasisPoints::new(0).unwrap(),
                memory_pressure: MemoryPressure::Nominal,
            },
        };
        (store, loaded, binding, issue, sample)
    }

    fn assert_unchanged_and_poisoned(
        store: &mut ControllerSqliteStore,
        original: &DeploymentAttempt,
    ) {
        assert_eq!(
            read_attempt(&store.connection, original.attempt_id()).unwrap(),
            Some(original.clone())
        );
        let applications: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM native_sampling_applications WHERE attempt_id=?1",
                [key(original.attempt_id())],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applications, 0);
        assert!(matches!(
            store.load_attempt(original.attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));
    }

    #[test]
    fn preparation_requires_exactly_one_pending_issue() {
        let (mut store, loaded, _, issue, _) = fixture(2);
        let original = loaded.checkpoint.clone();
        store
            .connection
            .execute(
                "DELETE FROM native_sampling_issues WHERE attempt_id=?1",
                [key(issue.key().attempt_id())],
            )
            .unwrap();
        assert!(matches!(
            store.prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_unchanged_and_poisoned(&mut store, &original);

        let (mut store, loaded, binding, issue, _) = fixture(2);
        let original = loaded.checkpoint.clone();
        insert_extra_pending_issue(&store, &binding, issue);
        assert!(matches!(
            store.prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_unchanged_and_poisoned(&mut store, &original);
    }

    fn insert_extra_pending_issue(
        store: &ControllerSqliteStore,
        binding: &NativeGuardianBinding,
        issue: NativeSamplingIssue,
    ) {
        store.connection.execute(
            "INSERT INTO native_sampling_issues (attempt_id,dispatch_revision,ordinal,session_generation,sequence) VALUES (?1,?2,0,?3,?4)",
            params![
                key(issue.key().attempt_id()),
                Word(binding.current_session().interruption_revision.get()),
                Word(binding.current_session().generation.get()),
                Word(issue.sequence())
            ],
        ).unwrap();
    }

    #[test]
    fn application_rejects_new_unsettled_issue_after_preparation() {
        let (mut store, loaded, binding, issue, sample) = fixture(2);
        let original = loaded.checkpoint.clone();
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        insert_extra_pending_issue(&store, &binding, issue);
        assert!(matches!(
            store.apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_unchanged_and_poisoned(&mut store, &original);
    }

    #[test]
    fn preparation_refuses_original_deadline_without_writes() {
        for now in [500, 501] {
            let (mut store, loaded, _, _, _) = fixture(2);
            let original = loaded.checkpoint.clone();
            assert!(matches!(
                store.prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(now)),
                Err(ControllerStoreError::RevisionConflict)
            ));
            assert_unchanged_and_poisoned(&mut store, &original);
        }
    }

    #[test]
    fn application_refuses_original_deadline_without_writes() {
        for now in [500, 501] {
            let (mut store, loaded, _, _, sample) = fixture(2);
            let original = loaded.checkpoint.clone();
            let candidate = store
                .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
                .unwrap();
            assert!(matches!(
                store.apply_recovered_native_sampling_at(
                    VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                    MonotonicTick::new(now)
                ),
                Err(ControllerStoreError::RevisionConflict)
            ));
            assert_unchanged_and_poisoned(&mut store, &original);
        }
    }

    #[test]
    fn preparation_refuses_deadline_reached_after_validation() {
        let (mut store, loaded, _, _, _) = fixture(2);
        let original = loaded.checkpoint.clone();
        let mut ticks = [499, 500].into_iter();
        assert!(matches!(
            store.prepare_native_sampling_recovery_with_clock(loaded, || {
                Ok(MonotonicTick::new(
                    ticks.next().expect("unexpected clock read"),
                ))
            }),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(ticks.next(), None);
        assert_unchanged_and_poisoned(&mut store, &original);
    }

    #[test]
    fn application_rolls_back_when_deadline_reached_before_commit() {
        let (mut store, loaded, _, _, sample) = fixture(2);
        let original = loaded.checkpoint.clone();
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        let mut ticks = [499, 500].into_iter();
        assert!(matches!(
            store.apply_recovered_native_sampling_with_clock(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                || Ok(MonotonicTick::new(
                    ticks.next().expect("unexpected clock read")
                )),
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(ticks.next(), None);
        assert_unchanged_and_poisoned(&mut store, &original);
    }

    #[cfg(unix)]
    #[test]
    fn public_recovery_entries_use_native_deadline() {
        let (mut store, loaded, _, _, _) = fixture(2);
        let original = loaded.checkpoint.clone();
        assert!(matches!(
            store.prepare_native_sampling_recovery(loaded),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_unchanged_and_poisoned(&mut store, &original);

        let (mut store, loaded, _, _, sample) = fixture(2);
        let original = loaded.checkpoint.clone();
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        assert!(matches!(
            store.apply_recovered_native_sampling(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_unchanged_and_poisoned(&mut store, &original);
    }

    #[test]
    fn recovered_sample_is_atomic_interrupted_and_not_reapplied() {
        let (mut store, loaded, binding, issue, sample) = fixture(2);
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        assert_eq!(candidate.issue(), issue);
        assert_eq!(candidate.binding(), &binding);
        let status = store
            .apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499),
            )
            .unwrap();
        assert_eq!(
            status,
            NativeSamplingStatus::AppliedPendingAck {
                issue,
                sample,
                applying_revision: binding
                    .current_session()
                    .interruption_revision
                    .checked_next()
                    .unwrap(),
            }
        );
        let checkpoint = store
            .load_attempt(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.phase(), Phase::Interrupted);
        assert_eq!(
            checkpoint.checkpoint.interrupted_sampling_sequence(),
            Some(1)
        );
        assert_eq!(
            store.load_native_sampling_status(issue.key()).unwrap(),
            Some(status)
        );
        let applications: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM native_sampling_applications",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let acks: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM native_sampling_ack_confirmations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((applications, acks), (1, 0));
        assert!(matches!(
            store.prepare_native_sampling_recovery_at(checkpoint, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert!(matches!(
            store.load_native_sampling_status(issue.key()),
            Err(ControllerStoreError::Poisoned)
        ));
    }

    #[test]
    fn preparation_rejects_wrong_sequence_stale_checkpoint_and_session() {
        let (mut store, loaded, _, issue, _) = fixture(2);
        store
            .connection
            .execute(
                "UPDATE native_sampling_issues SET sequence=?2 WHERE attempt_id=?1",
                params![key(issue.key().attempt_id()), Word(issue.sequence() + 1)],
            )
            .unwrap();
        assert!(matches!(
            store.prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert!(matches!(
            store.load_attempt(issue.key().attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));

        let (mut store, loaded, _, _, _) = fixture(2);
        store.persist(&loaded.checkpoint).unwrap();
        assert!(matches!(
            store.prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));

        let (mut store, loaded, binding, issue, _) = fixture(2);
        store.connection.execute(
            "UPDATE guardian_sessions SET state='detached' WHERE attempt_id=?1 AND generation=?2",
            params![key(issue.key().attempt_id()), Word(binding.current_session().generation.get())],
        ).unwrap();
        assert!(matches!(
            store.prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499)),
            Err(ControllerStoreError::RevisionConflict)
        ));
    }

    #[test]
    fn wrong_sequence_and_stale_binding_do_not_apply() {
        let (mut store, loaded, _, _, mut sample) = fixture(2);
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        sample.sequence += 1;
        assert!(matches!(
            store.apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        let (mut store, loaded, binding, issue, sample) = fixture(2);
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE guardian_bindings SET revision=?2 WHERE attempt_id=?1",
                params![
                    key(issue.key().attempt_id()),
                    Word(binding.revision.checked_next().unwrap().get())
                ],
            )
            .unwrap();
        assert!(matches!(
            store.apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_sampling_applications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn terminal_and_stale_session_samples_fail_without_writes() {
        let (mut store, loaded, _, issue, sample) = fixture(1);
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        let original = candidate.loaded.checkpoint.clone();
        assert!(matches!(
            store.apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(
            read_attempt(&store.connection, issue.key().attempt_id()).unwrap(),
            Some(original)
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_sampling_applications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );

        let (mut store, loaded, binding, issue, sample) = fixture(2);
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        store.connection.execute("UPDATE guardian_sessions SET state='detached' WHERE attempt_id=?1 AND generation=?2",
            params![key(issue.key().attempt_id()), Word(binding.current_session().generation.get())]).unwrap();
        assert!(matches!(
            store.apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::RevisionConflict)
        ));
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_sampling_applications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn failed_application_insert_rolls_back_checkpoint_and_poisons() {
        let (mut store, loaded, _, issue, sample) = fixture(2);
        let original = loaded.checkpoint.clone();
        let candidate = store
            .prepare_native_sampling_recovery_at(loaded, MonotonicTick::new(499))
            .unwrap();
        store.connection.execute_batch("CREATE TRIGGER fail_sample BEFORE INSERT ON native_sampling_applications BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(matches!(
            store.apply_recovered_native_sampling_at(
                VerifiedRecoveredNativeSampling::after_live_verification(candidate, sample),
                MonotonicTick::new(499)
            ),
            Err(ControllerStoreError::Sqlite(_))
        ));
        assert_eq!(
            read_attempt(&store.connection, issue.key().attempt_id()).unwrap(),
            Some(original)
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_sampling_applications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(matches!(
            store.load_attempt(issue.key().attempt_id()),
            Err(ControllerStoreError::Poisoned)
        ));
    }
}
