use chrono::{DateTime, Utc};
use phoenix_core::domain::conversation_goal::{GoalExecution, GoalTimestampUs, ToolRoundResult};
use phoenix_core::domain::db_schema::{
    InputOrigin, Message, MessageContent, MessageType, ToolResult,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::domain::tool_result_identity::tool_result_message_id;
use sqlx::Row;

use super::{checkpoint::finalize_report_tx, GoalDbError, GoalTx, Result};
use crate::{Database, DbError, DbResult};

/// An internal round plan: pending reports have no finalized output or message row.
pub struct GoalRoundCheckpoint<'a> {
    pub assistant: &'a Message,
    pub execution: GoalExecution,
    pub results: &'a [ToolRoundResult],
    pub state: &'a ConvState,
    pub state_updated_at: DateTime<Utc>,
}

/// Values returned only after the owning transaction commits.
#[derive(Debug)]
pub struct CommittedGoalRound {
    pub results: Vec<Message>,
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
    pub replayed: bool,
}

pub(crate) fn db_error(error: GoalDbError) -> DbError {
    match error {
        GoalDbError::Sqlx(error) => DbError::Sqlx(error),
        GoalDbError::Admission(error) => error,
        other => DbError::Serialization(other.to_string()),
    }
}

pub(crate) async fn persist_finalized_round_tx(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    results: &[Message],
) -> DbResult<()> {
    if let MessageContent::Agent(blocks) = &assistant.content {
        for block in blocks {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                if name == "goal_report" && results.iter().any(|result| matches!(
                    &result.content, MessageContent::Tool(content) if &content.tool_use_id == id && !content.is_error
                )) {
                    return Err(DbError::Serialization("goal report success requires pending round finalization".into()));
                }
            }
        }
    }
    crate::insert_message_tx(tx, assistant).await?;
    for result in results {
        crate::insert_message_tx(tx, result).await?;
    }
    Ok(())
}

pub(super) fn result_message(
    assistant: &Message,
    ordinal: usize,
    result: &ToolResult,
) -> Result<Message> {
    let ordinal =
        i64::try_from(ordinal).map_err(|_| GoalDbError::Integrity("round result ordinal"))?;
    let sequence_id = assistant
        .sequence_id
        .checked_add(ordinal)
        .and_then(|seq| seq.checked_add(1))
        .ok_or(GoalDbError::Integrity("round result sequence"))?;
    let mut display_data = result.display_data().cloned();
    if let Some(duration) = result.duration_ms {
        let display = display_data.get_or_insert_with(|| serde_json::json!({}));
        let object = display
            .as_object_mut()
            .ok_or(GoalDbError::Integrity("round result display data"))?;
        object.insert("duration_ms".into(), duration.into());
    }
    Ok(Message {
        message_id: tool_result_message_id(&assistant.message_id, &result.tool_use_id),
        conversation_id: assistant.conversation_id.clone(),
        sequence_id,
        message_type: MessageType::Tool,
        content: MessageContent::tool_with_images(
            &result.tool_use_id,
            result.output(),
            result.is_error(),
            result.images().to_vec(),
        ),
        display_data,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: assistant.created_at,
    })
}

async fn retained_message_tx(tx: &mut GoalTx<'_>, id: &str) -> Result<Option<Message>> {
    Ok(sqlx::query("SELECT message_id,conversation_id,sequence_id,message_type,content,display_data,usage_data,created_at,origin_kind,origin_product_conversation_id,origin_transcript_id,origin_subscription_event_id,origin_source_instance_id,origin_source_message_id,origin_source_tool_use_id FROM messages WHERE message_id=?")
        .bind(id).try_map(crate::parse_message_row).fetch_optional(&mut **tx).await?)
}

