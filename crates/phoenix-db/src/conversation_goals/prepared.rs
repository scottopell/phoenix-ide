//! Immutable Coordinator Goal actions. Preparing never changes a Goal or dispatches work.
use phoenix_core::domain::conversation_goal::{
    GoalAgentSource, GoalCommandDisposition, GoalExecution, GoalId, GoalState, GoalText,
    GoalVersion,
};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{sqlite::SqliteRow, Row};
use uuid::Uuid;

use super::{current_tx, GoalDbError, GoalTx, Result};
use crate::Database;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalPreparationIntent {
    Create { objective: GoalText },
    Replace { objective: GoalText },
    State { disposition: GoalCommandDisposition },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedGoalAction {
    pub reference: Uuid,
    pub owner: ProductConversationId,
    pub source: GoalAgentSource,
    pub expected_version: GoalVersion,
    pub observed_goal_id: Option<GoalId>,
    pub intent: GoalPreparationIntent,
}

impl PreparedGoalAction {
    pub fn command(&self, principal_id: Uuid) -> Result<super::user_commands::GoalUserCommand> {
        use super::user_commands::{GoalUserChange, GoalUserCommand};
        let change = match &self.intent {
            GoalPreparationIntent::Create { objective }
            | GoalPreparationIntent::Replace { objective } => GoalUserChange::Create {
                objective: objective.clone(),
            },
            GoalPreparationIntent::State { disposition } => GoalUserChange::State {
                goal_id: self
                    .observed_goal_id
                    .clone()
                    .ok_or(GoalDbError::Integrity("prepared state goal"))?,
                disposition: disposition.clone().into(),
            },
        };
        Ok(GoalUserCommand {
            owner: self.owner.clone(),
            principal_id,
            request_id: self.reference,
            expected_version: self.expected_version,
            change,
        })
    }
}

pub(super) async fn require_coordinator(tx: &mut GoalTx<'_>, caller: &str) -> Result<()> {
    let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id=? AND runtime_role='coordinator' AND coordinator_head=1)")
        .bind(caller).fetch_one(&mut **tx).await?;
    if !authorized {
        return Err(GoalDbError::TargetChanged);
    }
    Ok(())
}

fn columns(
    intent: &GoalPreparationIntent,
) -> (&'static str, Option<&str>, Option<&str>, Option<&str>) {
    match intent {
        GoalPreparationIntent::Create { objective } => {
            ("create", Some(objective.as_str()), None, None)
        }
        GoalPreparationIntent::Replace { objective } => {
            ("replace", Some(objective.as_str()), None, None)
        }
        GoalPreparationIntent::State { disposition } => {
            let (state, reason) = match disposition {
                GoalCommandDisposition::Underway => ("underway", None),
                GoalCommandDisposition::Blocked(reason) => ("blocked", Some(reason.as_str())),
                GoalCommandDisposition::Complete(reason) => ("complete", Some(reason.as_str())),
                GoalCommandDisposition::Exhausted(reason) => ("exhausted", Some(reason.as_str())),
            };
            ("state", None, Some(state), reason)
        }
    }
}

fn decode(row: &SqliteRow) -> Result<PreparedGoalAction> {
    let reference = Uuid::from_slice(&row.try_get::<Vec<u8>, _>("reference")?)
        .map_err(|_| GoalDbError::Integrity("preparation reference"))?;
    let source = GoalAgentSource {
        product_conversation_id: ProductConversationId::parse(
            row.try_get::<String, _>("source_product_id")?,
        )
        .map_err(|_| GoalDbError::Integrity("source product"))?,
        transcript_id: GoalText::parse(row.try_get::<String, _>("caller_conversation_id")?)?,
        execution: GoalExecution::parse(row.try_get("workflow_id")?, row.try_get("turn_id")?)?,
        message_id: GoalText::parse(row.try_get::<String, _>("message_id")?)?,
        tool_use_id: GoalText::parse(row.try_get::<String, _>("tool_use_id")?)?,
    };
    let objective = row
        .try_get::<Option<String>, _>("objective")?
        .map(GoalText::parse)
        .transpose()?;
    let operation: String = row.try_get("operation")?;
    let intent = match operation.as_str() {
        "create" => GoalPreparationIntent::Create {
            objective: objective.ok_or(GoalDbError::Integrity("prepared objective"))?,
        },
        "replace" => GoalPreparationIntent::Replace {
            objective: objective.ok_or(GoalDbError::Integrity("prepared objective"))?,
        },
        "state" => {
            let state: String = row.try_get("disposition")?;
            let reason = row
                .try_get::<Option<String>, _>("reason")?
                .map(GoalText::parse)
                .transpose()?;
            let disposition = match (state.as_str(), reason) {
                ("underway", None) => GoalCommandDisposition::Underway,
                ("blocked", Some(reason)) => GoalCommandDisposition::Blocked(reason),
                ("complete", Some(reason)) => GoalCommandDisposition::Complete(reason),
                ("exhausted", Some(reason)) => GoalCommandDisposition::Exhausted(reason),
                _ => return Err(GoalDbError::Integrity("prepared disposition")),
            };
            GoalPreparationIntent::State { disposition }
        }
        _ => return Err(GoalDbError::Integrity("prepared operation")),
    };
    Ok(PreparedGoalAction {
        reference,
        owner: ProductConversationId::parse(row.try_get::<String, _>("owner_id")?)
            .map_err(|_| GoalDbError::Integrity("prepared owner"))?,
        source,
        expected_version: GoalVersion::parse(row.try_get("expected_version")?)?,
        observed_goal_id: row
            .try_get::<Option<String>, _>("observed_goal_id")?
            .map(GoalId::parse)
            .transpose()?,
        intent,
    })
}

