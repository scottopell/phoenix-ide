use phoenix_core::domain::conversation_goal::{
    GoalAgentSource, GoalExecution, GoalFence, GoalId, GoalStartInput, GoalStartReceipt,
    GoalStartRequester, GoalText, GoalVersion,
};
use sqlx::Row;

use super::{execution::GoalExecutionInstallation, GoalDbError, GoalTx};

#[derive(Debug, thiserror::Error)]
pub enum GoalStartError {
    #[error("goal execution is not installed")]
    Disabled,
    #[error("Start requester is not the authenticated owner")]
    Unauthorized,
    #[error("Start request identity or requester conflicts with historical acceptance")]
    Conflict,
    #[error("Start requires the exact current underway goal")]
    StaleGoal,
    #[error("Start is stopped or has no initialized control state")]
    Stopped,
    #[error(transparent)]
    Request(#[from] super::execution::GoalRequestError),
    #[error(transparent)]
    Goal(#[from] GoalDbError),
    #[error(transparent)]
    Database(#[from] crate::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalStartAcceptance {
    Accepted(GoalStartReceipt),
    Replay(GoalStartReceipt),
}

type Result<T> = std::result::Result<T, GoalStartError>;

impl crate::Database {
    /// Atomically accepts owner Start, its canonical input, workflow and bindings.
    ///
    /// # Errors
    /// Refuses disabled policy, unauthenticated or changed inputs, unavailable targets,
    /// busy/stopped owners, stale goals, depleted capacity and unreadable evidence.
    #[allow(clippy::too_many_lines)]
    pub async fn accept_owner_goal_start(
        &self,
        input: &GoalStartInput,
        expected_transcript: &str,
        at: super::GoalTimestampUs,
    ) -> Result<GoalStartAcceptance> {
        if self.goal_execution_installation() == GoalExecutionInstallation::Disabled
            || self.goal_control_installation() == super::control::GoalControlInstallation::Disabled
        {
            return Err(GoalStartError::Disabled);
        }
        self.authenticate_start_owner(input).await?;
        let mut tx =
            crate::workflow::WorkflowTx::new(self.pool().begin_with("BEGIN IMMEDIATE").await?);
        let owner = &input.goal().owner;
        super::admission::require_target_tx(&mut tx.tx, owner, expected_transcript).await?;
        if let Some(retained) = retained_start_tx(&mut tx.tx, input).await? {
            tx.tx.commit().await?;
            return Ok(GoalStartAcceptance::Replay(retained));
        }
        super::admission::require_settled_owner_tx(&mut tx.tx, owner, expected_transcript).await?;
        let current = super::current_tx(&mut tx.tx, owner)
            .await?
            .ok_or(GoalStartError::StaleGoal)?;
        if current.fence() != *input.goal()
            || current.state != phoenix_core::domain::conversation_goal::GoalState::Underway
        {
            return Err(GoalStartError::StaleGoal);
        }
        let running: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversation_goal_controls WHERE owner_id=? AND stopped=0)")
            .bind(owner.as_str()).fetch_one(&mut *tx.tx).await?;
        if running != 1 {
            return Err(GoalStartError::Stopped);
        }
        let capacity: Option<(i64, i64)> = sqlx::query_as("SELECT l.request_limit,(SELECT coalesce(max(ordinal),0) FROM conversation_goal_requests r WHERE r.goal_id=l.goal_id) FROM conversation_goal_limits l WHERE l.goal_id=?")
            .bind(current.goal_id.as_str()).fetch_optional(&mut *tx.tx).await?;
        let (limit, used) = capacity.ok_or(super::execution::GoalRequestError::Ineligible)?;
        if used >= limit {
            return Err(super::execution::GoalRequestError::Exhausted.into());
        }
        crate::workflow::direct_turn::insert_goal_start_turn_tx(
            &mut tx,
            input,
            expected_transcript,
            phoenix_workflow::Timestamp(
                u64::try_from(at.get() / 1_000_000)
                    .map_err(|_| GoalDbError::Integrity("Start time"))?,
            ),
            self.goal_control_installation(),
        )
        .await?;
        let retained = retained_start_tx(&mut tx.tx, input)
            .await?
            .ok_or(GoalDbError::Integrity("new Start receipt disappeared"))?;
        tx.tx.commit().await?;
        Ok(GoalStartAcceptance::Accepted(retained))
    }

    async fn authenticate_start_owner(&self, input: &GoalStartInput) -> Result<()> {
        let GoalStartRequester::Owner { principal_id } = &input.requester else {
            return Err(GoalStartError::Unauthorized);
        };
        if principal_id.to_string() != self.instance_id().await?.to_string() {
            return Err(GoalStartError::Unauthorized);
        }
        Ok(())
    }

    /// Looks up retained owner acceptance after authenticating its actual principal.
    ///
    /// # Errors
    /// Refuses disabled policy, a changed requester/input and unreadable evidence.
    /// Does not interpret errors as a missing receipt or check fresh Start eligibility.
    pub async fn replay_owner_goal_start(
        &self,
        input: &GoalStartInput,
    ) -> Result<Option<GoalStartReceipt>> {
        if self.goal_execution_installation() == GoalExecutionInstallation::Disabled {
            return Err(GoalStartError::Disabled);
        }
        self.authenticate_start_owner(input).await?;
        let mut tx = self.pool().begin().await?;
        let receipt = retained_start_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}

pub(crate) async fn retained_start_tx(
    tx: &mut GoalTx<'_>,
    input: &GoalStartInput,
) -> Result<Option<GoalStartReceipt>> {
    let Some(row) = sqlx::query(
        "SELECT s.*,h.version,h.state,h.reason,g.objective FROM conversation_goal_starts s
         LEFT JOIN conversation_goal_history h ON h.owner_id=s.owner_id AND h.goal_id=s.goal_id AND h.version=s.observed_version
         LEFT JOIN conversation_goal_ids g ON g.owner_id=s.owner_id AND g.goal_id=s.goal_id
         WHERE s.owner_id=? AND s.request_id=?",
    ).bind(input.goal().owner.as_str()).bind(input.request_id.to_string()).fetch_optional(&mut **tx).await? else {
        return Ok(None);
    };
    let requester = match row.try_get::<String, _>("requester_kind")?.as_str() {
        "owner" => GoalStartRequester::Owner {
            principal_id: row
                .try_get::<String, _>("owner_principal_id")?
                .parse()
                .map_err(|_| GoalDbError::Integrity("Start principal UUID"))?,
        },
        "coordinator" => GoalStartRequester::Coordinator {
            source: GoalAgentSource {
                product_conversation_id: super::product(row.try_get("source_owner_id")?)?,
                execution: GoalExecution::parse(
                    row.try_get("source_workflow_id")?,
                    row.try_get("source_turn_id")?,
                )
                .map_err(GoalDbError::from)?,
                transcript_id: GoalText::parse(row.try_get::<String, _>("source_transcript_id")?)
                    .map_err(GoalDbError::from)?,
                message_id: GoalText::parse(row.try_get::<String, _>("source_message_id")?)
                    .map_err(GoalDbError::from)?,
                tool_use_id: GoalText::parse(row.try_get::<String, _>("source_call_id")?)
                    .map_err(GoalDbError::from)?,
            },
        },
        _ => return Err(GoalDbError::Integrity("Start requester kind").into()),
    };
    let retained_input = GoalStartInput::new(
        row.try_get::<String, _>("request_id")?
            .parse()
            .map_err(|_| GoalDbError::Integrity("Start request UUID"))?,
        requester,
        GoalFence {
            owner: super::product(row.try_get("owner_id")?)?,
            goal_id: GoalId::parse(row.try_get::<String, _>("goal_id")?)
                .map_err(GoalDbError::from)?,
            version: GoalVersion::parse(row.try_get("observed_version")?)
                .map_err(GoalDbError::from)?,
        },
    )
    .map_err(GoalDbError::from)?;
    if &retained_input != input {
        return Err(GoalStartError::Conflict);
    }
    let execution = GoalExecution::parse(
        row.try_get("historical_workflow_id")?,
        row.try_get("historical_turn_id")?,
    )
    .map_err(GoalDbError::from)?;
    let receipt = GoalStartReceipt::new(
        retained_input,
        execution,
        GoalText::parse(row.try_get::<String, _>("historical_transcript_id")?)
            .map_err(GoalDbError::from)?,
        row.try_get("live_turn_id")?,
        super::snapshot(&row)?,
    )
    .map_err(GoalDbError::from)?;
    Ok(Some(receipt))
}
