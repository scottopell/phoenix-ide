//! Controller-only persistence. The phase selects the required relational shape;
//! recovery rejects missing, extraneous, or contradictory evidence.

use std::{error::Error, fmt, path::Path};

use rusqlite::{
    params,
    types::{FromSql, FromSqlError, ToSql, ToSqlOutput, Value, ValueRef},
    Connection, OptionalExtension, TransactionBehavior,
};

use super::{
    ActiveAttempt, AdministrativeCloseoutRequired, AdministrativelyClosed, AttemptContext,
    AttemptId, AttemptStore, CleanupOutcome, CompatibilityBoundary, DeploymentAttempt,
    ForwardRecoveryAttempt, InterruptedAttempt, LoadedAttempt, MonotonicTick, OwnershipAttempt,
    OwnershipPhase, PersistenceRevision, Phase, ReadyAttempt, RecoveryResume,
    ResourceBlockedAttempt, ResourceSamplingAttempt,
};
use crate::{
    candidate::RetainedCandidateIdentity,
    os::CandidateVerification,
    ownership::CleanupReadiness,
    resource::{ResourceBlock, ResourceWindow, SustainedResourceEvidence},
};

mod resources;
mod tree;

const SCHEMA: &str = include_str!("sqlite/schema.sql");
const SCHEMA_VERSION: i64 = 1;

/// An independent controller database, never a Phoenix application connection.
/// Only schema version 1 is supported; there is no upgrade/downgrade policy.
#[derive(Debug)]
pub struct ControllerSqliteStore {
    connection: Connection,
    poisoned: bool,
}

/// A failure grants no effect capability. Persistence errors poison the handle;
/// reopening permits classification of durable evidence, never blind retry.
#[derive(Debug)]
pub enum ControllerStoreError {
    Sqlite(rusqlite::Error),
    UnsupportedSchema(i64),
    NonemptyUnversionedDatabase,
    RevisionConflict,
    RevisionExhausted,
    Poisoned,
    Malformed(&'static str),
}

impl fmt::Display for ControllerStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "controller SQLite failure: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported controller schema version {version}")
            }
            Self::NonemptyUnversionedDatabase => {
                f.write_str("refusing nonempty unversioned controller database")
            }
            Self::RevisionConflict => f.write_str("controller checkpoint revision conflict"),
            Self::RevisionExhausted => f.write_str("controller checkpoint revision exhausted"),
            Self::Poisoned => {
                f.write_str("controller store requires reopen and durable classification")
            }
            Self::Malformed(reason) => write!(f, "malformed controller checkpoint: {reason}"),
        }
    }
}

impl Error for ControllerStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::UnsupportedSchema(_)
            | Self::NonemptyUnversionedDatabase
            | Self::RevisionConflict
            | Self::RevisionExhausted
            | Self::Poisoned
            | Self::Malformed(_) => None,
        }
    }
}

impl From<rusqlite::Error> for ControllerStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

type Result<T> = std::result::Result<T, ControllerStoreError>;

