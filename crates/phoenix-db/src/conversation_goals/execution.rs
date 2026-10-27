use phoenix_core::domain::conversation_goal::{
    GoalExecution, GoalFence, GoalId, GoalRequestCause, GoalRequestId, GoalRequestIntent,
    GoalRequestLimit, GoalText, GoalVersion,
};
use sqlx::{sqlite::SqliteRow, Row};

use super::{GoalDbError, GoalTx};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum GoalExecutionInstallation {
    #[default]
    Disabled,
    Installed {
        ceiling: GoalRequestLimit,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GoalWritePolicies {
    pub control: super::control::GoalControlInstallation,
    pub execution: GoalExecutionInstallation,
}

impl crate::Database {
    pub(crate) fn goal_write_policies(&self) -> GoalWritePolicies {
        GoalWritePolicies {
            control: self.goal_control_installation,
            execution: self.goal_execution_installation,
        }
    }

    /// Install an explicit finite request policy in an isolated migrated test database.
    ///
    /// # Errors
    /// Rejects missing controls or execution tables without installing the policy.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn install_goal_execution_for_tests(
        mut self,
        ceiling: GoalRequestLimit,
    ) -> Result<Self> {
        if self.goal_control_installation != super::control::GoalControlInstallation::Installed {
            return Err(GoalRequestError::Disabled);
        }
        sqlx::query("SELECT request_id FROM conversation_goal_requests LIMIT 0")
            .fetch_all(self.pool())
            .await?;
        sqlx::query("SELECT start_id FROM conversation_goal_start_materializations LIMIT 0")
            .fetch_all(self.pool())
            .await?;
        self.goal_execution_installation = GoalExecutionInstallation::Installed { ceiling };
        Ok(self)
    }

    #[must_use]
    pub fn goal_execution_installation(&self) -> GoalExecutionInstallation {
        self.goal_execution_installation
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GoalRequestError {
    #[error("goal execution policy is not installed")]
    Disabled,
    #[error("goal has no captured finite request limit")]
    Ineligible,
    #[error("goal request limit is exhausted")]
    Exhausted,
    #[error("logical request identity or causal slot conflicts")]
    Conflict,
    #[error("goal request no longer has its exact live Underway binding")]
    Revoked,
    #[error("accepted queued input must be drained before this goal request")]
    QueuedInput,
    #[error("request checkpoint requires its live admitted execution")]
    Detached,
    #[error(transparent)]
    Goal(#[from] GoalDbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

type Result<T> = std::result::Result<T, GoalRequestError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRequestAdmission {
    pub intent: GoalRequestIntent,
    pub ordinal: i64,
    pub live_turn_id: Option<i64>,
    pub checkpoint: Option<GoalText>,
}

pub(crate) async fn capture_created_limit_tx(
    tx: &mut GoalTx<'_>,
    installation: GoalExecutionInstallation,
    goal: &GoalId,
) -> super::Result<()> {
    let GoalExecutionInstallation::Installed { ceiling } = installation else {
        return Ok(());
    };
    sqlx::query("INSERT INTO conversation_goal_limits (goal_id, request_limit) VALUES (?,?)")
        .bind(goal.as_str())
        .bind(ceiling.get())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn cause_parts(cause: &GoalRequestCause) -> (&'static str, Option<&str>, Option<&str>) {
    match cause {
        GoalRequestCause::Initial => ("initial", None, None),
        GoalRequestCause::AfterCheckpoint { predecessor } => {
            ("after_checkpoint", Some(predecessor.as_str()), None)
        }
        GoalRequestCause::Summary { operation } => ("summary", None, Some(operation.as_str())),
    }
}

fn decode(row: &SqliteRow) -> super::Result<GoalRequestAdmission> {
    let cause = match (
        row.try_get::<String, _>("cause")?.as_str(),
        row.try_get::<Option<String>, _>("predecessor_request_id")?,
        row.try_get::<Option<String>, _>("summary_operation")?,
    ) {
        ("initial", None, None) => GoalRequestCause::Initial,
        ("after_checkpoint", Some(predecessor), None) => GoalRequestCause::AfterCheckpoint {
            predecessor: GoalRequestId::parse(predecessor)?,
        },
        ("summary", None, Some(operation)) => GoalRequestCause::Summary {
            operation: GoalText::parse(operation)?,
        },
        _ => return Err(GoalDbError::Integrity("logical request cause")),
    };
    let intent = GoalRequestIntent::new(
        GoalRequestId::parse(row.try_get::<String, _>("request_id")?)?,
        GoalFence::new(
            super::product(row.try_get("owner_id")?)?,
            GoalId::parse(row.try_get::<String, _>("goal_id")?)?,
            GoalVersion::parse(row.try_get("observed_version")?)?,
        )?,
        GoalExecution::parse(
            row.try_get("historical_workflow_id")?,
            row.try_get("historical_turn_id")?,
        )?,
        row.try_get("generation")?,
        cause,
    )?;
    let ordinal: i64 = row.try_get("ordinal")?;
    let live_turn_id: Option<i64> = row.try_get("live_turn_id")?;
    if ordinal <= 0 || live_turn_id.is_some_and(|turn| turn != intent.execution.turn_id()) {
        return Err(GoalDbError::Integrity(
            "request ordinal or live association",
        ));
    }
    let checkpoint = row
        .try_get::<Option<String>, _>("checkpoint_key")?
        .map(GoalText::parse)
        .transpose()?;
    Ok(GoalRequestAdmission {
        intent,
        ordinal,
        live_turn_id,
        checkpoint,
    })
}

pub(crate) async fn latest_request_tx(
    tx: &mut GoalTx<'_>,
    execution: GoalExecution,
) -> Result<Option<GoalRequestAdmission>> {
    sqlx::query(
        "SELECT r.*,c.checkpoint_key FROM conversation_goal_requests r
         LEFT JOIN conversation_goal_request_checkpoints c USING(request_id)
         WHERE r.historical_turn_id=? AND r.historical_workflow_id=?
         ORDER BY r.ordinal DESC LIMIT 1",
    )
    .bind(execution.turn_id())
    .bind(execution.workflow_id())
    .fetch_optional(&mut **tx)
    .await?
    .as_ref()
    .map(decode)
    .transpose()
    .map_err(GoalRequestError::from)
}

/// Reads retained identity/causal-slot evidence without requiring a live turn.
///
/// # Errors
/// Returns a conflict for any changed identity fact or occupied causal slot.
/// Returns storage/decode errors without interpreting them as absence.
pub async fn retained_request_tx(
    tx: &mut GoalTx<'_>,
    installation: GoalExecutionInstallation,
    intent: &GoalRequestIntent,
) -> Result<Option<GoalRequestAdmission>> {
    if installation == GoalExecutionInstallation::Disabled {
        return Err(GoalRequestError::Disabled);
    }
    let (cause, predecessor, summary) = cause_parts(intent.cause());
    let rows = sqlx::query(
        "SELECT r.*, c.checkpoint_key FROM conversation_goal_requests r
         LEFT JOIN conversation_goal_request_checkpoints c USING (request_id)
         WHERE r.request_id=? OR (r.goal_id=? AND r.cause=? AND (
             (?='initial' AND r.historical_turn_id=?)
             OR (?='after_checkpoint' AND r.predecessor_request_id=?)
             OR (?='summary' AND r.summary_operation=?)))",
    )
    .bind(intent.id().as_str())
    .bind(intent.goal().goal_id().as_str())
    .bind(cause)
    .bind(cause)
    .bind(intent.execution.turn_id())
    .bind(cause)
    .bind(predecessor)
    .bind(cause)
    .bind(summary)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > 1 {
        return Err(GoalRequestError::Conflict);
    }
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let retained = decode(row)?;
    if &retained.intent != intent {
        return Err(GoalRequestError::Conflict);
    }
    Ok(Some(retained))
}

fn next_ordinal(used: i64, limit: GoalRequestLimit) -> Result<i64> {
    if used < 0 {
        return Err(GoalDbError::Integrity("negative request ordinal").into());
    }
    if used >= limit.get() {
        return Err(GoalRequestError::Exhausted);
    }
    used.checked_add(1).ok_or(GoalRequestError::Exhausted)
}

/// Appends to the caller's transaction; this operation never commits or dispatches.
///
/// # Errors
/// Rejects disabled policy, ineligible goals, conflicting replay, exhausted
/// capacity, invalid live bindings and storage/decode failures.
pub async fn append_request_tx(
    tx: &mut GoalTx<'_>,
    installation: GoalExecutionInstallation,
    intent: &GoalRequestIntent,
) -> Result<GoalRequestAdmission> {
    if installation == GoalExecutionInstallation::Disabled {
        return Err(GoalRequestError::Disabled);
    }
    if let Some(retained) = retained_request_tx(tx, installation, intent).await? {
        return Ok(retained);
    }
    let limit: Option<i64> =
        sqlx::query_scalar("SELECT request_limit FROM conversation_goal_limits WHERE goal_id=?")
            .bind(intent.goal().goal_id().as_str())
            .fetch_optional(&mut **tx)
            .await?;
    let limit = GoalRequestLimit::parse(limit.ok_or(GoalRequestError::Ineligible)?)
        .map_err(GoalDbError::from)?;
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal),0) FROM conversation_goal_requests WHERE goal_id=?",
    )
    .bind(intent.goal().goal_id().as_str())
    .fetch_one(&mut **tx)
    .await?;
    let ordinal = next_ordinal(used, limit)?;
    let (cause, predecessor, summary) = cause_parts(intent.cause());
    sqlx::query(
        "INSERT INTO conversation_goal_requests
         (request_id,goal_id,ordinal,owner_id,observed_version,historical_workflow_id,
          historical_turn_id,generation,cause,predecessor_request_id,summary_operation,live_turn_id)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(intent.id().as_str())
    .bind(intent.goal().goal_id().as_str())
    .bind(ordinal)
    .bind(intent.goal().owner().as_str())
    .bind(intent.goal().version().get())
    .bind(intent.execution.workflow_id())
    .bind(intent.execution.turn_id())
    .bind(intent.generation())
    .bind(cause)
    .bind(predecessor)
    .bind(summary)
    .bind(intent.execution.turn_id())
    .execute(&mut **tx)
    .await?;
    Ok(GoalRequestAdmission {
        intent: intent.clone(),
        ordinal,
        live_turn_id: Some(intent.execution.turn_id()),
        checkpoint: None,
    })
}

pub(crate) async fn require_live_request_tx(
    tx: &mut GoalTx<'_>,
    intent: &GoalRequestIntent,
    transcript: &str,
) -> Result<()> {
    super::admission::require_target_tx(tx, intent.goal().owner(), transcript).await?;
    super::control::require_current_control_tx(tx, intent.goal().owner(), intent.execution)
        .await
        .map_err(GoalDbError::from)?;
    let binding = super::binding_tx(tx, intent.execution.turn_id())
        .await?
        .ok_or(GoalRequestError::Detached)?;
    let current = super::current_tx(tx, intent.goal().owner())
        .await?
        .ok_or(GoalRequestError::Revoked)?;
    if binding.execution() != intent.execution
        || binding.observation()
            != &phoenix_core::domain::conversation_goal::GoalAdmissionObservation::Goal(
                intent.goal().clone(),
            )
        || current.fence() != *intent.goal()
        || current.state() != &phoenix_core::domain::conversation_goal::GoalState::Underway
    {
        return Err(GoalRequestError::Revoked);
    }
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM steering_messages WHERE conversation_id=?)",
    )
    .bind(transcript)
    .fetch_one(&mut **tx)
    .await?;
    if queued {
        return Err(GoalRequestError::QueuedInput);
    }
    Ok(())
}

/// Adds immutable checkpoint evidence to the caller's uncommitted transaction.
///
/// # Errors
/// Rejects disabled policy, changed identity/checkpoint, a detached or stale
/// execution, and storage/decode failures.
pub async fn checkpoint_request_tx(
    tx: &mut GoalTx<'_>,
    installation: GoalExecutionInstallation,
    intent: &GoalRequestIntent,
    claim_generation: i64,
    checkpoint: &GoalText,
) -> Result<()> {
    if installation == GoalExecutionInstallation::Disabled {
        return Err(GoalRequestError::Disabled);
    }
    let retained = retained_request_tx(tx, installation, intent)
        .await?
        .ok_or(GoalRequestError::Conflict)?;
    if let Some(previous) = retained.checkpoint {
        return if &previous == checkpoint {
            Ok(())
        } else {
            Err(GoalRequestError::Conflict)
        };
    }
    if retained.live_turn_id.is_none() {
        return Err(GoalRequestError::Detached);
    }
    let live_claim: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM durable_turns WHERE turn_id=? AND workflow_id=?
         AND generation=? AND disposition='Runtime' AND terminal_kind IS NULL AND owns_conversation=1)",
    ).bind(intent.execution.turn_id()).bind(intent.execution.workflow_id())
        .bind(claim_generation).fetch_one(&mut **tx).await?;
    if !live_claim {
        return Err(GoalRequestError::Detached);
    }
    sqlx::query("INSERT INTO conversation_goal_request_checkpoints (request_id,checkpoint_key) VALUES (?,?)")
        .bind(intent.id().as_str()).bind(checkpoint.as_str()).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod arithmetic_tests {
    use super::*;

    #[test]
    fn conversation_goal_execution_next_ordinal_is_bounded_without_overflow() {
        let maximum = GoalRequestLimit::parse(i64::MAX).unwrap();
        assert_eq!(next_ordinal(0, maximum).unwrap(), 1);
        assert_eq!(next_ordinal(i64::MAX - 1, maximum).unwrap(), i64::MAX);
        assert!(matches!(
            next_ordinal(i64::MAX, maximum),
            Err(GoalRequestError::Exhausted)
        ));
        assert!(matches!(
            next_ordinal(1, GoalRequestLimit::parse(1).unwrap()),
            Err(GoalRequestError::Exhausted)
        ));
        assert!(matches!(
            next_ordinal(-1, maximum),
            Err(GoalRequestError::Goal(GoalDbError::Integrity(_)))
        ));
    }
}
