use std::num::NonZeroU64;

use phoenix_core::domain::{
    conversation_goal::GoalTimestampUs, product_conversation::ProductConversationId,
};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, Row};
use uuid::Uuid;

use crate::Database;

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

async fn receipt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &ControlRequest,
) -> Result<Option<ControlReceipt>, ControlError> {
    sqlx::query("SELECT command,expected_control_version,outcome,rejection,result_control_version,result_stopped,recorded_at_us FROM conversation_goal_control_requests WHERE owner_id=? AND principal_id=? AND request_id=?")
        .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
        .bind(input.request_id.as_bytes().as_slice()).fetch_optional(&mut **tx).await?
        .as_ref().map(|row| decode(row, input)).transpose()
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

impl Database {
    /// Persist a control command after the application has authenticated an owner request.
    /// The reserved control schema is not installed by this operation.
    ///
    /// # Errors
    /// Rejects closed/nonordinary owners, conflicting request identities, invalid versions and storage failures.
    pub async fn apply_goal_control(
        &self,
        input: &ControlRequest,
        at: GoalTimestampUs,
    ) -> Result<ControlReceipt, ControlError> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let authorized_owner: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_conversations p WHERE p.id=? AND p.kind='ordinary' AND p.ordinary_lifecycle='open' AND NOT EXISTS(SELECT 1 FROM close_obligations o WHERE o.product_conversation_id=p.id AND o.phase!='completed'))")
            .bind(input.owner.as_str()).fetch_one(&mut *tx).await?;
        if !authorized_owner {
            return Err(ControlError::OwnerUnavailable);
        }
        if let Some(receipt) = receipt_tx(&mut tx, input).await? {
            tx.commit().await?;
            return Ok(receipt);
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
        tx.commit().await?;
        Ok(receipt)
    }
}