impl ControllerSqliteStore {
    /// Opens only the caller-selected path; no application database discovery occurs.
    ///
    /// # Errors
    /// Rejects unknown schemas, nonempty unversioned databases, and SQLite failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    /// Creates an isolated, ephemeral controller database.
    ///
    /// # Errors
    /// Returns an error if SQLite initialization fails.
    pub fn in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                let objects: i64 = transaction.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE substr(name, 1, 7) != 'sqlite_'",
                    [],
                    |row| row.get(0),
                )?;
                if objects != 0 {
                    return Err(ControllerStoreError::NonemptyUnversionedDatabase);
                }
                transaction.execute_batch(SCHEMA)?;
            }
            SCHEMA_VERSION => {}
            other => return Err(ControllerStoreError::UnsupportedSchema(other)),
        }
        transaction.commit()?;
        Ok(Self {
            connection,
            poisoned: false,
        })
    }

    /// Loads affine recovery evidence under a consistent read transaction.
    /// Only id, phase, and consuming recovery preparation are exposed.
    ///
    /// # Errors
    /// Rejects poisoned handles and malformed, incomplete, unknown, or
    /// phase-inconsistent stored data.
    pub fn load_attempt(&mut self, id: AttemptId) -> Result<Option<LoadedAttempt>> {
        self.ensure_usable()?;
        let result = self.load_attempt_inner(id);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn load_attempt_inner(&mut self, id: AttemptId) -> Result<Option<LoadedAttempt>> {
        let transaction = self.connection.transaction()?;
        check_authority(&transaction)?;
        let attempt = read_attempt(&transaction, id)?;
        transaction.commit()?;
        Ok(attempt.map(|checkpoint| LoadedAttempt { checkpoint }))
    }

    fn ensure_usable(&self) -> Result<()> {
        if self.poisoned {
            Err(ControllerStoreError::Poisoned)
        } else {
            Ok(())
        }
    }

    fn persist_checkpoint(
        &mut self,
        checkpoint: &DeploymentAttempt,
    ) -> Result<PersistenceRevision> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_authority(&transaction)?;
        let durable = read_attempt(&transaction, checkpoint.attempt_id())?;
        let expected = checkpoint.persistence_revision();
        let matches = match (&durable, expected) {
            (None, None) => true,
            (Some(durable), Some(expected)) => durable.persistence_revision() == Some(expected),
            (None, Some(_)) | (Some(_), None) => false,
        };
        if !matches {
            return Err(ControllerStoreError::RevisionConflict);
        }
        let revision = match expected {
            None => PersistenceRevision::first(),
            Some(revision) => revision
                .checked_next()
                .ok_or(ControllerStoreError::RevisionExhausted)?,
        };
        let mut next = checkpoint.clone();
        next.set_revision(revision);
        // BEGIN IMMEDIATE holds the writer lock across compare, replace, and commit.
        transaction.execute(
            "DELETE FROM attempts WHERE attempt_id = ?1",
            [key(checkpoint.attempt_id())],
        )?;
        write_attempt(&transaction, &next)?;
        if read_attempt(&transaction, checkpoint.attempt_id())?.as_ref() != Some(&next) {
            return malformed("checkpoint did not round-trip losslessly");
        }
        transaction.commit()?;
        Ok(revision)
    }
}

impl AttemptStore for ControllerSqliteStore {
    type Error = ControllerStoreError;

    fn persist(&mut self, checkpoint: &DeploymentAttempt) -> Result<PersistenceRevision> {
        self.ensure_usable()?;
        let result = self.persist_checkpoint(checkpoint);
        if result.is_err() {
            // This also covers errors from commit or an unobserved rollback failure.
            self.poisoned = true;
        }
        result
    }
}

fn check_authority(connection: &Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(ControllerStoreError::UnsupportedSchema(version));
    }
    let foreign_keys: bool =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if !foreign_keys {
        return malformed("foreign keys disabled");
    }
    if connection
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return malformed("foreign key violation");
    }
    Ok(())
}

fn malformed<T>(reason: &'static str) -> Result<T> {
    Err(ControllerStoreError::Malformed(reason))
}
fn required<T>(value: Option<T>, reason: &'static str) -> Result<T> {
    value.ok_or(ControllerStoreError::Malformed(reason))
}
fn key(id: AttemptId) -> Vec<u8> {
    id.get().to_be_bytes().to_vec()
}

/// Unsigned words are fixed-width big-endian blobs, never signed INTEGER casts.
#[derive(Clone, Copy, Debug)]
struct Word(u64);
impl ToSql for Word {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Blob(
            self.0.to_be_bytes().to_vec(),
        )))
    }
}
impl FromSql for Word {
    fn column_result(value: ValueRef<'_>) -> std::result::Result<Self, FromSqlError> {
        let bytes: [u8; 8] = value.as_blob()?.try_into().map_err(|_| {
            FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsigned word must have eight bytes",
            )))
        })?;
        Ok(Self(u64::from_be_bytes(bytes)))
    }
}

#[derive(Debug)]
struct Header {
    deadline: MonotonicTick,
    revision: PersistenceRevision,
    phase: Phase,
    interrupted: Option<Phase>,
    resume: Option<RecoveryResume>,
    cleanup: Option<CleanupOutcome>,
    block: Option<ResourceBlock>,
    verification: Option<CandidateVerification>,
    migration_invoked: bool,
}

