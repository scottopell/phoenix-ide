use std::num::{NonZeroU32, NonZeroU64};

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{
    binding_revision, check_authority, key, malformed, read_attempt, read_binding, read_header,
    required, ControllerSqliteStore, ControllerStoreError, Result, Word,
};
use crate::{
    BirthToken, DeploymentAttempt, DispatchKey, DispatchTicket, LoadedAttempt, MonotonicTick,
    NativeGuardianBinding, NativeGuardianIdentity, NativeSessionState, OsEffect, OsQueryError,
    PersistenceRevision, Phase, ProcessId, ProcessIdentity,
};

/// A native PID is the same validated nonzero OS process ID used by the policy engine.
pub type NativePid = ProcessId;

/// Diagnostic evidence only; cannot recreate a dispatch ticket.
/// ```compile_fail
/// use phoenix_deployment_controller::{NativeChildListIssue, DispatchTicket};
/// fn forge(issue: NativeChildListIssue) -> DispatchTicket { issue.into() }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeChildListIssue {
    key: DispatchKey,
    session_generation: NonZeroU64,
    guardian_identity: NativeGuardianIdentity,
    parent: ProcessIdentity,
}
impl NativeChildListIssue {
    #[must_use]
    pub const fn key(self) -> DispatchKey {
        self.key
    }
    #[must_use]
    pub const fn parent(self) -> ProcessIdentity {
        self.parent
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

#[derive(Debug, Eq, PartialEq)]
pub enum NativeChildListCompletion {
    Children(Vec<NativePid>),
    Failure(OsQueryError),
}

/// Affine custody of an admitted effect.
/// ```compile_fail
/// use phoenix_deployment_controller::NativeChildListDispatch;
/// fn duplicate(dispatch: NativeChildListDispatch) { let _ = dispatch.clone(); }
/// ```
#[derive(Debug)]
pub struct NativeChildListDispatch {
    ticket: DispatchTicket,
    issue: NativeChildListIssue,
}
impl NativeChildListDispatch {
    #[allow(dead_code)]
    pub(crate) fn new(ticket: DispatchTicket, issue: NativeChildListIssue) -> Self {
        Self { ticket, issue }
    }

    #[must_use]
    pub(crate) const fn issue(&self) -> NativeChildListIssue {
        self.issue
    }
    pub(crate) fn into_parts(self) -> (DispatchTicket, NativeChildListIssue) {
        (self.ticket, self.issue)
    }
}

#[derive(Debug)]
pub struct VerifiedNativeChildListCompletion {
    dispatch: NativeChildListDispatch,
    completion: NativeChildListCompletion,
}
impl VerifiedNativeChildListCompletion {
    #[allow(dead_code)]
    pub(crate) fn new(
        dispatch: NativeChildListDispatch,
        completion: NativeChildListCompletion,
    ) -> Self {
        Self {
            dispatch,
            completion,
        }
    }
    pub(crate) fn into_parts(
        self,
    ) -> (
        DispatchTicket,
        NativeChildListIssue,
        NativeChildListCompletion,
    ) {
        let (ticket, issue) = self.dispatch.into_parts();
        (ticket, issue, self.completion)
    }
}

/// Private ACK proof; status, issue, and wire data cannot recreate it.
/// ```compile_fail
/// use phoenix_deployment_controller::{NativeChildListStatus, VerifiedNativeChildListAckConfirmation};
/// fn forge(status: NativeChildListStatus) -> VerifiedNativeChildListAckConfirmation { status.into() }
/// ```
#[derive(Debug)]
pub struct VerifiedNativeChildListAckConfirmation {
    issue: NativeChildListIssue,
    children: Vec<NativePid>,
    applying_revision: PersistenceRevision,
    confirming_session_generation: NonZeroU64,
}
impl VerifiedNativeChildListAckConfirmation {
    #[allow(dead_code)]
    pub(crate) fn new(
        issue: NativeChildListIssue,
        children: Vec<NativePid>,
        applying_revision: PersistenceRevision,
        confirming_session_generation: NonZeroU64,
    ) -> Self {
        Self {
            issue,
            children,
            applying_revision,
            confirming_session_generation,
        }
    }
}

/// Store-validated, one-use interrupted child-list evidence; no dispatch or ACK authority.
/// ```compile_fail
/// use phoenix_deployment_controller::NativeChildListRecoveryCandidate;
/// fn duplicate(candidate: NativeChildListRecoveryCandidate) { let _ = candidate.clone(); }
/// ```
#[derive(Debug)]
pub struct NativeChildListRecoveryCandidate {
    loaded: LoadedAttempt,
    binding: NativeGuardianBinding,
    issue: NativeChildListIssue,
}
impl NativeChildListRecoveryCandidate {
    #[must_use]
    pub const fn issue(&self) -> NativeChildListIssue {
        self.issue
    }

    #[must_use]
    pub const fn binding(&self) -> &NativeGuardianBinding {
        &self.binding
    }
}

/// Positive same-guardian live observation, consumed once by durable application.
/// ```compile_fail
/// use phoenix_deployment_controller::VerifiedRecoveredNativeChildList;
/// fn duplicate(proof: VerifiedRecoveredNativeChildList) { let _ = proof.clone(); }
/// ```
#[derive(Debug)]
pub struct VerifiedRecoveredNativeChildList {
    candidate: NativeChildListRecoveryCandidate,
    children: Vec<NativePid>,
}
impl VerifiedRecoveredNativeChildList {
    #[allow(dead_code)]
    pub(crate) fn after_live_verification(
        candidate: NativeChildListRecoveryCandidate,
        children: Vec<NativePid>,
    ) -> Self {
        Self {
            candidate,
            children,
        }
    }
}

/// Loaded result evidence never grants a native execution or ACK capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeChildListStatus {
    IssuedPendingApplication {
        issue: NativeChildListIssue,
    },
    AppliedPendingAck {
        issue: NativeChildListIssue,
        children: Vec<NativePid>,
        applying_revision: PersistenceRevision,
    },
    ChildrenAckConfirmed {
        issue: NativeChildListIssue,
        children: Vec<NativePid>,
        applying_revision: PersistenceRevision,
        confirming_session_generation: NonZeroU64,
    },
    ParkedRetained {
        issue: NativeChildListIssue,
        failure: OsQueryError,
        applying_revision: PersistenceRevision,
    },
}

fn identity_row(
    connection: &rusqlite::Connection,
    attempt: crate::AttemptId,
    parent: ProcessIdentity,
) -> Result<Option<i64>> {
    connection.query_row("SELECT ordinal FROM process_identities WHERE attempt_id=?1 AND pid=?2 AND birth_seconds=?3 AND birth_nanos=?4", params![key(attempt), parent.pid().get(), Word(parent.birth().seconds()), parent.birth().nanos()], |r| r.get(0)).optional().map_err(Into::into)
}

fn parts(issue: NativeChildListIssue) -> (Vec<u8>, Word, i64) {
    (
        key(issue.key.attempt_id()),
        Word(issue.key.revision().get()),
        i64::try_from(issue.key.ordinal()).expect("validated ordinal"),
    )
}

impl ControllerSqliteStore {
    /// Commits one exact root scan issue under a writer lock. Does not consume its ticket.
    /// # Errors
    /// Rejected authority or storage failure poisons the handle.
    pub fn commit_native_child_list_issue(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
    ) -> Result<NativeChildListIssue> {
        self.commit_native_child_list_issue_with_clock(
            ticket,
            expected,
            super::native_sampling::native_deadline_now,
        )
    }
    #[cfg(test)]
    pub(crate) fn commit_native_child_list_issue_at(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
        now: MonotonicTick,
    ) -> Result<NativeChildListIssue> {
        self.commit_native_child_list_issue_with_clock(ticket, expected, || Ok(now))
    }
    fn commit_native_child_list_issue_with_clock(
        &mut self,
        ticket: &DispatchTicket,
        expected: &NativeGuardianBinding,
        mut clock: impl FnMut() -> Result<MonotonicTick>,
    ) -> Result<NativeChildListIssue> {
        self.ensure_usable()?;
        let result = (|| {
            let OsEffect::ListChildren { parent } = ticket.effect() else {
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
            let root = identity_row(&tx, dispatch.attempt_id(), *parent)?;
            let pending: Option<i64> = tx
                .query_row(
                    "SELECT parent_ordinal FROM pending_child_scans WHERE attempt_id=?1",
                    [key(dispatch.attempt_id())],
                    |r| r.get(0),
                )
                .optional()?;
            if binding != *expected
                || binding.attempt_id() != dispatch.attempt_id()
                || session.state != NativeSessionState::Active
                || binding.dispatch_floor() > dispatch.revision()
                || header.revision != dispatch.revision()
                || header.phase != Phase::OwnershipDiscovery
                || header.deadline != binding.deadline()
                || clock()? >= header.deadline
                || dispatch.ordinal() != 0
                || binding.slots().len() != 1
                || binding.slots()[0].process != *parent
                || !checkpoint.native_root_scan(*parent)
                || root.is_none()
                || root != pending
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let issue = NativeChildListIssue {
                key: dispatch,
                session_generation: session.generation,
                guardian_identity: binding.identity(),
                parent: *parent,
            };
            let (id, revision, ordinal) = parts(issue);
            tx.execute(
                "INSERT INTO native_child_list_issues VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    revision,
                    ordinal,
                    Word(session.generation.get()),
                    parent.pid().get(),
                    Word(parent.birth().seconds()),
                    parent.birth().nanos()
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

    /// Loads diagnostic evidence without recreating dispatch or ACK authority.
    /// # Errors
    /// Invalid rows poison this handle.
    pub fn load_native_child_list_status(
        &mut self,
        dispatch: DispatchKey,
    ) -> Result<Option<NativeChildListStatus>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let value = read_status(&tx, dispatch)?;
            tx.commit()?;
            Ok(value)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    /// Reads checkpoint, binding and issue from one snapshot.
    /// # Errors
    /// Partial evidence poisons this handle.
    pub fn read_native_child_list_evidence(
        &mut self,
        dispatch: DispatchKey,
    ) -> Result<Option<(LoadedAttempt, NativeGuardianBinding, NativeChildListStatus)>> {
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
                _ => return malformed("partial native child list evidence"),
            };
            tx.commit()?;
            Ok(value)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    /// Returns at most one successful child list awaiting confirmation.
    /// # Errors
    /// Ambiguous or malformed evidence poisons the handle.
    pub fn pending_native_child_list_ack(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(DispatchKey, NativeChildListStatus)>> {
        self.ensure_usable()?;
        let result = (|| {
            let tx = self.connection.transaction()?;
            check_authority(&tx)?;
            let mut statement = tx.prepare("SELECT dispatch_revision,ordinal FROM native_child_list_issues WHERE attempt_id=?1")?;
            let mut pending = None;
            let rows = statement.query_map([key(attempt)], |r| {
                Ok((r.get::<_, Word>(0)?.0, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (revision, ordinal) = row?;
                let dispatch = DispatchKey::from_committed_parts(
                    attempt,
                    binding_revision(revision)?,
                    usize::try_from(ordinal)
                        .map_err(|_| ControllerStoreError::Malformed("invalid native ordinal"))?,
                );
                match read_status(&tx, dispatch)? {
                    Some(status @ NativeChildListStatus::AppliedPendingAck { .. }) => {
                        if pending.replace((dispatch, status)).is_some() {
                            return malformed("multiple pending child list ACKs");
                        }
                    }
                    Some(_) => {}
                    None => return malformed("missing native child list issue"),
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
    /// Confirms only the exact successfully applied child vector in the current session.
    /// # Errors
    /// Mismatch or replay poisons this handle.
    pub fn confirm_native_child_list_ack(
        &mut self,
        proof: VerifiedNativeChildListAckConfirmation,
    ) -> Result<NativeChildListStatus> {
        self.ensure_usable()?;
        let result = (|| {
            let VerifiedNativeChildListAckConfirmation {
                issue,
                children,
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
                    != Some(NativeChildListStatus::AppliedPendingAck {
                        issue,
                        children: children.clone(),
                        applying_revision,
                    })
            {
                return Err(ControllerStoreError::RevisionConflict);
            }
            let (id, revision, ordinal) = parts(issue);
            tx.execute(
                "INSERT INTO native_child_list_ack_confirmations VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    revision,
                    ordinal,
                    Word(applying_revision.get()),
                    Word(confirming_session_generation.get())
                ],
            )?;
            let status = NativeChildListStatus::ChildrenAckConfirmed {
                issue,
                children,
                applying_revision,
                confirming_session_generation,
            };
            if read_status(&tx, issue.key)? != Some(status.clone()) {
                return malformed("child ACK did not round trip");
            }
            tx.commit()?;
            Ok(status)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub(crate) fn persist_native_child_list(
        &mut self,
        checkpoint: &DeploymentAttempt,
        issue: &NativeChildListIssue,
        completion: &NativeChildListCompletion,
    ) -> Result<PersistenceRevision> {
        self.persist_native_child_list_with_clock(
            checkpoint,
            issue,
            completion,
            &mut super::native_sampling::native_deadline_now,
        )
    }

    pub(crate) fn persist_native_child_list_with_clock(
        &mut self,
        checkpoint: &DeploymentAttempt,
        issue: &NativeChildListIssue,
        completion: &NativeChildListCompletion,
        clock: &mut (impl FnMut() -> Result<MonotonicTick> + ?Sized),
    ) -> Result<PersistenceRevision> {
        self.ensure_usable()?;
        let result = self.persist_checkpoint_with_sample(
            checkpoint,
            None,
            Some((issue, completion)),
            None,
            None,
            clock,
        );
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

fn unique_recovery_issue(
    tx: &rusqlite::Transaction<'_>,
    attempt: crate::AttemptId,
) -> Result<NativeChildListIssue> {
    let mut statement = tx.prepare(
        "SELECT dispatch_revision, ordinal FROM native_child_list_issues WHERE attempt_id=?1",
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
                .map_err(|_| ControllerStoreError::Malformed("invalid child issue ordinal"))?,
        );
        match read_status(tx, dispatch)? {
            Some(NativeChildListStatus::IssuedPendingApplication { issue }) => {
                if candidate.replace(issue).is_some() {
                    return Err(ControllerStoreError::RevisionConflict);
                }
            }
            Some(NativeChildListStatus::ChildrenAckConfirmed { .. }) => {}
            _ => return Err(ControllerStoreError::RevisionConflict),
        }
    }
    candidate.ok_or(ControllerStoreError::RevisionConflict)
}

pub(super) fn validate_recovery_candidate(
    tx: &rusqlite::Transaction<'_>,
    interrupted: &DeploymentAttempt,
    expected: &NativeGuardianBinding,
    issue: NativeChildListIssue,
    now: MonotonicTick,
) -> Result<()> {
    let session = expected.current_session();
    if now >= interrupted.deadline()
        || unique_recovery_issue(tx, interrupted.attempt_id())? != issue
        || interrupted.attempt_id() != issue.key().attempt_id()
        || interrupted.persistence_revision() != Some(session.interruption_revision)
        || !matches!(interrupted.phase(), Phase::Interrupted)
        || !interrupted.native_root_scan(issue.parent())
        || interrupted.deadline() != expected.deadline()
        || expected.attempt_id() != issue.key().attempt_id()
        || expected.identity() != issue.guardian_identity()
        || expected.slots().len() != 1
        || expected.slots()[0].process != issue.parent()
        || session.state != NativeSessionState::Reconciling
        || issue.session_generation() >= session.generation
        || issue.key().revision() >= session.interruption_revision
        || expected.dispatch_floor() != session.interruption_revision
        || read_attempt(tx, issue.key().attempt_id())?.as_ref() != Some(interrupted)
        || read_binding(tx, issue.key().attempt_id())?.as_ref() != Some(expected)
        || read_status(tx, issue.key())?
            != Some(NativeChildListStatus::IssuedPendingApplication { issue })
    {
        return Err(ControllerStoreError::RevisionConflict);
    }
    Ok(())
}

impl ControllerSqliteStore {
    /// Consumes an exact interrupted checkpoint, reconciling binding, and sole pending issue.
    /// # Errors
    /// Rejected or malformed durable evidence poisons this store handle.
    pub fn prepare_native_child_list_recovery(
        &mut self,
        loaded: LoadedAttempt,
    ) -> Result<NativeChildListRecoveryCandidate> {
        self.prepare_native_child_list_recovery_with_clock(
            loaded,
            super::native_sampling::native_deadline_now,
        )
    }

    #[cfg(test)]
    pub(crate) fn prepare_native_child_list_recovery_at(
        &mut self,
        loaded: LoadedAttempt,
        now: MonotonicTick,
    ) -> Result<NativeChildListRecoveryCandidate> {
        self.prepare_native_child_list_recovery_with_clock(loaded, || Ok(now))
    }

    fn prepare_native_child_list_recovery_with_clock(
        &mut self,
        loaded: LoadedAttempt,
        mut clock: impl FnMut() -> Result<MonotonicTick>,
    ) -> Result<NativeChildListRecoveryCandidate> {
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
            Ok(NativeChildListRecoveryCandidate {
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

    /// Atomically checkpoints the interrupted fold and ordered application evidence.
    /// Returns diagnostics only; no executable effect or ACK permit.
    /// # Errors
    /// Stale, expired, duplicate, or malformed evidence poisons this store handle.
    pub fn apply_recovered_native_child_list(
        &mut self,
        verified: VerifiedRecoveredNativeChildList,
    ) -> Result<NativeChildListStatus> {
        self.apply_recovered_native_child_list_with_clock(
            verified,
            super::native_sampling::native_deadline_now,
        )
    }

    #[cfg(test)]
    pub(crate) fn apply_recovered_native_child_list_at(
        &mut self,
        verified: VerifiedRecoveredNativeChildList,
        now: MonotonicTick,
    ) -> Result<NativeChildListStatus> {
        self.apply_recovered_native_child_list_with_clock(verified, || Ok(now))
    }

    fn apply_recovered_native_child_list_with_clock(
        &mut self,
        verified: VerifiedRecoveredNativeChildList,
        mut clock: impl FnMut() -> Result<MonotonicTick>,
    ) -> Result<NativeChildListStatus> {
        self.ensure_usable()?;
        let result = (|| {
            let VerifiedRecoveredNativeChildList {
                candidate,
                children,
            } = verified;
            let NativeChildListRecoveryCandidate {
                loaded,
                binding,
                issue,
            } = candidate;
            let checkpoint = loaded
                .checkpoint
                .clone()
                .fold_interrupted_child_list(issue.parent(), children.clone())
                .ok_or(ControllerStoreError::RevisionConflict)?;
            let revision = self.persist_checkpoint_with_recovery(
                &checkpoint,
                None,
                Some((
                    &issue,
                    &NativeChildListCompletion::Children(children.clone()),
                )),
                None,
                None,
                Some((&loaded.checkpoint, &binding, &issue)),
                &mut clock,
            )?;
            Ok(NativeChildListStatus::AppliedPendingAck {
                issue,
                children,
                applying_revision: revision,
            })
        })();
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
    issue: &NativeChildListIssue,
    completion: &NativeChildListCompletion,
) -> Result<()> {
    let old = required(durable, "missing native child list checkpoint")?;
    let binding = required(
        read_binding(tx, issue.key.attempt_id())?,
        "missing child list binding",
    )?;
    if old.attempt_id() != issue.key.attempt_id()
        || old.persistence_revision() != Some(issue.key.revision())
        || binding.identity() != issue.guardian_identity
        || binding.current_session().generation != issue.session_generation
        || binding.current_session().state != NativeSessionState::Active
        || binding.dispatch_floor() > issue.key.revision()
        || binding.slots().len() != 1
        || binding.slots()[0].process != issue.parent
        || !old.native_root_scan(issue.parent)
        || read_status(tx, issue.key)?
            != Some(NativeChildListStatus::IssuedPendingApplication { issue: *issue })
        || checkpoint
            != &match completion {
                NativeChildListCompletion::Children(children) => {
                    old.clone()
                        .record_child_list(issue.parent, children.clone())
                        .map_err(|_| ControllerStoreError::RevisionConflict)?
                        .checkpoint
                }
                NativeChildListCompletion::Failure(_) => old.clone(),
            }
    {
        return Err(ControllerStoreError::RevisionConflict);
    }
    Ok(())
}

pub(super) fn insert_application(
    tx: &rusqlite::Transaction<'_>,
    issue: &NativeChildListIssue,
    completion: &NativeChildListCompletion,
    applying_revision: PersistenceRevision,
) -> Result<()> {
    let (id, revision, ordinal) = parts(*issue);
    let outcome = match completion {
        NativeChildListCompletion::Children(_) => "success",
        NativeChildListCompletion::Failure(OsQueryError::PermissionDenied) => "permission_denied",
        NativeChildListCompletion::Failure(OsQueryError::TemporarilyUnavailable) => {
            "temporarily_unavailable"
        }
        NativeChildListCompletion::Failure(OsQueryError::Malformed) => "malformed",
    };
    tx.execute(
        "INSERT INTO native_child_list_applications VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            id,
            revision,
            ordinal,
            Word(applying_revision.get()),
            outcome,
            match completion {
                NativeChildListCompletion::Children(children) => Some(
                    i64::try_from(children.len())
                        .map_err(|_| ControllerStoreError::Malformed("child count overflow"))?
                ),
                NativeChildListCompletion::Failure(_) => None,
            }
        ],
    )?;
    if let NativeChildListCompletion::Children(children) = completion {
        for (index, pid) in children.iter().enumerate() {
            tx.execute(
                "INSERT INTO native_child_list_result_children VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    revision,
                    ordinal,
                    i64::try_from(index)
                        .map_err(|_| ControllerStoreError::Malformed("child index overflow"))?,
                    pid.get()
                ],
            )?;
        }
    }
    let expected = match completion {
        NativeChildListCompletion::Children(children) => NativeChildListStatus::AppliedPendingAck {
            issue: *issue,
            children: children.clone(),
            applying_revision,
        },
        NativeChildListCompletion::Failure(failure) => NativeChildListStatus::ParkedRetained {
            issue: *issue,
            failure: *failure,
            applying_revision,
        },
    };
    if read_status(tx, issue.key)? != Some(expected) {
        return malformed("native child list application did not round trip");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) fn read_status(
    connection: &rusqlite::Connection,
    dispatch: DispatchKey,
) -> Result<Option<NativeChildListStatus>> {
    let id = key(dispatch.attempt_id());
    let ordinal =
        i64::try_from(dispatch.ordinal()).map_err(|_| ControllerStoreError::RevisionConflict)?;
    let row=connection.query_row("SELECT i.session_generation,i.parent_pid,i.parent_birth_seconds,i.parent_birth_nanos,a.applying_revision,a.outcome,a.child_count,c.confirming_session_generation FROM native_child_list_issues i LEFT JOIN native_child_list_applications a ON a.attempt_id=i.attempt_id AND a.dispatch_revision=i.dispatch_revision AND a.ordinal=i.ordinal LEFT JOIN native_child_list_ack_confirmations c ON c.attempt_id=i.attempt_id AND c.dispatch_revision=i.dispatch_revision AND c.ordinal=i.ordinal WHERE i.attempt_id=?1 AND i.dispatch_revision=?2 AND i.ordinal=?3",params![id,Word(dispatch.revision().get()),ordinal],|r|Ok((r.get::<_,Word>(0)?.0,r.get::<_,i64>(1)?,r.get::<_,Word>(2)?.0,r.get::<_,i64>(3)?,r.get::<_,Option<Word>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,Option<Word>>(7)?))).optional()?;
    let Some((
        generation,
        pid,
        birth_seconds,
        birth_nanos,
        applying,
        outcome,
        child_count,
        confirming,
    )) = row
    else {
        return Ok(None);
    };
    if dispatch.ordinal() != 0 {
        return malformed("invalid child issue ordinal");
    }
    let parent = ProcessIdentity::new(
        ProcessId::new(
            NonZeroU32::new(
                u32::try_from(pid)
                    .map_err(|_| ControllerStoreError::Malformed("invalid parent pid"))?,
            )
            .ok_or(ControllerStoreError::Malformed("zero parent pid"))?,
        ),
        BirthToken::new(
            birth_seconds,
            u32::try_from(birth_nanos)
                .map_err(|_| ControllerStoreError::Malformed("invalid birth nanos"))?,
        )
        .map_err(|_| ControllerStoreError::Malformed("invalid birth nanos"))?,
    );
    let binding = required(
        read_binding(connection, dispatch.attempt_id())?,
        "missing child issue binding",
    )?;
    let header = required(read_header(connection, &id)?, "missing child issue attempt")?;
    let session_generation = required(NonZeroU64::new(generation), "zero child issue generation")?;
    let issue = NativeChildListIssue {
        key: dispatch,
        session_generation,
        guardian_identity: binding.identity(),
        parent,
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
        || binding.slots()[0].process != parent
    {
        return malformed("invalid native child issue identity");
    }
    let mut stmt=connection.prepare("SELECT child_ordinal,pid FROM native_child_list_result_children WHERE attempt_id=?1 AND dispatch_revision=?2 AND issue_ordinal=?3 ORDER BY child_ordinal")?;
    let rows = stmt.query_map(params![id, Word(dispatch.revision().get()), ordinal], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut children = Vec::new();
    for row in rows {
        let (child_ordinal, pid) = row?;
        if child_ordinal
            != i64::try_from(children.len())
                .map_err(|_| ControllerStoreError::Malformed("child count overflow"))?
        {
            return malformed("noncontiguous child list");
        }
        let pid = NonZeroU32::new(
            u32::try_from(pid).map_err(|_| ControllerStoreError::Malformed("invalid child pid"))?,
        )
        .ok_or(ControllerStoreError::Malformed("zero child pid"))?;
        children.push(ProcessId::new(pid));
    }
    drop(stmt);
    let Some(applying) = applying else {
        if outcome.is_some()
            || child_count.is_some()
            || confirming.is_some()
            || !children.is_empty()
        {
            return malformed("partial child application");
        }
        return Ok(Some(NativeChildListStatus::IssuedPendingApplication {
            issue,
        }));
    };
    let applying_revision = binding_revision(applying.0)?;
    if applying_revision <= dispatch.revision() || applying_revision > header.revision {
        return malformed("invalid child applying revision");
    }
    let status = match outcome.as_deref() {
        Some("success") => {
            if child_count
                != Some(
                    i64::try_from(children.len())
                        .map_err(|_| ControllerStoreError::Malformed("child count overflow"))?,
                )
            {
                return malformed("partial child list");
            }
            if let Some(generation) = confirming {
                let confirming_session_generation =
                    required(NonZeroU64::new(generation.0), "zero confirming generation")?;
                if confirming_session_generation < session_generation
                    || !binding.sessions().iter().any(|s| {
                        s.generation == confirming_session_generation
                            && s.interruption_revision <= header.revision
                    })
                {
                    return malformed("invalid confirming child session");
                }
                NativeChildListStatus::ChildrenAckConfirmed {
                    issue,
                    children,
                    applying_revision,
                    confirming_session_generation,
                }
            } else {
                NativeChildListStatus::AppliedPendingAck {
                    issue,
                    children,
                    applying_revision,
                }
            }
        }
        Some(failure @ ("permission_denied" | "temporarily_unavailable" | "malformed")) => {
            if !children.is_empty() || child_count.is_some() || confirming.is_some() {
                return malformed("failed child list has children or ACK");
            }
            let failure = match failure {
                "permission_denied" => OsQueryError::PermissionDenied,
                "temporarily_unavailable" => OsQueryError::TemporarilyUnavailable,
                _ => OsQueryError::Malformed,
            };
            NativeChildListStatus::ParkedRetained {
                issue,
                failure,
                applying_revision,
            }
        }
        _ => return malformed("invalid child list outcome"),
    };
    Ok(Some(status))
}

#[cfg(test)]
#[path = "native_child_list/tests.rs"]
mod tests;