pub(crate) async fn finalize_round_tx(
    tx: &mut GoalTx<'_>,
    checkpoint: &GoalRoundCheckpoint<'_>,
) -> Result<(Vec<Message>, bool)> {
    let assistant = checkpoint.assistant;
    let MessageContent::Agent(blocks) = &assistant.content else {
        return Err(GoalDbError::Integrity("goal round assistant"));
    };
    let calls: Vec<_> = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let result_ids: Vec<_> = checkpoint
        .results
        .iter()
        .map(ToolRoundResult::tool_use_id)
        .collect();
    let call_set: std::collections::HashSet<_> = calls.iter().copied().collect();
    let mut seen_results = std::collections::HashSet::new();
    if assistant.message_type != MessageType::Agent
        || calls.is_empty()
        || calls.len() != result_ids.len()
        || call_set.len() != calls.len()
        || result_ids
            .iter()
            .any(|id| !call_set.contains(id) || !seen_results.insert(*id))
    {
        return Err(GoalDbError::Integrity("goal round tool pairing"));
    }
    let mut retained = Vec::with_capacity(calls.len());
    for id in &calls {
        retained.push(
            retained_message_tx(tx, &tool_result_message_id(&assistant.message_id, id)).await?,
        );
    }
    let replayed = retained.iter().all(Option::is_some);
    if retained.iter().any(Option::is_some) && !replayed {
        return Err(GoalDbError::Integrity("goal round partial retained result"));
    }
    crate::insert_message_tx(tx, assistant).await?;
    let mut messages = Vec::with_capacity(calls.len());
    for (ordinal, result) in checkpoint.results.iter().enumerate() {
        let resolved = match result {
            ToolRoundResult::Finalized(result) => {
                if !result.is_error() && blocks.iter().any(|block| matches!(
                    block, ContentBlock::ToolUse { id, name, .. } if id == &result.tool_use_id && name == "goal_report"
                )) {
                    return Err(GoalDbError::Integrity("goal report result was finalized before commit"));
                }
                result.clone()
            }
            ToolRoundResult::PendingGoalReport {
                report,
                duration_ms,
            } => {
                finalize_report_tx(
                    tx,
                    assistant,
                    checkpoint.execution,
                    report,
                    *duration_ms,
                    GoalTimestampUs::parse(checkpoint.state_updated_at.timestamp_micros())?,
                )
                .await?
            }
        };
        let candidate = result_message(assistant, ordinal, &resolved)?;
        crate::insert_message_tx(tx, &candidate).await?;
        messages.push(candidate);
    }
    Ok((messages, replayed))
}

pub(crate) async fn selected_state_tx(
    tx: &mut GoalTx<'_>,
    checkpoint: &GoalRoundCheckpoint<'_>,
    replayed: bool,
) -> Result<(ConvState, DateTime<Utc>)> {
    if !replayed {
        crate::persist_tool_round_state_tx(
            tx,
            &checkpoint.assistant.conversation_id,
            checkpoint.state,
            checkpoint.state_updated_at,
        )
        .await?;
        return Ok((checkpoint.state.clone(), checkpoint.state_updated_at));
    }
    let row = sqlx::query("SELECT state,state_updated_at FROM conversations WHERE id=?")
        .bind(&checkpoint.assistant.conversation_id)
        .fetch_one(&mut **tx)
        .await?;
    let state: String = row.try_get("state")?;
    let timestamp: String = row.try_get("state_updated_at")?;
    let state =
        serde_json::from_str(&state).map_err(|_| GoalDbError::Integrity("retained round state"))?;
    let timestamp = DateTime::parse_from_rfc3339(&timestamp)
        .map_err(|_| GoalDbError::Integrity("retained round timestamp"))?
        .with_timezone(&Utc);
    Ok((state, timestamp))
}

impl Database {
    /// Commit genuine paired results, goal reports and selected state atomically.
    ///
    /// # Errors
    /// Source/identity, SQL and commit errors abort the whole round. Typed semantic
    /// refusals commit an error result instead of an accepted goal mutation.
    pub async fn persist_goal_tool_round_and_state(
        &self,
        checkpoint: &GoalRoundCheckpoint<'_>,
    ) -> DbResult<CommittedGoalRound> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let (results, replayed) = finalize_round_tx(&mut tx, checkpoint)
            .await
            .map_err(db_error)?;
        let (state, state_updated_at) = selected_state_tx(&mut tx, checkpoint, replayed)
            .await
            .map_err(db_error)?;
        tx.commit().await?;
        Ok(CommittedGoalRound {
            results,
            state,
            state_updated_at,
            replayed,
        })
    }
}
