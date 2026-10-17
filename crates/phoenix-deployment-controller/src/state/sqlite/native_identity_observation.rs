use std::num::{NonZeroU32, NonZeroU64};

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{
    binding_revision, check_authority, key, malformed, read_attempt, read_binding, read_header,
    required, ControllerSqliteStore, ControllerStoreError, Result, Word,
};
use crate::{
    BirthToken, DeploymentAttempt, DispatchKey, DispatchTicket, IdentityObservation, LoadedAttempt,
    MonotonicTick, NativeGuardianBinding, NativeGuardianIdentity, NativeSessionState,
    ObservationFailure, OsEffect, PersistenceRevision, Phase, ProcessId, ProcessIdentity,
};

/// Diagnostic identity; it cannot reconstruct a policy-issued ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeIdentityObservationIssue {
    key: DispatchKey,
    session_generation: NonZeroU64,
    guardian_identity: NativeGuardianIdentity,
    target: ProcessIdentity,
}
impl NativeIdentityObservationIssue {
    #[must_use]
    pub const fn key(self) -> DispatchKey {
        self.key
    }
    #[must_use]
    pub const fn target(self) -> ProcessIdentity {
        self.target
    }
    #[must_use]
    pub const fn session_generation(self) -> NonZeroU64 {
        self.session_generation
    }
    #[must_use]
    pub const fn guardian_identity(self) -> NativeGuardianIdentity {
        self.guardian_identity
    }
}

/// Affine custody of the original PolicyLoop-issued ObserveIdentity ticket.
#[derive(Debug)]
pub struct NativeIdentityObservationDispatch {
    ticket: DispatchTicket,
    issue: NativeIdentityObservationIssue,
}
impl NativeIdentityObservationDispatch {
    #[allow(dead_code)]
    pub(crate) fn new(ticket: DispatchTicket, issue: NativeIdentityObservationIssue) -> Self {
        Self { ticket, issue }
    }
    pub(crate) fn into_parts(self) -> (DispatchTicket, NativeIdentityObservationIssue) {
        (self.ticket, self.issue)
    }
}

#[derive(Debug)]
pub struct VerifiedNativeIdentityObservationCompletion {
    dispatch: NativeIdentityObservationDispatch,
    observation: IdentityObservation,
}
impl VerifiedNativeIdentityObservationCompletion {
    #[allow(dead_code)]
    pub(crate) fn new(
        dispatch: NativeIdentityObservationDispatch,
        observation: IdentityObservation,
    ) -> Self {
        Self {
            dispatch,
            observation,
        }
    }
    pub(crate) fn into_parts(
        self,
    ) -> (
        DispatchTicket,
        NativeIdentityObservationIssue,
        IdentityObservation,
    ) {
        let (ticket, issue) = self.dispatch.into_parts();
        (ticket, issue, self.observation)
    }
}