pub(super) async fn require_exact_preparation(
    tx: &mut GoalTx<'_>,
    _caller: &str,
    source: &GoalAgentSource,
    input: &super::user_commands::GoalUserCommand,
) -> Result<()> {
    let row = sqlx::query("SELECT * FROM conversation_goal_preparations WHERE reference=?")
        .bind(input.request_id.as_bytes().as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GoalDbError::TargetChanged)?;
    let prepared = decode(&row)?;
    let expected = prepared.command(input.principal_id)?;
    if &prepared.source != source
        || prepared.owner != input.owner
        || expected.expected_version != input.expected_version
        || expected.change != input.change
    {
        return Err(GoalDbError::RetryConflict);
    }
    Ok(())
}

pub(super) async fn require_retained_preparation(
    tx: &mut GoalTx<'_>,
    _caller: &str,
    input: &super::user_commands::GoalUserCommand,
) -> Result<()> {
    let row = sqlx::query("SELECT * FROM conversation_goal_preparations WHERE reference=?")
        .bind(input.request_id.as_bytes().as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GoalDbError::TargetChanged)?;
    let prepared = decode(&row)?;
    let expected = prepared.command(input.principal_id)?;
    if prepared.owner != input.owner
        || expected.expected_version != input.expected_version
        || expected.change != input.change
    {
        return Err(GoalDbError::RetryConflict);
    }
    Ok(())
}

