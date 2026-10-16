use std::num::NonZeroU64;

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{
    binding_revision, check_authority, key, malformed, read_binding, read_header, required,
    ControllerSqliteStore, ControllerStoreError, Result, Word,
};
use crate::{
    CpuContentionBasisPoints, DeploymentAttempt, DispatchKey, DispatchTicket, MemoryPressure,
    NativeGuardianBinding, NativeSessionState, OsEffect, PersistenceRevision, ResourceCapacity,
    ResourceContention, ResourceObservationFailure, ResourceSample,
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

/// Same-guardian retained sampling observation verified by the authenticated client.
/// It is process-local evidence, not a dispatch ticket or ACK permit.
#[derive(Debug)]
pub struct VerifiedNativeSamplingCompletion {
    issue: NativeSamplingIssue,
    completion: NativeSamplingCompletion,
}

impl VerifiedNativeSamplingCompletion {
    pub(crate) const fn new(
        issue: NativeSamplingIssue,
        completion: NativeSamplingCompletion,
    ) -> Self {
        Self { issue, completion }
    }

    #[must_use]
    pub const fn issue(&self) -> NativeSamplingIssue {
        self.issue
    }

    #[must_use]
    pub const fn completion(&self) -> NativeSamplingCompletion {
        self.completion
    }

    pub(crate) fn into_parts(self) -> (NativeSamplingIssue, NativeSamplingCompletion) {
        (self.issue, self.completion)
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
        "SELECT i.session_generation,i.sequence,a.applying_revision,a.outcome,a.available_memory,a.available_storage,a.cpu,a.pressure FROM native_sampling_issues i LEFT JOIN native_sampling_applications a ON a.attempt_id=i.attempt_id AND a.dispatch_revision=i.dispatch_revision AND a.ordinal=i.ordinal WHERE i.attempt_id=?1 AND i.dispatch_revision=?2 AND i.ordinal=?3",
        params![id, Word(dispatch.revision().get()), ordinal], |row| Ok((
            row.get::<_, Word>(0)?.0, row.get::<_, Word>(1)?.0,
            row.get::<_, Option<Word>>(2)?, row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<Word>>(4)?, row.get::<_, Option<Word>>(5)?,
            row.get::<_, Option<i64>>(6)?, row.get::<_, Option<String>>(7)?,
        ))
    ).optional()?;
    let Some((generation, sequence, applying, outcome, memory, storage, cpu, pressure)) = row
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
        if outcome.is_some()
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
        NativeSamplingCompletion::Sample(sample) => NativeSamplingStatus::AppliedPendingAck {
            issue,
            sample,
            applying_revision,
        },
        NativeSamplingCompletion::Failure(failure) => NativeSamplingStatus::ParkedRetained {
            issue,
            failure,
            applying_revision,
        },
    }))
}
