use std::num::NonZeroU64;

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{
    binding_revision, check_authority, key, malformed, read_attempt, read_binding, read_header,
    required, ControllerSqliteStore, ControllerStoreError, Result, Word,
};
use crate::{
    CpuContentionBasisPoints, DeploymentAttempt, DispatchKey, DispatchTicket, LoadedAttempt,
    MemoryPressure, NativeGuardianBinding, NativeSessionState, OsEffect, PersistenceRevision,
    ResourceCapacity, ResourceContention, ResourceObservationFailure, ResourceSample,
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
        let result = self.persist_checkpoint(checkpoint, Some((issue, completion)));
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
fn read_status(
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