impl Database {
    pub async fn read_coordinator_goals(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<(
        Option<super::GoalHistoryEntry>,
        Vec<super::GoalHistoryEntry>,
    )> {
        let mut tx = self.pool().begin().await?;
        require_coordinator(&mut tx, caller).await?;
        let live = Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        super::admission::require_target_tx(&mut tx, owner, &live.latest_transcript_row_id).await?;
        let row = sqlx::query("SELECT h.*,g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING(goal_id,owner_id) WHERE owner_id=? ORDER BY version DESC LIMIT 1")
            .bind(owner.as_str()).fetch_optional(&mut *tx).await?;
        let current = match row {
            Some(row) => Some(super::history_entry(&mut tx, &row).await?),
            None => None,
        };
        let history = super::history_tx(&mut tx, owner, after).await?;
        tx.rollback().await?;
        Ok((current, history))
    }

    pub async fn require_current_coordinator(&self, caller: &str) -> Result<()> {
        let mut tx = self.pool().begin().await?;
        require_coordinator(&mut tx, caller).await?;
        tx.rollback().await?;
        Ok(())
    }

    pub async fn prepare_coordinator_goal_action(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        source_call: &SourceToolCall,
        turn_id: phoenix_core::domain::sm_event::DirectTurnTurnId,
        intent: GoalPreparationIntent,
    ) -> Result<PreparedGoalAction> {
        let turn_id = i64::try_from(turn_id.0)
            .map_err(|_| GoalDbError::Invalid("durable turn ID is out of range"))?;
        let mut connection = self.pool().acquire().await?;
        let mut tx = sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await?;
        require_coordinator(&mut tx, caller).await?;
        if let Some(row) = sqlx::query("SELECT * FROM conversation_goal_preparations WHERE caller_conversation_id=? AND message_id=? AND tool_use_id=?")
            .bind(caller).bind(&source_call.message_id).bind(&source_call.tool_use_id).fetch_optional(&mut *tx).await? {
            let previous = decode(&row)?;
            if previous.owner != *owner || previous.intent != intent || previous.source.execution.turn_id() != turn_id { return Err(GoalDbError::RetryConflict); }
            tx.rollback().await?;
            return Ok(previous);
        }
        let source_row = sqlx::query("SELECT c.product_conversation_id,t.workflow_id,t.turn_id FROM conversations c JOIN durable_turns t ON t.conversation_id=c.id WHERE c.id=? AND t.turn_id=? AND t.disposition='Runtime' AND t.terminal_kind IS NULL AND t.owns_conversation=1")
            .bind(caller).bind(turn_id).fetch_optional(&mut *tx).await?.ok_or(GoalDbError::TargetChanged)?;
        let source = GoalAgentSource {
            product_conversation_id: ProductConversationId::parse(
                source_row.try_get::<String, _>("product_conversation_id")?,
            )
            .map_err(|_| GoalDbError::Integrity("source product"))?,
            transcript_id: GoalText::parse(caller)?,
            execution: GoalExecution::parse(
                source_row.try_get("workflow_id")?,
                source_row.try_get("turn_id")?,
            )?,
            message_id: GoalText::parse(source_call.message_id.clone())?,
            tool_use_id: GoalText::parse(source_call.tool_use_id.clone())?,
        };
        let message: Option<String> = sqlx::query_scalar("SELECT content FROM messages WHERE conversation_id=? AND message_id=? AND message_type='agent'")
            .bind(caller).bind(&source_call.message_id).fetch_optional(&mut *tx).await?;
        let blocks: Vec<phoenix_core::domain::llm_types::ContentBlock> =
            serde_json::from_str(&message.ok_or(GoalDbError::TargetChanged)?)
                .map_err(|_| GoalDbError::Integrity("source tool message content"))?;
        if !blocks.iter().any(|block| matches!(block, phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, .. } if id == &source_call.tool_use_id)) { return Err(GoalDbError::TargetChanged); }
        let live = Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        super::admission::require_target_tx(&mut tx, owner, &live.latest_transcript_row_id).await?;
        let current = current_tx(&mut tx, owner).await?;
        let (expected_version, observed_goal_id) = match (&intent, current.as_ref()) {
            (GoalPreparationIntent::Create { .. }, None) => (GoalVersion::INITIAL, None),
            (GoalPreparationIntent::Replace { .. }, Some(current)) => {
                if matches!(current.state(), GoalState::Suspended(_)) {
                    return Err(GoalDbError::Invalid("Suspended Goal requires explicit Start; replacement tools cannot reactivate it"));
                }
                (current.version(), Some(current.goal_id().clone()))
            }
            (GoalPreparationIntent::State { disposition }, Some(current)) => {
                if matches!(disposition, GoalCommandDisposition::Underway)
                    && matches!(current.state(), GoalState::Suspended(_))
                {
                    return Err(GoalDbError::Invalid("Suspended Goal requires explicit Start; disposition tools cannot reactivate it"));
                }
                (current.version(), Some(current.goal_id().clone()))
            }
            (GoalPreparationIntent::Create { .. }, Some(_)) => {
                return Err(GoalDbError::Invalid(
                    "Goal exists; use explicit replace after reading current Goal/history",
                ))
            }
            _ => return Err(GoalDbError::Invalid("No saved Goal; use create first")),
        };
        let reference = Uuid::new_v4();
        let (operation, objective, disposition, reason) = columns(&intent);
        sqlx::query("INSERT INTO conversation_goal_preparations(reference,owner_id,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,operation,expected_version,observed_goal_id,objective,disposition,reason) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(reference.as_bytes().as_slice()).bind(owner.as_str()).bind(caller).bind(source.product_conversation_id.as_str())
            .bind(source.execution.workflow_id()).bind(source.execution.turn_id()).bind(source.message_id.as_str()).bind(source.tool_use_id.as_str())
            .bind(operation).bind(expected_version.get()).bind(observed_goal_id.as_ref().map(GoalId::as_str)).bind(objective).bind(disposition).bind(reason).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(PreparedGoalAction {
            reference,
            owner: owner.clone(),
            source,
            expected_version,
            observed_goal_id,
            intent,
        })
    }

    pub async fn read_coordinator_goal_preparation(
        &self,
        caller: &str,
        reference: Uuid,
    ) -> Result<PreparedGoalAction> {
        let mut tx = self.pool().begin().await?;
        require_coordinator(&mut tx, caller).await?;
        let row = sqlx::query("SELECT * FROM conversation_goal_preparations WHERE reference=?")
            .bind(reference.as_bytes().as_slice())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(GoalDbError::Invalid(
                "Unknown Goal action reference; prepare an explicit new action",
            ))?;
        let result = decode(&row)?;
        tx.rollback().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