impl Header {
    fn from_attempt(attempt: &DeploymentAttempt) -> Result<Self> {
        let active = match attempt {
            DeploymentAttempt::Active(active) => Some(active),
            DeploymentAttempt::Interrupted(interrupted) => Some(&interrupted.resume),
            DeploymentAttempt::Ready(_)
            | DeploymentAttempt::ResourceBlocked(_)
            | DeploymentAttempt::ForwardRecovery(_)
            | DeploymentAttempt::AdministrativeCloseoutRequired(_)
            | DeploymentAttempt::AdministrativelyClosed(_) => None,
        };
        let interrupted = if let DeploymentAttempt::Interrupted(interrupted) = attempt {
            Some(active_phase(&interrupted.resume))
        } else {
            None
        };
        let (resume, cleanup) = match active {
            Some(ActiveAttempt::Ownership(owner)) => (owner.recovery_resume, Some(owner.cleanup)),
            Some(ActiveAttempt::ResourceSampling(_)) | None => (None, None),
        };
        Ok(Self {
            deadline: attempt.deadline(),
            revision: required(
                attempt.persistence_revision(),
                "unpersisted checkpoint header",
            )?,
            phase: attempt.phase(),
            interrupted,
            resume,
            cleanup,
            block: attempt.resource_block_reason(),
            verification: attempt.candidate_verification(),
            migration_invoked: matches!(
                attempt.compatibility_boundary(),
                CompatibilityBoundary::CandidateMigrationInvoked(_)
            ),
        })
    }
}

fn active_phase(active: &ActiveAttempt) -> Phase {
    match active {
        ActiveAttempt::ResourceSampling(_) => Phase::ResourceSampling,
        ActiveAttempt::Ownership(owner) => ownership_phase(owner.phase),
    }
}

fn write_attempt(connection: &Connection, attempt: &DeploymentAttempt) -> Result<()> {
    let id = key(attempt.attempt_id());
    let header = Header::from_attempt(attempt)?;
    connection.execute(
        "INSERT INTO attempts (attempt_id,deadline,phase,interrupted_phase,recovery_resume,ownership_cleanup,block_reason,verification,compatibility,revision) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            id,
            Word(header.deadline.get()),
            phase_text(header.phase),
            header.interrupted.map(phase_text),
            header.resume.map(resume_text),
            header.cleanup.map(cleanup_text),
            header.block.map(block_text),
            header.verification.map(verification_text),
            if header.migration_invoked {
                "candidate_invoked"
            } else {
                "pre_migration"
            },
            Word(header.revision.get())
        ],
    )?;
    if let CompatibilityBoundary::CandidateMigrationInvoked(candidate) =
        attempt.compatibility_boundary()
    {
        connection.execute(
            "INSERT INTO retained_candidates VALUES (?1,?2,?3)",
            params![
                id,
                candidate.source_commit.bytes().as_slice(),
                candidate.artifact_digest.bytes().as_slice()
            ],
        )?;
    }
    let sampling = match attempt {
        DeploymentAttempt::Active(ActiveAttempt::ResourceSampling(sampling))
        | DeploymentAttempt::Interrupted(InterruptedAttempt {
            resume: ActiveAttempt::ResourceSampling(sampling),
            ..
        }) => Some(sampling),
        DeploymentAttempt::Active(ActiveAttempt::Ownership(_))
        | DeploymentAttempt::Interrupted(InterruptedAttempt {
            resume: ActiveAttempt::Ownership(_),
            ..
        })
        | DeploymentAttempt::Ready(_)
        | DeploymentAttempt::ResourceBlocked(_)
        | DeploymentAttempt::ForwardRecovery(_)
        | DeploymentAttempt::AdministrativeCloseoutRequired(_)
        | DeploymentAttempt::AdministrativelyClosed(_) => None,
    };
    let identities = tree::write(
        connection,
        &id,
        sampling.map(|value| value.root),
        attempt.ownership_tree(),
    )?;
    if let Some(sampling) = sampling {
        resources::write_policy(connection, &id, sampling.window.policy())?;
        resources::write_samples(connection, &id, sampling.window.samples())?;
        connection.execute(
            "INSERT INTO sampling_obligations VALUES (?1,?2,?3)",
            params![
                id,
                tree::ordinal(&identities, sampling.root)?,
                Word(sampling.pending_sample_sequence)
            ],
        )?;
    } else if let Some(evidence) = attempt.resource_evidence() {
        resources::write_policy(connection, &id, evidence.policy())?;
        resources::write_samples(connection, &id, evidence.samples())?;
    }
    Ok(())
}

