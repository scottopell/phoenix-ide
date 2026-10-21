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
    #[error(transparent)]
    Goal(#[from] GoalDbError),
    #[error(transparent)]
    Database(#[from] crate::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

type Result<T> = std::result::Result<T, GoalStartError>;

impl crate::Database {
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
        let GoalStartRequester::Owner { principal_id } = &input.requester else {
            return Err(GoalStartError::Unauthorized);
        };
        if principal_id.to_string() != self.instance_id().await?.to_string() {
            return Err(GoalStartError::Unauthorized);
        }
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