/// One-use ACK proof from a verified native result; diagnostic rows never produce one.
#[derive(Debug)]
pub struct VerifiedNativeIdentityObservationAckConfirmation {
    issue: NativeIdentityObservationIssue,
    observation: IdentityObservation,
    applying_revision: PersistenceRevision,
    confirming_session_generation: NonZeroU64,
}
impl VerifiedNativeIdentityObservationAckConfirmation {
    #[allow(dead_code)]
    pub(crate) fn new(
        issue: NativeIdentityObservationIssue,
        observation: IdentityObservation,
        applying_revision: PersistenceRevision,
        confirming_session_generation: NonZeroU64,
    ) -> Self {
        Self {
            issue,
            observation,
            applying_revision,
            confirming_session_generation,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeIdentityObservationStatus {
    IssuedPendingApplication {
        issue: NativeIdentityObservationIssue,
    },
    AppliedPendingAck {
        issue: NativeIdentityObservationIssue,
        observation: IdentityObservation,
        applying_revision: PersistenceRevision,
    },
    AckConfirmed {
        issue: NativeIdentityObservationIssue,
        observation: IdentityObservation,
        applying_revision: PersistenceRevision,
        confirming_session_generation: NonZeroU64,
    },
}

fn parts(issue: NativeIdentityObservationIssue) -> Result<(Vec<u8>, Word, i64)> {
    Ok((
        key(issue.key.attempt_id()),
        Word(issue.key.revision().get()),
        i64::try_from(issue.key.ordinal()).map_err(|_| ControllerStoreError::RevisionConflict)?,
    ))
}
fn decode_identity(pid: i64, seconds: u64, nanos: i64) -> Result<ProcessIdentity> {
    let pid = NonZeroU32::new(
        u32::try_from(pid).map_err(|_| ControllerStoreError::Malformed("invalid identity pid"))?,
    )
    .ok_or(ControllerStoreError::Malformed("zero identity pid"))?;
    let birth = BirthToken::new(
        seconds,
        u32::try_from(nanos).map_err(|_| ControllerStoreError::Malformed("invalid birth nanos"))?,
    )
    .map_err(|_| ControllerStoreError::Malformed("invalid birth nanos"))?;
    Ok(ProcessIdentity::new(ProcessId::new(pid), birth))
}
fn encode_identity(identity: Option<ProcessIdentity>) -> (Option<u32>, Option<Word>, Option<u32>) {
    match identity {
        Some(identity) => (
            Some(identity.pid().get()),
            Some(Word(identity.birth().seconds())),
            Some(identity.birth().nanos()),
        ),
        None => (None, None, None),
    }
}
fn decode_optional(
    pid: Option<i64>,
    seconds: Option<Word>,
    nanos: Option<i64>,
) -> Result<Option<ProcessIdentity>> {
    match (pid, seconds, nanos) {
        (None, None, None) => Ok(None),
        (Some(pid), Some(seconds), Some(nanos)) => {
            Ok(Some(decode_identity(pid, seconds.0, nanos)?))
        }
        _ => malformed("partial observed identity"),
    }
}

impl ControllerSqliteStore {
    /// Persists only a policy-issued exact owned-root observation, without consuming the ticket.
    /// # Errors
    /// Stale, expired or mismatched authority poisons this handle.
    pub fn commit_native_identity_observation_issue(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
    ) -> Result<NativeIdentityObservationIssue> {
        self.commit_native_identity_observation_issue_with_clock(
            ticket,
            expected,
            super::native_sampling::native_deadline_now,
        )
    }
    #[cfg(test)]
    pub(crate) fn commit_native_identity_observation_issue_at(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
        now: MonotonicTick,
    ) -> Result<NativeIdentityObservationIssue> {
        self.commit_native_identity_observation_issue_with_clock(ticket, expected, || Ok(now))
    }
    fn commit_native_identity_observation_issue_with_clock(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
        mut clock: impl FnMut() -> Result<MonotonicTick>,
    ) -> Result<NativeIdentityObservationIssue> {
        self.ensure_usable()?;
        let result = (|| {
            let OsEffect::ObserveIdentity { identity } = ticket.effect() else {
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
            let header = required(
                read_header(&tx, &key(dispatch.attempt_id()))?,
                "missing native attempt",
            )?;
            let checkpoint = required(
                read_attempt(&tx, dispatch.attempt_id())?,
                "missing native checkpoint",
            )?;
            let session = binding.current_session();
            if binding != *expected
                || binding.attempt_id() != dispatch.attempt_id()
                || session.state != NativeSessionState::Active
                || binding.dispatch_floor() > dispatch.revision()
                || header.revision != dispatch.revision()
                || !matches!(
                    header.phase,
                    Phase::CleaningOwnedTree | Phase::VerifyingCleanup
                )
                || header.deadline != binding.deadline()
                || clock()? >= header.deadline
                || binding.slots().len() != 1
                || binding.slots()[0].process != *identity
                || !checkpoint.native_root_identity_observation(*identity)
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let issue = NativeIdentityObservationIssue {
                key: dispatch,
                session_generation: session.generation,
                guardian_identity: binding.identity(),
                target: *identity,
            };
            let (id, revision, ordinal) = parts(issue)?;
            tx.execute(
                "INSERT INTO native_identity_observation_issues VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    revision,
                    ordinal,
                    Word(session.generation.get()),
                    identity.pid().get(),
                    Word(identity.birth().seconds()),
                    identity.birth().nanos()
                ],
            )?;
            if clock()? >= header.deadline {
                return Err(ControllerStoreError::RevisionConflict);
            }
            tx.commit()?;
            Ok(issue)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Reads diagnostic evidence only, without granting dispatch or ACK authority.
    /// # Errors
    /// Malformed or contradictory evidence poisons this handle.
    pub fn load_native_identity_observation_status(
        &mut self,
        dispatch: DispatchKey,
    ) -> Result<Option<NativeIdentityObservationStatus>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let status = read_status(&tx, dispatch)?;
            tx.commit()?;
            Ok(status)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Reads checkpoint, binding and diagnostic status from a single snapshot.
    /// # Errors
    /// Partial evidence poisons this handle.
    pub fn read_native_identity_observation_evidence(
        &mut self,
        dispatch: DispatchKey,
    ) -> Result<
        Option<(
            LoadedAttempt,
            NativeGuardianBinding,
            NativeIdentityObservationStatus,
        )>,
    > {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let value = match (
                read_attempt(&tx, dispatch.attempt_id())?,
                read_binding(&tx, dispatch.attempt_id())?,
                read_status(&tx, dispatch)?,
            ) {
                (Some(checkpoint), Some(binding), Some(status)) => {
                    Some((LoadedAttempt { checkpoint }, binding, status))
                }
                (Some(_), Some(_), None) | (None, None, None) => None,
                _ => return malformed("partial native identity evidence"),
            };
            tx.commit()?;
            Ok(value)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Returns at most one applied identity observation awaiting ACK.
    /// # Errors
    /// Ambiguous or malformed evidence poisons this handle.
    pub fn pending_native_identity_observation_ack(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(DispatchKey, NativeIdentityObservationStatus)>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let mut statement = tx.prepare("SELECT dispatch_revision,ordinal FROM native_identity_observation_issues WHERE attempt_id=?1")?;
            let mut pending = None;
            for row in statement.query_map([key(attempt)], |r| {
                Ok((r.get::<_, Word>(0)?.0, r.get::<_, i64>(1)?))
            })? {
                let (revision, ordinal) = row?;
                let dispatch = DispatchKey::from_committed_parts(
                    attempt,
                    binding_revision(revision)?,
                    usize::try_from(ordinal)
                        .map_err(|_| ControllerStoreError::Malformed("invalid issue ordinal"))?,
                );
                match read_status(&tx, dispatch)? {
                    Some(status @ NativeIdentityObservationStatus::AppliedPendingAck { .. }) => {
                        if pending.replace((dispatch, status)).is_some() {
                            return malformed("multiple pending identity ACKs");
                        }
                    }
                    Some(_) => {}
                    None => return malformed("missing native identity issue"),
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

    /// Confirms the exact applied observation in an active or reconciling same-guardian session.
    /// # Errors
    /// Mismatch or replay poisons the handle.
    pub fn confirm_native_identity_observation_ack(
        &mut self,
        proof: VerifiedNativeIdentityObservationAckConfirmation,
    ) -> Result<NativeIdentityObservationStatus> {
        self.ensure_usable()?;
        let result = (|| {
            let VerifiedNativeIdentityObservationAckConfirmation {
                issue,
                observation,
                applying_revision,
                confirming_session_generation,
            } = proof;
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
                || header.deadline != binding.deadline()
                || header.revision < applying_revision
                || read_status(&tx, issue.key)?
                    != Some(NativeIdentityObservationStatus::AppliedPendingAck {
                        issue,
                        observation,
                        applying_revision,
                    })
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let (id, revision, ordinal) = parts(issue)?;
            tx.execute(
                "INSERT INTO native_identity_observation_ack_confirmations VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    revision,
                    ordinal,
                    Word(applying_revision.get()),
                    Word(confirming_session_generation.get())
                ],
            )?;
            let status = NativeIdentityObservationStatus::AckConfirmed {
                issue,
                observation,
                applying_revision,
                confirming_session_generation,
            };
            if read_status(&tx, issue.key)? != Some(status.clone()) {
                return malformed("identity ACK did not round trip");
            }
            tx.commit()?;
            Ok(status)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub(crate) fn persist_native_identity_observation(
        &mut self,
        checkpoint: &DeploymentAttempt,
        issue: &NativeIdentityObservationIssue,
        observation: &IdentityObservation,
    ) -> Result<PersistenceRevision> {
        self.persist_native_identity_observation_with_clock(
            checkpoint,
            issue,
            observation,
            &mut super::native_sampling::native_deadline_now,
        )
    }
    pub(crate) fn persist_native_identity_observation_with_clock(
        &mut self,
        checkpoint: &DeploymentAttempt,
        issue: &NativeIdentityObservationIssue,
        observation: &IdentityObservation,
        clock: &mut (impl FnMut() -> Result<MonotonicTick> + ?Sized),
    ) -> Result<PersistenceRevision> {
        self.ensure_usable()?;
        let result = self.persist_checkpoint_with_recovery(
            checkpoint,
            None,
            None,
            None,
            None,
            None,
            Some((issue, observation)),
            clock,
        );
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

pub(super) fn validate_application(
    tx: &rusqlite::Transaction<'_>,
    durable: Option<&DeploymentAttempt>,
    checkpoint: &DeploymentAttempt,
    issue: &NativeIdentityObservationIssue,
    observation: &IdentityObservation,
) -> Result<()> {
    let old = required(durable, "missing identity checkpoint")?;
    let binding = required(
        read_binding(tx, issue.key.attempt_id())?,
        "missing identity binding",
    )?;
    if old.attempt_id() != issue.key.attempt_id()
        || old.persistence_revision() != Some(issue.key.revision())
        || binding.identity() != issue.guardian_identity
        || binding.current_session().generation != issue.session_generation
        || binding.current_session().state != NativeSessionState::Active
        || binding.dispatch_floor() > issue.key.revision()
        || binding.slots().len() != 1
        || binding.slots()[0].process != issue.target
        || !old.native_root_identity_observation(issue.target)
        || read_status(tx, issue.key)?
            != Some(NativeIdentityObservationStatus::IssuedPendingApplication { issue: *issue })
        || checkpoint
            != &old
                .clone()
                .identity_observed(issue.target, *observation)
                .map_err(|_| ControllerStoreError::RevisionConflict)?
                .checkpoint
    {
        return Err(ControllerStoreError::RevisionConflict);
    }
    Ok(())
}

pub(super) fn insert_application(
    tx: &rusqlite::Transaction<'_>,
    issue: &NativeIdentityObservationIssue,
    observation: &IdentityObservation,
    applying_revision: PersistenceRevision,
) -> Result<()> {
    let (parent, replacement, outcome, failure) = match observation {
        IdentityObservation::SameIdentity { current_parent } => {
            (*current_parent, None, "same_identity", None)
        }
        IdentityObservation::ExactIdentityAbsent => (None, None, "exact_identity_absent", None),
        IdentityObservation::PidReused(replacement) => {
            (None, Some(*replacement), "pid_reused", None)
        }
        IdentityObservation::Unresolved(failure) => (
            None,
            None,
            "unresolved",
            Some(match failure {
                ObservationFailure::NoSuchProcess => "no_such_process",
                ObservationFailure::PermissionDenied => "permission_denied",
                ObservationFailure::TemporarilyUnavailable => "temporarily_unavailable",
                ObservationFailure::Malformed => "malformed",
            }),
        ),
    };
    let (parent_pid, parent_seconds, parent_nanos) = encode_identity(parent);
    let (replacement_pid, replacement_seconds, replacement_nanos) = encode_identity(replacement);
    let (id, revision, ordinal) = parts(*issue)?;
    tx.execute("INSERT INTO native_identity_observation_applications VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![id, revision, ordinal, Word(applying_revision.get()), outcome,
            parent_pid, parent_seconds, parent_nanos, replacement_pid, replacement_seconds, replacement_nanos, failure])?;
    if read_status(tx, issue.key)?
        != Some(NativeIdentityObservationStatus::AppliedPendingAck {
            issue: *issue,
            observation: *observation,
            applying_revision,
        })
    {
        return malformed("identity application did not round trip");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) fn read_status(
    connection: &rusqlite::Connection,
    dispatch: DispatchKey,
) -> Result<Option<NativeIdentityObservationStatus>> {
    let id = key(dispatch.attempt_id());
    let ordinal =
        i64::try_from(dispatch.ordinal()).map_err(|_| ControllerStoreError::RevisionConflict)?;
    let row = connection.query_row(
        "SELECT i.session_generation,i.target_pid,i.target_birth_seconds,i.target_birth_nanos, \
         a.applying_revision,a.outcome,a.current_parent_pid,a.current_parent_birth_seconds,a.current_parent_birth_nanos, \
         a.replacement_pid,a.replacement_birth_seconds,a.replacement_birth_nanos,a.failure,c.confirming_session_generation \
         FROM native_identity_observation_issues i LEFT JOIN native_identity_observation_applications a \
         ON a.attempt_id=i.attempt_id AND a.dispatch_revision=i.dispatch_revision AND a.ordinal=i.ordinal \
         LEFT JOIN native_identity_observation_ack_confirmations c \
         ON c.attempt_id=i.attempt_id AND c.dispatch_revision=i.dispatch_revision AND c.ordinal=i.ordinal \
         WHERE i.attempt_id=?1 AND i.dispatch_revision=?2 AND i.ordinal=?3",
        params![id, Word(dispatch.revision().get()), ordinal], |r| Ok((
            (r.get::<_, Word>(0)?.0, r.get::<_, i64>(1)?, r.get::<_, Word>(2)?.0, r.get::<_, i64>(3)?),
            (r.get::<_, Option<Word>>(4)?, r.get::<_, Option<String>>(5)?,
             r.get::<_, Option<i64>>(6)?, r.get::<_, Option<Word>>(7)?, r.get::<_, Option<i64>>(8)?,
             r.get::<_, Option<i64>>(9)?, r.get::<_, Option<Word>>(10)?, r.get::<_, Option<i64>>(11)?,
             r.get::<_, Option<String>>(12)?, r.get::<_, Option<Word>>(13)?)))).optional()?;
    let Some((
        (generation, pid, seconds, nanos),
        (
            applying,
            outcome,
            parent_pid,
            parent_seconds,
            parent_nanos,
            replacement_pid,
            replacement_seconds,
            replacement_nanos,
            failure,
            confirming,
        ),
    )) = row
    else {
        return Ok(None);
    };
    let target = decode_identity(pid, seconds, nanos)?;
    let binding = required(
        read_binding(connection, dispatch.attempt_id())?,
        "missing identity issue binding",
    )?;
    let header = required(
        read_header(connection, &id)?,
        "missing identity issue attempt",
    )?;
    let session_generation = required(NonZeroU64::new(generation), "zero issuing generation")?;
    let issue = NativeIdentityObservationIssue {
        key: dispatch,
        session_generation,
        guardian_identity: binding.identity(),
        target,
    };
    let session = required(
        binding
            .sessions()
            .iter()
            .find(|s| s.generation == session_generation),
        "missing issuing session",
    )?;
    if session.interruption_revision > dispatch.revision()
        || dispatch.revision() > header.revision
        || header.deadline != binding.deadline()
        || binding.slots().len() != 1
        || binding.slots()[0].process != target
    {
        return malformed("invalid native identity issue authority");
    }
    let parent = decode_optional(parent_pid, parent_seconds, parent_nanos)?;
    let replacement = decode_optional(replacement_pid, replacement_seconds, replacement_nanos)?;
    let Some(applying) = applying else {
        if outcome.is_some()
            || parent.is_some()
            || replacement.is_some()
            || failure.is_some()
            || confirming.is_some()
        {
            return malformed("partial identity application");
        }
        return Ok(Some(
            NativeIdentityObservationStatus::IssuedPendingApplication { issue },
        ));
    };
    let applying_revision = binding_revision(applying.0)?;
    if applying_revision <= dispatch.revision() || applying_revision > header.revision {
        return malformed("invalid identity applying revision");
    }
    let observation = match (outcome.as_deref(), parent, replacement, failure.as_deref()) {
        (Some("same_identity"), parent, None, None) => IdentityObservation::SameIdentity {
            current_parent: parent,
        },
        (Some("exact_identity_absent"), None, None, None) => {
            IdentityObservation::ExactIdentityAbsent
        }
        (Some("pid_reused"), None, Some(identity), None)
            if identity.pid() == target.pid() && identity != target =>
        {
            IdentityObservation::PidReused(identity)
        }
        (Some("unresolved"), None, None, Some(failure)) => {
            IdentityObservation::Unresolved(match failure {
                "no_such_process" => ObservationFailure::NoSuchProcess,
                "permission_denied" => ObservationFailure::PermissionDenied,
                "temporarily_unavailable" => ObservationFailure::TemporarilyUnavailable,
                "malformed" => ObservationFailure::Malformed,
                _ => return malformed("invalid observation failure"),
            })
        }
        _ => return malformed("invalid identity observation outcome"),
    };
    if let Some(generation) = confirming {
        let confirming_session_generation =
            required(NonZeroU64::new(generation.0), "zero confirming generation")?;
        if confirming_session_generation < session_generation
            || !binding.sessions().iter().any(|s| {
                s.generation == confirming_session_generation
                    && s.interruption_revision <= header.revision
            })
        {
            return malformed("invalid identity confirming session");
        }
        Ok(Some(NativeIdentityObservationStatus::AckConfirmed {
            issue,
            observation,
            applying_revision,
            confirming_session_generation,
        }))
    } else {
        Ok(Some(NativeIdentityObservationStatus::AppliedPendingAck {
            issue,
            observation,
            applying_revision,
        }))
    }
}

#[cfg(test)]
mod tests;