fn read_header(connection: &Connection, id: &[u8]) -> Result<Option<Header>> {
    let row = connection.query_row("SELECT deadline,phase,interrupted_phase,recovery_resume,ownership_cleanup,block_reason,verification,compatibility,revision FROM attempts WHERE attempt_id=?1", [id], |row| {
        Ok((row.get::<_, Word>(0)?.0, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?, row.get::<_, Option<String>>(6)?, row.get::<_, String>(7)?, row.get::<_, Word>(8)?.0))
    }).optional()?;
    let Some((
        deadline,
        phase,
        interrupted,
        resume,
        cleanup,
        block,
        verification,
        compatibility,
        revision,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(Header {
        deadline: MonotonicTick::new(deadline),
        revision: PersistenceRevision::new(required(
            std::num::NonZeroU64::new(revision),
            "zero persistence revision",
        )?),
        phase: parse_phase(&phase)?,
        migration_invoked: match compatibility.as_str() {
            "pre_migration" => false,
            "candidate_invoked" => true,
            _ => return malformed("unknown compatibility boundary"),
        },
        interrupted: interrupted.as_deref().map(parse_phase).transpose()?,
        resume: resume.as_deref().map(parse_resume).transpose()?,
        cleanup: cleanup.as_deref().map(parse_cleanup).transpose()?,
        block: block.as_deref().map(parse_block).transpose()?,
        verification: verification
            .as_deref()
            .map(parse_verification)
            .transpose()?,
    }))
}

fn read_attempt(
    connection: &Connection,
    attempt_id: AttemptId,
) -> Result<Option<DeploymentAttempt>> {
    let id = key(attempt_id);
    let Some(header) = read_header(connection, &id)? else {
        return Ok(None);
    };
    let candidate = read_candidate(connection, &id)?;
    if header.migration_invoked != candidate.is_some() {
        return malformed("retained candidate contradicts compatibility boundary");
    }
    let context = AttemptContext {
        id: attempt_id,
        revision: Some(header.revision),
        deadline: header.deadline,
        compatibility: candidate.map_or(
            CompatibilityBoundary::PreMigration,
            CompatibilityBoundary::CandidateMigrationInvoked,
        ),
    };
    validate_header(&header, candidate)?;
    let effective = header.interrupted.unwrap_or(header.phase);
    let owner_phase = parse_ownership_phase(effective);
    let policy = resources::read_policy(connection, &id)?;
    let samples = resources::read_samples(connection, &id)?;
    let mut identities = tree::read_identities(connection, &id)?;
    let sampling = connection
        .query_row(
            "SELECT root_ordinal,pending_sequence FROM sampling_obligations WHERE attempt_id=?1",
            [&id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Word>(1)?.0)),
        )
        .optional()?;
    let root = sampling
        .map(|(ordinal, _)| identities.resolve(ordinal))
        .transpose()?;
    let tree = tree::read(connection, &id, &mut identities)?;
    identities.finish()?;
    let needs_sampling = effective == Phase::ResourceSampling;
    let needs_tree = owner_phase.is_some()
        || matches!(
            effective,
            Phase::ReadyForDisruption
                | Phase::AdministrativeCloseoutRequired
                | Phase::AdministrativelyClosed
        );
    let needs_resources = needs_sampling || needs_tree;
    if needs_resources != policy.is_some()
        || needs_sampling != sampling.is_some()
        || needs_tree != tree.is_some()
        || (!needs_resources && !samples.is_empty())
    {
        return malformed("phase child-table mismatch");
    }
    let attempt = if effective == Phase::ResourceSampling {
        let (_, pending) = required(sampling, "missing sampling obligation")?;
        validate_sampling_obligation(pending, &samples)?;
        let window = required(
            ResourceWindow::restore(required(policy, "missing policy")?, samples),
            "invalid resource window",
        )?;
        DeploymentAttempt::Active(ActiveAttempt::ResourceSampling(ResourceSamplingAttempt {
            context,
            root: required(root, "missing root")?,
            pending_sample_sequence: pending,
            window,
        }))
    } else if let Some(phase) = owner_phase {
        let cleanup = required(header.cleanup, "missing ownership cleanup")?;
        let tree = required(tree, "missing ownership")?;
        validate_ownership(phase, header.resume, cleanup, &tree)?;
        DeploymentAttempt::Active(ActiveAttempt::Ownership(OwnershipAttempt {
            context,
            resources: required(
                SustainedResourceEvidence::restore(
                    required(policy, "missing admission policy")?,
                    samples,
                ),
                "invalid sustained evidence",
            )?,
            tree,
            phase,
            recovery_resume: header.resume,
            cleanup,
        }))
    } else {
        read_terminal(context, &header, candidate, tree, policy, samples)?
    };
    if header.phase == Phase::Interrupted {
        let DeploymentAttempt::Active(resume) = attempt else {
            return malformed("invalid interruption");
        };
        Ok(Some(DeploymentAttempt::Interrupted(InterruptedAttempt {
            context,
            resume,
        })))
    } else {
        Ok(Some(attempt))
    }
}

