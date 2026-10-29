use std::num::NonZeroU64;

use phoenix_core::domain::{
    conversation_goal::GoalTimestampUs, product_conversation::ProductConversationId,
};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, Connection, Row};
use uuid::Uuid;

use crate::Database;

pub mod dispatch;
#[cfg(test)]
mod tests;
use dispatch::GoalDispatchPermit;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GoalControlInstallation {
    #[default]
    Disabled,
    Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlCommand {
    Stop,
    Release,
}

impl ControlCommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ControlSnapshot {
    version: NonZeroU64,
    stopped: bool,
}

impl ControlSnapshot {
    #[must_use]
    pub fn version(self) -> u64 {
        self.version.get()
    }

    #[must_use]
    pub fn stopped(self) -> bool {
        self.stopped
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ControlOutcome {
    Accepted { snapshot: ControlSnapshot },
    RejectedVersion { observed: Option<ControlSnapshot> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlReceipt {
    pub owner: ProductConversationId,
    pub principal_id: Uuid,
    pub request_id: Uuid,
    pub command: ControlCommand,
    pub expected_control_version: u64,
    pub result: ControlOutcome,
    pub recorded_at: GoalTimestampUs,
}

pub struct ControlRequest {
    pub owner: ProductConversationId,
    pub principal_id: Uuid,
    pub request_id: Uuid,
    pub command: ControlCommand,
    pub expected_control_version: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
    #[error(transparent)]
    Admission(#[from] crate::DbError),
    #[error("ordinary owner is unavailable for control")]
    OwnerUnavailable,
    #[error("control request identity already has different input")]
    RequestConflict,
    #[error("control version is out of range")]
    VersionRange,
    #[error("control admission requires an initialized open owner control")]
    AdmissionStopped,
    #[error("invalid persisted control data: {0}")]
    Integrity(&'static str),
}

fn snapshot(version: i64, stopped: Option<i64>) -> Result<Option<ControlSnapshot>, ControlError> {
    if version == 0 && stopped.is_none() {
        return Ok(None);
    }
    let version = u64::try_from(version)
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or(ControlError::Integrity("control version"))?;
    let stopped = match stopped {
        Some(0) => false,
        Some(1) => true,
        Some(_) | None => return Err(ControlError::Integrity("control stop latch")),
    };
    Ok(Some(ControlSnapshot { version, stopped }))
}

fn decode(row: &SqliteRow, input: &ControlRequest) -> Result<ControlReceipt, ControlError> {
    let command = match row.try_get::<&str, _>("command")? {
        "stop" => ControlCommand::Stop,
        "release" => ControlCommand::Release,
        _ => return Err(ControlError::Integrity("control command")),
    };
    let expected_control_version =
        u64::try_from(row.try_get::<i64, _>("expected_control_version")?)
            .map_err(|_| ControlError::Integrity("expected control version"))?;
    if command != input.command || expected_control_version != input.expected_control_version {
        return Err(ControlError::RequestConflict);
    }
    let observed = snapshot(
        row.try_get("result_control_version")?,
        row.try_get("result_stopped")?,
    )?;
    let result = match row.try_get::<&str, _>("outcome")? {
        "accepted" => ControlOutcome::Accepted {
            snapshot: observed.ok_or(ControlError::Integrity("accepted control snapshot"))?,
        },
        "rejected"
            if row.try_get::<Option<&str>, _>("rejection")? == Some("control_version_conflict") =>
        {
            ControlOutcome::RejectedVersion { observed }
        }
        _ => return Err(ControlError::Integrity("control receipt outcome")),
    };
    Ok(ControlReceipt {
        owner: input.owner.clone(),
        principal_id: input.principal_id,
        request_id: input.request_id,
        command,
        expected_control_version,
        result,
        recorded_at: GoalTimestampUs::parse(row.try_get("recorded_at_us")?)
            .map_err(|_| ControlError::Integrity("control receipt timestamp"))?,
    })
}

#[cfg(test)]
enum ControlCommitCut {
    None,
    LostAcknowledgement,
    LostClassification,
    AfterCommit {
        reached: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    },
}

pub(super) struct ControlCommitAuthority<F: FnOnce()> {
    unresolved: bool,
    on_authority_lost: Option<F>,
}

impl<F: FnOnce()> ControlCommitAuthority<F> {
    pub(super) fn new(on_authority_lost: F) -> Self {
        Self {
            unresolved: true,
            on_authority_lost: Some(on_authority_lost),
        }
    }

    pub(super) fn classified(&mut self) {
        self.unresolved = false;
    }
}

impl<F: FnOnce()> Drop for ControlCommitAuthority<F> {
    fn drop(&mut self) {
        if self.unresolved {
            if let Some(close_authority) = self.on_authority_lost.take() {
                close_authority();
            }
        }
    }
}

async fn receipt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &ControlRequest,
) -> Result<Option<ControlReceipt>, ControlError> {
    sqlx::query("SELECT command,expected_control_version,outcome,rejection,result_control_version,result_stopped,recorded_at_us FROM conversation_goal_control_requests WHERE owner_id=? AND principal_id=? AND request_id=?")
        .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
        .bind(input.request_id.as_bytes().as_slice()).fetch_optional(&mut **tx).await?
        .as_ref().map(|row| decode(row, input)).transpose()
}

pub(crate) async fn capture_runtime_control_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    execution: phoenix_core::domain::conversation_goal::GoalExecution,
    installation: GoalControlInstallation,
) -> Result<(), ControlError> {
    if installation == GoalControlInstallation::Disabled {
        return Ok(());
    }
    let owner: Option<String> = sqlx::query_scalar(
        "SELECT p.id FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id
         JOIN product_conversations p ON p.id=c.product_conversation_id
         WHERE t.turn_id=? AND t.workflow_id=? AND p.kind='ordinary'",
    )
    .bind(execution.turn_id())
    .bind(execution.workflow_id())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(owner) = owner {
        let owner = ProductConversationId::parse(owner)
            .map_err(|_| ControlError::Integrity("admitted owner identity"))?;
        bind_control_at_admission_tx(tx, &owner, execution).await?;
    }
    Ok(())
}

pub(crate) fn db_error(error: ControlError) -> crate::DbError {
    match error {
        ControlError::Storage(error) => crate::DbError::Sqlx(error),
        ControlError::Admission(error) => error,
        other @ (ControlError::OwnerUnavailable
        | ControlError::RequestConflict
        | ControlError::VersionRange
        | ControlError::AdmissionStopped
        | ControlError::Integrity(_)) => crate::DbError::Serialization(other.to_string()),
    }
}

/// Capture control in the same transaction that captures the turn's Goal/NoGoal observation.
///
/// # Errors
/// Rejects stopped/uninitialized owners, invalid execution ownership and persistence failures.
pub async fn bind_control_at_admission_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner: &ProductConversationId,
    execution: phoenix_core::domain::conversation_goal::GoalExecution,
) -> Result<(), ControlError> {
    let result = sqlx::query("INSERT INTO conversation_goal_control_bindings (turn_id,workflow_id,owner_id,control_version,principal_id,request_id) SELECT ?,?,owner_id,control_version,principal_id,request_id FROM conversation_goal_controls WHERE owner_id=? AND stopped=0")
        .bind(execution.turn_id()).bind(execution.workflow_id()).bind(owner.as_str())
        .execute(&mut **tx).await?;
    if result.rows_affected() != 1 {
        return Err(ControlError::AdmissionStopped);
    }
    Ok(())
}

/// Recheck the exact admitted control inside the transaction that owns the next effect.
///
/// # Errors
/// Rejects missing, stopped, stale or cross-execution control and unavailable owners.
/// Storage failures are not relabeled as semantic refusal.
pub async fn require_current_control_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner: &ProductConversationId,
    execution: phoenix_core::domain::conversation_goal::GoalExecution,
) -> Result<(), ControlError> {
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM conversation_goal_control_bindings b
         JOIN conversation_goal_controls ctl ON ctl.owner_id=b.owner_id
             AND ctl.control_version=b.control_version AND ctl.stopped=b.admitted_stopped
             AND ctl.principal_id=b.principal_id AND ctl.request_id=b.request_id
         JOIN product_conversations p ON p.id=b.owner_id
         JOIN durable_turns t ON t.turn_id=b.turn_id AND t.workflow_id=b.workflow_id
         JOIN conversations c ON c.id=t.conversation_id AND c.product_conversation_id=b.owner_id
         WHERE b.owner_id=? AND b.turn_id=? AND b.workflow_id=? AND b.admitted_stopped=0
             AND p.kind='ordinary' AND p.ordinary_lifecycle='open'
             AND t.disposition='Runtime' AND t.terminal_kind IS NULL
             AND t.owns_conversation=1
             AND NOT EXISTS(SELECT 1 FROM close_obligations o
                 WHERE o.product_conversation_id=p.id AND o.phase<>'completed'))",
    )
    .bind(owner.as_str())
    .bind(execution.turn_id())
    .bind(execution.workflow_id())
    .fetch_one(&mut **tx)
    .await?;
    if !current {
        return Err(ControlError::AdmissionStopped);
    }
    Ok(())
}

impl Database {
    /// Read existing control facts with canonical owner resolution on the same read snapshot.
    ///
    /// # Errors
    /// Rejects unavailable/nonordinary owners and malformed persisted controls.
    pub async fn ordinary_goal_control(
        &self,
        owner: &ProductConversationId,
    ) -> Result<Option<ControlSnapshot>, ControlError> {
        let mut tx = self.pool().begin().await?;
        Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        let row: Option<(i64, i64)> = sqlx::query_as(
            "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
        )
        .bind(owner.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let current = row.map_or(Ok(None), |(version, stopped)| {
            snapshot(version, Some(stopped))
        })?;
        tx.commit().await?;
        Ok(current)
    }

    #[must_use]
    pub fn goal_control_installation(&self) -> GoalControlInstallation {
        self.goal_control_installation
    }

    /// Enable controls in an isolated test database with migrated control tables.
    ///
    /// # Errors
    /// Returns a missing-schema error without enabling the controls.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn install_goal_controls_for_tests(mut self) -> Result<Self, ControlError> {
        sqlx::query("SELECT owner_id FROM conversation_goal_controls LIMIT 0")
            .fetch_all(self.pool())
            .await?;
        self.goal_control_installation = GoalControlInstallation::Installed;
        Ok(self)
    }

    /// Admit an exact active execution to the shared dispatch-versus-control owner gate.
    ///
    /// # Errors
    /// Refuses missing/stale authority and stopped control; storage errors remain distinct.
    pub async fn admit_goal_control_dispatch(
        &self,
        transcript: &str,
        active_turn: Option<(i64, u64)>,
    ) -> Result<GoalDispatchPermit, ControlError> {
        if self.goal_control_installation == GoalControlInstallation::Disabled {
            return Ok(GoalDispatchPermit::disabled());
        }
        let owner: Option<(String, String)> = sqlx::query_as(
            "SELECT p.id,p.kind FROM conversations c JOIN product_conversations p
             ON p.id=c.product_conversation_id WHERE c.id=?",
        )
        .bind(transcript)
        .fetch_optional(self.pool())
        .await?;
        let (owner, kind) = owner.ok_or(ControlError::OwnerUnavailable)?;
        if kind != "ordinary" {
            return Ok(GoalDispatchPermit::disabled());
        }
        let gate = self.goal_control_runtime.owner(&owner)?;
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let epoch = gate.lock().await;
        let (turn, generation) = active_turn.ok_or(ControlError::AdmissionStopped)?;
        let generation = i64::try_from(generation).map_err(|_| ControlError::AdmissionStopped)?;
        let workflow: Option<i64> = sqlx::query_scalar(
            "SELECT t.workflow_id FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id
             WHERE t.turn_id=? AND t.conversation_id=? AND t.generation=?
             AND t.disposition='Runtime' AND t.owns_conversation=1 AND t.terminal_kind IS NULL
             AND t.materialized=1 AND c.parent_conversation_id IS NULL
             AND c.continued_in_conv_id IS NULL",
        )
        .bind(turn)
        .bind(transcript)
        .bind(generation)
        .fetch_optional(&mut *tx)
        .await?;
        let execution = phoenix_core::domain::conversation_goal::GoalExecution::parse(
            workflow.ok_or(ControlError::AdmissionStopped)?,
            turn,
        )
        .map_err(|_| ControlError::Integrity("active execution identity"))?;
        let owner = ProductConversationId::parse(owner)
            .map_err(|_| ControlError::Integrity("active owner identity"))?;
        require_current_control_tx(&mut tx, &owner, execution).await?;
        tx.commit().await?;
        Ok(GoalDispatchPermit::admitted(gate.clone(), &epoch))
    }

    /// Persist a control command after the application has authenticated an owner request.
    /// The reserved control schema is not installed by this operation.
    ///
    /// # Errors
    /// Established failures reject closed/nonordinary owners, conflicting identities,
    /// invalid versions, or a transaction known not to have committed.
    ///
    /// `on_authority_lost` must synchronously close the owner's existing authority
    /// fence, honoring its serving/coordinated-shutdown disposition. It must not
    /// defer closure to an async task or panic. It is invoked at most once, including
    /// when this future is cancelled during COMMIT; it must not publish cancellation.
    pub async fn apply_goal_control<F: FnOnce() + Send>(
        &self,
        input: &ControlRequest,
        at: GoalTimestampUs,
        on_authority_lost: F,
    ) -> crate::workflow::LocalAuthorityResult<Result<ControlReceipt, ControlError>> {
        self.apply_goal_control_at_cut(
            input,
            at,
            on_authority_lost,
            #[cfg(test)]
            ControlCommitCut::None,
        )
        .await
    }

    async fn apply_goal_control_at_cut<F: FnOnce() + Send>(
        &self,
        input: &ControlRequest,
        at: GoalTimestampUs,
        on_authority_lost: F,
        #[cfg(test)] cut: ControlCommitCut,
    ) -> crate::workflow::LocalAuthorityResult<Result<ControlReceipt, ControlError>> {
        use crate::workflow::LocalAuthorityResult::{
            DurableFactEstablished, DurableFactUnclassified,
        };
        match self
            .apply_goal_control_command(
                input,
                at,
                on_authority_lost,
                #[cfg(test)]
                cut,
            )
            .await
        {
            Ok(DurableFactEstablished(receipt)) => DurableFactEstablished(Ok(receipt)),
            Ok(DurableFactUnclassified) => DurableFactUnclassified,
            Err(error) => DurableFactEstablished(Err(error)),
        }
    }

    async fn apply_goal_control_command<F: FnOnce() + Send>(
        &self,
        input: &ControlRequest,
        at: GoalTimestampUs,
        on_authority_lost: F,
        #[cfg(test)] cut: ControlCommitCut,
    ) -> Result<crate::workflow::LocalAuthorityResult<ControlReceipt>, ControlError> {
        let gate = self.goal_control_runtime.owner(input.owner.as_str())?;
        let mut connection = self.pool().acquire().await?;
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut epoch = gate.lock().await;
        let authorized_owner: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_conversations p WHERE p.id=? AND p.kind='ordinary' AND p.ordinary_lifecycle='open' AND NOT EXISTS(SELECT 1 FROM close_obligations o WHERE o.product_conversation_id=p.id AND o.phase!='completed'))")
            .bind(input.owner.as_str()).fetch_one(&mut *tx).await?;
        if !authorized_owner {
            return Err(ControlError::OwnerUnavailable);
        }
        if let Some(receipt) = receipt_tx(&mut tx, input).await? {
            return Ok(crate::workflow::LocalAuthorityResult::DurableFactEstablished(receipt));
        }
        let expected = i64::try_from(input.expected_control_version)
            .map_err(|_| ControlError::VersionRange)?;
        let current: Option<(i64, i64)> = sqlx::query_as(
            "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
        )
        .bind(input.owner.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let version = current.map_or(0, |(version, _)| version);
        let accepted = version == expected;
        let (result_version, stopped) = if accepted {
            (
                version.checked_add(1).ok_or(ControlError::VersionRange)?,
                Some(i64::from(input.command == ControlCommand::Stop)),
            )
        } else {
            (version, current.map(|(_, stopped)| stopped))
        };
        sqlx::query("INSERT INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,rejection,result_control_version,result_stopped,recorded_at_us) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice()).bind(input.request_id.as_bytes().as_slice())
            .bind(input.command.as_str()).bind(expected).bind(if accepted { "accepted" } else { "rejected" })
            .bind((!accepted).then_some("control_version_conflict")).bind(result_version).bind(stopped).bind(at.get())
            .execute(&mut *tx).await?;
        let receipt = receipt_tx(&mut tx, input)
            .await?
            .ok_or(ControlError::Integrity("missing committed control receipt"))?;
        let mut authority = ControlCommitAuthority {
            unresolved: true,
            on_authority_lost: Some(on_authority_lost),
        };
        let commit_result = tx.commit().await;
        let result = async {
            #[cfg(test)]
            let commit_result = match cut {
                ControlCommitCut::None => commit_result,
                ControlCommitCut::LostAcknowledgement => {
                    commit_result?;
                    Err(sqlx::Error::Protocol(
                        "injected lost commit acknowledgement".into(),
                    ))
                }
                ControlCommitCut::LostClassification => {
                    commit_result?;
                    return Err(ControlError::Integrity(
                        "injected unavailable classification",
                    ));
                }
                ControlCommitCut::AfterCommit { reached, release } => {
                    commit_result?;
                    let _ = reached.send(());
                    let _ = release.await;
                    Ok(())
                }
            };
            if let Err(commit_error) = commit_result {
                // SQLx queues the failed transaction's rollback on this same worker
                // before BEGIN; absence is authoritative only after BEGIN succeeds.
                let mut classification = connection.begin_with("BEGIN IMMEDIATE").await?;
                match receipt_tx(&mut classification, input).await? {
                    Some(actual) if actual == receipt => {}
                    None => {
                        authority.classified();
                        return Err(commit_error.into());
                    }
                    Some(_) => {
                        return Err(ControlError::Integrity("control commit receipt differs"))
                    }
                }
            }
            authority.classified();
            if accepted {
                epoch.revoke();
            }
            Ok(receipt)
        }
        .await;
        if authority.unresolved {
            drop(authority);
            Ok(crate::workflow::LocalAuthorityResult::DurableFactUnclassified)
        } else {
            result.map(crate::workflow::LocalAuthorityResult::DurableFactEstablished)
        }
    }
}