fn validate_sampling_obligation(
    pending: u64,
    samples: &[crate::resource::ResourceSample],
) -> Result<()> {
    if pending
        != u64::try_from(samples.len())
            .map_err(|_| ControllerStoreError::Malformed("sample count overflow"))?
        || !samples
            .iter()
            .enumerate()
            .all(|(index, sample)| u64::try_from(index) == Ok(sample.sequence))
    {
        return malformed("sampling obligation does not follow samples");
    }
    Ok(())
}

fn validate_header(header: &Header, candidate: Option<RetainedCandidateIdentity>) -> Result<()> {
    if (header.phase == Phase::Interrupted) != header.interrupted.is_some()
        || (header.phase == Phase::ResourceBlocked) != header.block.is_some()
        || (header.phase != Phase::ForwardRecovery && header.verification.is_some())
        || (header.phase == Phase::ForwardRecovery && candidate.is_none())
        || (candidate.is_some()
            && !matches!(
                header.phase,
                Phase::ReadyForDisruption | Phase::ForwardRecovery
            ))
    {
        return malformed("phase metadata mismatch");
    }
    let effective = header.interrupted.unwrap_or(header.phase);
    if header.interrupted.is_some() && !is_active(effective) {
        return malformed("invalid interruption target");
    }
    let owner_phase = parse_ownership_phase(effective);
    if (effective == Phase::Recovering) != header.resume.is_some()
        || owner_phase.is_some() != header.cleanup.is_some()
    {
        return malformed("ownership/recovery metadata mismatch");
    }
    Ok(())
}

fn validate_ownership(
    phase: OwnershipPhase,
    resume: Option<RecoveryResume>,
    cleanup: CleanupOutcome,
    tree: &crate::ownership::OwnedProcessTree,
) -> Result<()> {
    if !matches!(
        cleanup,
        CleanupOutcome::NotStarted | CleanupOutcome::InProgress
    ) {
        return malformed("invalid ownership cleanup outcome");
    }
    let needs_cleaning = matches!(phase, OwnershipPhase::Cleaning | OwnershipPhase::Verifying)
        || matches!(
            resume,
            Some(RecoveryResume::Cleaning | RecoveryResume::Verifying)
        );
    if needs_cleaning && cleanup != CleanupOutcome::InProgress {
        return malformed("cleanup not started");
    }
    if matches!(phase, OwnershipPhase::Cleaning | OwnershipPhase::Verifying)
        && (!tree.pending_child_scans().is_empty() || !tree.unresolved().is_empty())
    {
        return malformed("cleanup has unfinished discovery");
    }
    Ok(())
}

fn read_terminal(
    context: AttemptContext,
    header: &Header,
    candidate: Option<RetainedCandidateIdentity>,
    tree: Option<crate::ownership::OwnedProcessTree>,
    policy: Option<crate::resource::ResourcePolicy>,
    samples: Vec<crate::resource::ResourceSample>,
) -> Result<DeploymentAttempt> {
    match header.phase {
        Phase::ResourceBlocked | Phase::ForwardRecovery => {
            if !samples.is_empty() {
                return malformed("unexpected resource evidence");
            }
            if header.phase == Phase::ResourceBlocked {
                Ok(DeploymentAttempt::ResourceBlocked(ResourceBlockedAttempt {
                    context,
                    reason: required(header.block, "missing resource block")?,
                }))
            } else {
                Ok(DeploymentAttempt::ForwardRecovery(ForwardRecoveryAttempt {
                    context,
                    candidate: required(candidate, "missing retained candidate")?,
                    verification: header.verification,
                }))
            }
        }
        Phase::ReadyForDisruption
        | Phase::AdministrativeCloseoutRequired
        | Phase::AdministrativelyClosed => {
            let resources = required(
                SustainedResourceEvidence::restore(
                    required(policy, "missing admission policy")?,
                    samples,
                ),
                "invalid sustained evidence",
            )?;
            let tree = required(tree, "missing ownership tree")?;
            if (header.phase == Phase::ReadyForDisruption)
                != (tree.cleanup_readiness() == CleanupReadiness::VerifiedClean)
            {
                return malformed("terminal cleanup evidence contradicts phase");
            }
            match header.phase {
                Phase::ReadyForDisruption => Ok(DeploymentAttempt::Ready(ReadyAttempt {
                    context,
                    resources,
                    tree,
                })),
                Phase::AdministrativeCloseoutRequired => {
                    Ok(DeploymentAttempt::AdministrativeCloseoutRequired(
                        AdministrativeCloseoutRequired {
                            context,
                            resources,
                            tree,
                        },
                    ))
                }
                Phase::AdministrativelyClosed => Ok(DeploymentAttempt::AdministrativelyClosed(
                    AdministrativelyClosed {
                        context,
                        resources,
                        tree,
                    },
                )),
                Phase::ResourceSampling
                | Phase::OwnershipDiscovery
                | Phase::AwaitingBirthEvidence
                | Phase::CleaningOwnedTree
                | Phase::VerifyingCleanup
                | Phase::ResourceBlocked
                | Phase::Interrupted
                | Phase::Recovering
                | Phase::ForwardRecovery => malformed("invalid terminal phase"),
            }
        }
        Phase::ResourceSampling
        | Phase::OwnershipDiscovery
        | Phase::AwaitingBirthEvidence
        | Phase::CleaningOwnedTree
        | Phase::VerifyingCleanup
        | Phase::Interrupted
        | Phase::Recovering => malformed("invalid terminal phase"),
    }
}

fn read_candidate(connection: &Connection, id: &[u8]) -> Result<Option<RetainedCandidateIdentity>> {
    let bytes = connection
        .query_row(
            "SELECT source_commit,artifact_digest FROM retained_candidates WHERE attempt_id=?1",
            [id],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    bytes
        .map(|(commit, digest)| {
            Ok(RetainedCandidateIdentity {
                source_commit: crate::candidate::FullCommitSha::from_bytes(
                    commit
                        .try_into()
                        .map_err(|_| ControllerStoreError::Malformed("invalid commit length"))?,
                ),
                artifact_digest: crate::candidate::ArtifactDigest::from_bytes(
                    digest
                        .try_into()
                        .map_err(|_| ControllerStoreError::Malformed("invalid digest length"))?,
                ),
            })
        })
        .transpose()
}

fn is_active(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::ResourceSampling
            | Phase::OwnershipDiscovery
            | Phase::AwaitingBirthEvidence
            | Phase::CleaningOwnedTree
            | Phase::VerifyingCleanup
            | Phase::Recovering
    )
}

const fn ownership_phase(phase: OwnershipPhase) -> Phase {
    match phase {
        OwnershipPhase::Discovery => Phase::OwnershipDiscovery,
        OwnershipPhase::AwaitingBirthEvidence => Phase::AwaitingBirthEvidence,
        OwnershipPhase::Cleaning => Phase::CleaningOwnedTree,
        OwnershipPhase::Verifying => Phase::VerifyingCleanup,
        OwnershipPhase::Recovering => Phase::Recovering,
    }
}
fn parse_ownership_phase(phase: Phase) -> Option<OwnershipPhase> {
    match phase {
        Phase::OwnershipDiscovery => Some(OwnershipPhase::Discovery),
        Phase::AwaitingBirthEvidence => Some(OwnershipPhase::AwaitingBirthEvidence),
        Phase::CleaningOwnedTree => Some(OwnershipPhase::Cleaning),
        Phase::VerifyingCleanup => Some(OwnershipPhase::Verifying),
        Phase::Recovering => Some(OwnershipPhase::Recovering),
        Phase::ResourceSampling
        | Phase::ReadyForDisruption
        | Phase::ResourceBlocked
        | Phase::Interrupted
        | Phase::ForwardRecovery
        | Phase::AdministrativeCloseoutRequired
        | Phase::AdministrativelyClosed => None,
    }
}

const fn phase_text(phase: Phase) -> &'static str {
    match phase {
        Phase::ResourceSampling => "sampling",
        Phase::OwnershipDiscovery => "discovery",
        Phase::AwaitingBirthEvidence => "awaiting",
        Phase::CleaningOwnedTree => "cleaning",
        Phase::VerifyingCleanup => "verifying",
        Phase::Recovering => "recovering",
        Phase::ReadyForDisruption => "ready",
        Phase::ResourceBlocked => "blocked",
        Phase::Interrupted => "interrupted",
        Phase::ForwardRecovery => "forward",
        Phase::AdministrativeCloseoutRequired => "closeout",
        Phase::AdministrativelyClosed => "closed",
    }
}
fn parse_phase(value: &str) -> Result<Phase> {
    match value {
        "sampling" => Ok(Phase::ResourceSampling),
        "discovery" => Ok(Phase::OwnershipDiscovery),
        "awaiting" => Ok(Phase::AwaitingBirthEvidence),
        "cleaning" => Ok(Phase::CleaningOwnedTree),
        "verifying" => Ok(Phase::VerifyingCleanup),
        "recovering" => Ok(Phase::Recovering),
        "ready" => Ok(Phase::ReadyForDisruption),
        "blocked" => Ok(Phase::ResourceBlocked),
        "interrupted" => Ok(Phase::Interrupted),
        "forward" => Ok(Phase::ForwardRecovery),
        "closeout" => Ok(Phase::AdministrativeCloseoutRequired),
        "closed" => Ok(Phase::AdministrativelyClosed),
        _ => malformed("unknown phase"),
    }
}
const fn resume_text(value: RecoveryResume) -> &'static str {
    match value {
        RecoveryResume::Discovery => "discovery",
        RecoveryResume::AwaitingBirthEvidence => "awaiting",
        RecoveryResume::Cleaning => "cleaning",
        RecoveryResume::Verifying => "verifying",
    }
}
fn parse_resume(value: &str) -> Result<RecoveryResume> {
    match value {
        "discovery" => Ok(RecoveryResume::Discovery),
        "awaiting" => Ok(RecoveryResume::AwaitingBirthEvidence),
        "cleaning" => Ok(RecoveryResume::Cleaning),
        "verifying" => Ok(RecoveryResume::Verifying),
        _ => malformed("unknown recovery resume"),
    }
}
fn cleanup_text(value: CleanupOutcome) -> &'static str {
    match value {
        CleanupOutcome::NotStarted => "not_started",
        CleanupOutcome::InProgress => "in_progress",
        CleanupOutcome::VerifiedClean
        | CleanupOutcome::UnresolvedRisk
        | CleanupOutcome::AdministrativeRiskAccepted => "invalid",
    }
}
fn parse_cleanup(value: &str) -> Result<CleanupOutcome> {
    match value {
        "not_started" => Ok(CleanupOutcome::NotStarted),
        "in_progress" => Ok(CleanupOutcome::InProgress),
        _ => malformed("unknown ownership cleanup"),
    }
}
const fn block_text(value: ResourceBlock) -> &'static str {
    match value {
        ResourceBlock::MemoryCapacity => "memory",
        ResourceBlock::StorageCapacity => "storage",
        ResourceBlock::CpuContention => "cpu",
        ResourceBlock::MemoryPressure => "pressure",
    }
}
fn parse_block(value: &str) -> Result<ResourceBlock> {
    match value {
        "memory" => Ok(ResourceBlock::MemoryCapacity),
        "storage" => Ok(ResourceBlock::StorageCapacity),
        "cpu" => Ok(ResourceBlock::CpuContention),
        "pressure" => Ok(ResourceBlock::MemoryPressure),
        _ => malformed("unknown resource block"),
    }
}
const fn verification_text(value: CandidateVerification) -> &'static str {
    match value {
        CandidateVerification::Exact => "exact",
        CandidateVerification::Missing => "missing",
        CandidateVerification::Mismatch => "mismatch",
        CandidateVerification::Unavailable => "unavailable",
    }
}
fn parse_verification(value: &str) -> Result<CandidateVerification> {
    match value {
        "exact" => Ok(CandidateVerification::Exact),
        "missing" => Ok(CandidateVerification::Missing),
        "mismatch" => Ok(CandidateVerification::Mismatch),
        "unavailable" => Ok(CandidateVerification::Unavailable),
        _ => malformed("unknown candidate verification"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttemptId, ControllerSqliteStore, ControllerStoreError, DeploymentAttempt, MonotonicTick,
        Word,
    };
    use crate::{BirthToken, ProcessId, ProcessIdentity, ResourcePolicy};
    use std::num::{NonZeroU32, NonZeroUsize};

    fn start() -> super::super::Transition {
        let root = ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(42).unwrap()),
            BirthToken::new(1, 0).unwrap(),
        );
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            crate::CpuContentionBasisPoints::new(0).unwrap(),
            crate::MemoryPressure::Critical,
        );
        DeploymentAttempt::start(AttemptId::new(1), MonotonicTick::new(100), policy, root)
    }

    #[test]
    fn foreign_keys_are_enabled_on_the_actual_controller_connection() {
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        assert!(store
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
            .unwrap());
        assert!(store
            .connection
            .execute(
                "INSERT INTO pending_child_scans VALUES (zeroblob(16),0)",
                []
            )
            .is_err());
        store
            .connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        let error = start().persist(&mut store).unwrap_err();
        assert!(matches!(
            error,
            ControllerStoreError::Malformed("foreign keys disabled")
        ));
        assert!(matches!(
            start().persist(&mut store),
            Err(ControllerStoreError::Poisoned)
        ));
        assert!(matches!(
            store.load_attempt(AttemptId::new(1)),
            Err(ControllerStoreError::Poisoned)
        ));
    }

    #[test]
    fn schema_error_drops_transition_and_poisons_handle() {
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        store
            .connection
            .execute_batch("DROP TABLE resource_policies")
            .unwrap();
        let error = start().persist(&mut store).unwrap_err();
        assert!(matches!(error, ControllerStoreError::Sqlite(_)));
        assert!(matches!(
            start().persist(&mut store),
            Err(ControllerStoreError::Poisoned)
        ));
        assert!(matches!(
            store.load_attempt(AttemptId::new(1)),
            Err(ControllerStoreError::Poisoned)
        ));
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM attempts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn exact_read_preserves_committed_revision_in_every_interrupted_context() {
        let mut store = ControllerSqliteStore::in_memory().unwrap();
        let transition = start();
        assert_eq!(transition.checkpoint.persistence_revision(), None);
        let committed = transition.persist(&mut store).unwrap();
        assert_eq!(
            committed.checkpoint().persistence_revision(),
            Some(super::PersistenceRevision::first())
        );
        assert_eq!(
            super::read_attempt(&store.connection, AttemptId::new(1))
                .unwrap()
                .as_ref(),
            Some(committed.checkpoint())
        );
        let committed = committed
            .into_parts()
            .0
            .interrupt()
            .unwrap()
            .persist(&mut store)
            .unwrap();
        assert_eq!(
            committed.checkpoint().persistence_revision().unwrap().get(),
            2
        );
        assert_eq!(
            super::read_attempt(&store.connection, AttemptId::new(1))
                .unwrap()
                .as_ref(),
            Some(committed.checkpoint())
        );
        let committed = committed
            .into_parts()
            .0
            .recover()
            .unwrap()
            .persist(&mut store)
            .unwrap();
        assert_eq!(
            committed.checkpoint().persistence_revision().unwrap().get(),
            3
        );
        assert_eq!(
            super::read_attempt(&store.connection, AttemptId::new(1))
                .unwrap()
                .as_ref(),
            Some(committed.checkpoint())
        );
    }

    #[test]
    fn unsigned_word_codec_is_total_and_rejects_integer_or_wrong_width() {
        let store = ControllerSqliteStore::in_memory().unwrap();
        for value in [
            0,
            1,
            i64::MAX.unsigned_abs(),
            i64::MAX.unsigned_abs() + 1,
            u64::MAX,
        ] {
            let restored = store
                .connection
                .query_row("SELECT ?1", [Word(value)], |row| row.get::<_, Word>(0))
                .unwrap();
            assert_eq!(restored.0, value);
        }
        for query in ["SELECT 1", "SELECT zeroblob(7)", "SELECT zeroblob(9)"] {
            assert!(store
                .connection
                .query_row(query, [], |row| row.get::<_, Word>(0))
                .is_err());
        }
    }
}
