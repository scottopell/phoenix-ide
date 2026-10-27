use chrono::{DateTime, Utc};
use phoenix_core::domain::conversation_goal::{GoalExecution, GoalTimestampUs, ToolRoundResult};
use phoenix_core::domain::db_schema::{
    InputOrigin, Message, MessageContent, MessageType, ToolResult,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::domain::tool_result_identity::tool_result_message_id;
use sqlx::Row;

use super::{GoalDbError, GoalTx, Result};
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

/// Committed tool evidence; the existing terminal obligation owns state projection.
#[derive(Debug)]
pub struct CommittedGoalTerminalRound {
    pub results: Vec<Message>,
    pub replayed: bool,
}

pub(crate) enum ToolRoundWrite<'a> {
    Finalized(&'a [Message]),
    Pending {
        execution: GoalExecution,
        results: &'a [ToolRoundResult],
    },
}
pub(crate) fn db_error(error: GoalDbError) -> DbError {
    match error {
        GoalDbError::Sqlx(error) => DbError::Sqlx(error),
        GoalDbError::Control(error) => super::control::db_error(error),
        GoalDbError::Admission(error) => error,
        other @ (GoalDbError::Busy
        | GoalDbError::TargetChanged
        | GoalDbError::Domain(_)
        | GoalDbError::RetryConflict
        | GoalDbError::DuplicateGoalId
        | GoalDbError::BindingConflict
        | GoalDbError::Integrity(_)) => DbError::Serialization(other.to_string()),
    }
}
pub(crate) async fn persist_finalized_round_tx(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    results: &[Message],
) -> DbResult<()> {
    let MessageContent::Agent(blocks) = &assistant.content else {
        return Err(DbError::Serialization(
            "tool round requires a genuine assistant".into(),
        ));
    };
    let mut calls = std::collections::HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolUse { id, name, .. } = block {
            if calls.insert(id.as_str(), name.as_str()).is_some() {
                return Err(DbError::Serialization("tool round duplicate call".into()));
            }
        }
    }
    if assistant.message_type != MessageType::Agent || calls.len() != results.len() {
        return Err(DbError::Serialization(
            "tool round call/result cardinality".into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for result in results {
        let MessageContent::Tool(content) = &result.content else {
            return Err(DbError::Serialization(
                "tool round requires tool results".into(),
            ));
        };
        let Some(name) = calls.get(content.tool_use_id.as_str()) else {
            return Err(DbError::Serialization(
                "tool round result has no owning call".into(),
            ));
        };
        if result.message_type != MessageType::Tool
            || result.conversation_id != assistant.conversation_id
            || result.message_id
                != tool_result_message_id(&assistant.message_id, &content.tool_use_id)
            || !seen.insert(content.tool_use_id.as_str())
        {
            return Err(DbError::Serialization(
                "tool round result owning assistant differs".into(),
            ));
        }
        if *name == "goal_report" && !content.is_error {
            return Err(DbError::Serialization(
                "goal report success requires pending round finalization".into(),
            ));
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
        content: MessageContent::Tool(result.persisted_tool_content()),
        display_data,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: assistant.created_at,
    })
}

pub(crate) async fn retained_message_tx(tx: &mut GoalTx<'_>, id: &str) -> Result<Option<Message>> {
    Ok(sqlx::query("SELECT message_id,conversation_id,sequence_id,message_type,content,display_data,usage_data,created_at,origin_kind,origin_product_conversation_id,origin_transcript_id,origin_subscription_event_id,origin_source_message_id,origin_source_tool_use_id FROM messages WHERE message_id=?")
        .bind(id).try_map(crate::parse_message_row).fetch_optional(&mut **tx).await?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_round_tx(
    tx: &mut GoalTx<'_>,
    conversation_id: &str,
    assistant: &Message,
    input: ToolRoundWrite<'_>,
    state: &ConvState,
    state_updated_at: DateTime<Utc>,
    policies: super::execution::GoalWritePolicies,
) -> DbResult<(Vec<Message>, bool)> {
    if conversation_id != assistant.conversation_id {
        return Err(DbError::Serialization(
            "tool round transcript differs".into(),
        ));
    }
    match input {
        ToolRoundWrite::Finalized(results) => {
            persist_finalized_round_tx(tx, assistant, results).await?;
            Ok((results.to_vec(), false))
        }
        ToolRoundWrite::Pending { execution, results } => finalize_round_tx(
            tx,
            &GoalRoundCheckpoint {
                assistant,
                execution,
                results,
                state,
                state_updated_at,
            },
            policies,
        )
        .await
        .map_err(db_error),
    }
}

pub(crate) fn validate_round_shape(checkpoint: &GoalRoundCheckpoint<'_>) -> Result<()> {
    let assistant = checkpoint.assistant;
    let MessageContent::Agent(blocks) = &assistant.content else {
        return Err(GoalDbError::Integrity("goal round assistant"));
    };
    let calls: Vec<_> = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            ContentBlock::Text { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::WebSearchToolResult { .. }
            | ContentBlock::WebFetchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. }
            | ContentBlock::BashCodeExecutionToolResult { .. }
            | ContentBlock::TextEditorCodeExecutionToolResult { .. }
            | ContentBlock::McpToolUse { .. }
            | ContentBlock::McpToolResult { .. } => None,
        })
        .collect();
    let result_ids: Vec<_> = checkpoint
        .results
        .iter()
        .map(ToolRoundResult::tool_use_id)
        .collect();
    let call_set: std::collections::HashSet<_> = calls.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    if assistant.message_type != MessageType::Agent
        || calls.is_empty()
        || calls.len() != result_ids.len()
        || call_set.len() != calls.len()
        || result_ids
            .iter()
            .any(|id| !call_set.contains(id) || !seen.insert(*id))
    {
        return Err(GoalDbError::Integrity("goal round tool pairing"));
    }
    for result in checkpoint.results {
        if let ToolRoundResult::Finalized(result) = result {
            if result.is_success() && blocks.iter().any(|block| matches!(block,
                ContentBlock::ToolUse { id, name, .. } if id == &result.tool_use_id && name == "goal_report")) {
                return Err(GoalDbError::Integrity("goal report result finalized before commit"));
            }
        }
    }
    Ok(())
}

pub(crate) async fn retained_round_tx(
    tx: &mut GoalTx<'_>,
    checkpoint: &GoalRoundCheckpoint<'_>,
) -> Result<Option<Vec<Message>>> {
    validate_round_shape(checkpoint)?;
    let assistant = checkpoint.assistant;
    let retained_assistant = retained_message_tx(tx, &assistant.message_id).await?;
    let mut retained = Vec::with_capacity(checkpoint.results.len());
    for result in checkpoint.results {
        retained.push(
            retained_message_tx(
                tx,
                &tool_result_message_id(&assistant.message_id, result.tool_use_id()),
            )
            .await?,
        );
    }
    if retained.iter().all(Option::is_none) {
        if let Some(actual) = retained_assistant {
            if !message_matches(&actual, assistant) {
                return Err(GoalDbError::Integrity("retained round assistant differs"));
            }
        }
        return Ok(None);
    }
    let retained_assistant =
        retained_assistant.ok_or(GoalDbError::Integrity("retained round missing assistant"))?;
    if !message_matches(&retained_assistant, assistant) {
        return Err(GoalDbError::Integrity("retained round assistant differs"));
    }
    let mut messages = Vec::with_capacity(retained.len());
    for (ordinal, (intent, actual)) in checkpoint.results.iter().zip(retained).enumerate() {
        let actual = actual.ok_or(GoalDbError::Integrity("retained round missing result"))?;
        let finalized = match intent {
            ToolRoundResult::Finalized(result) => result.clone(),
            ToolRoundResult::PendingGoalReport { report, .. } => {
                super::checkpoint::verify_source_tx(tx, assistant, checkpoint.execution, report)
                    .await?;
                super::checkpoint::retained_result_tx(tx, report, &actual.message_id)
                    .await?
                    .ok_or(GoalDbError::Integrity("retained report missing result"))?
            }
        };
        if !message_matches(&actual, &result_message(assistant, ordinal, &finalized)?) {
            return Err(GoalDbError::Integrity("retained round result differs"));
        }
        messages.push(actual);
    }
    Ok(Some(messages))
}

async fn finalize_round_tx(
    tx: &mut GoalTx<'_>,
    checkpoint: &GoalRoundCheckpoint<'_>,
    policies: super::execution::GoalWritePolicies,
) -> Result<(Vec<Message>, bool)> {
    if let Some(retained) = retained_round_tx(tx, checkpoint).await? {
        return Ok((retained, true));
    }
    let assistant = checkpoint.assistant;
    crate::insert_message_tx(tx, assistant).await?;
    let mut messages = Vec::with_capacity(checkpoint.results.len());
    for (ordinal, result) in checkpoint.results.iter().enumerate() {
        let resolved = match result {
            ToolRoundResult::Finalized(result) => result.clone(),
            ToolRoundResult::PendingGoalReport {
                report,
                duration_ms,
            } => {
                super::checkpoint::finalize_report_tx(
                    tx,
                    assistant,
                    checkpoint.execution,
                    report,
                    *duration_ms,
                    GoalTimestampUs::parse(checkpoint.state_updated_at.timestamp_micros())?,
                    policies,
                )
                .await?
            }
        };
        let candidate = result_message(assistant, ordinal, &resolved)?;
        crate::insert_message_tx(tx, &candidate).await?;
        messages.push(candidate);
    }
    Ok((messages, false))
}

fn message_matches(existing: &Message, expected: &Message) -> bool {
    existing.message_id == expected.message_id
        && existing.conversation_id == expected.conversation_id
        && existing.sequence_id == expected.sequence_id
        && existing.message_type == expected.message_type
        && existing.content == expected.content
        && existing.origin == expected.origin
        && existing.display_data == expected.display_data
        && existing.usage_data == expected.usage_data
        && existing.created_at == expected.created_at
}

pub(crate) async fn selected_state_tx(
    tx: &mut GoalTx<'_>,
    conversation_id: &str,
    state: &ConvState,
    state_updated_at: DateTime<Utc>,
    replayed: bool,
) -> Result<(ConvState, DateTime<Utc>)> {
    if !replayed {
        crate::coordinator_watches::record_wait_entry_tx(tx, conversation_id, state).await?;
        let state_json = serde_json::to_string(state)
            .map_err(|_| GoalDbError::Integrity("selected round state serialization"))?;
        let updated = sqlx::query(
            "UPDATE conversations SET state=?1,state_kind=?2,state_updated_at=?3,updated_at=?4 WHERE id=?5",
        )
        .bind(state_json)
        .bind(crate::conv_state_kind(state))
        .bind(state_updated_at.to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .bind(conversation_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(GoalDbError::Integrity(
                "selected round conversation missing",
            ));
        }
        return Ok((state.clone(), state_updated_at));
    }
    let row = sqlx::query("SELECT state,state_updated_at FROM conversations WHERE id=?")
        .bind(conversation_id)
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
    /// Commit a pending report round and selected state atomically.
    ///
    /// # Errors
    /// Rejects invalid source evidence or a failed canonical transaction.
    pub async fn persist_goal_tool_round_and_state(
        &self,
        checkpoint: &GoalRoundCheckpoint<'_>,
    ) -> DbResult<CommittedGoalRound> {
        self.persist_tool_round_state_and_replay_input(
            &checkpoint.assistant.conversation_id,
            checkpoint.assistant,
            ToolRoundWrite::Pending {
                execution: checkpoint.execution,
                results: checkpoint.results,
            },
            checkpoint.state,
            checkpoint.state_updated_at,
            None,
        )
        .await
    }

    /// Commit a pending report round, selected state and provider replay atomically.
    ///
    /// # Errors
    /// Rejects invalid source/replay evidence or a failed canonical transaction.
    pub async fn persist_goal_tool_round_state_and_replay(
        &self,
        checkpoint: &GoalRoundCheckpoint<'_>,
        update: &phoenix_core::domain::provider_replay::ProviderReplayUpdate,
    ) -> DbResult<CommittedGoalRound> {
        self.persist_tool_round_state_and_replay_input(
            &checkpoint.assistant.conversation_id,
            checkpoint.assistant,
            ToolRoundWrite::Pending {
                execution: checkpoint.execution,
                results: checkpoint.results,
            },
            checkpoint.state,
            checkpoint.state_updated_at,
            Some(update),
        )
        .await
    }

    /// Commit report evidence and the existing terminal obligation without projecting state.
    ///
    /// # Errors
    /// Rejects mismatched turn evidence or a failed canonical transaction.
    pub async fn persist_goal_tool_round_with_terminal_obligation(
        &self,
        assistant: &Message,
        execution: GoalExecution,
        results: &[ToolRoundResult],
        obligation: &crate::workflow::DirectTurnTerminalObligationInput,
        replay: Option<&phoenix_core::domain::provider_replay::ProviderReplayUpdate>,
    ) -> DbResult<CommittedGoalTerminalRound> {
        if i64::try_from(obligation.turn_id.0).ok() != Some(execution.turn_id()) {
            return Err(DbError::Serialization(
                "goal report terminal turn differs".into(),
            ));
        }
        self.persist_tool_round_with_terminal_obligation_input_at_cut(
            &assistant.conversation_id,
            assistant,
            ToolRoundWrite::Pending { execution, results },
            obligation,
            replay,
            crate::TerminalEvidenceTransactionCut::None,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use phoenix_core::domain::db_schema::InputOrigin;

    fn fixture(name: &str, result: ToolResult) -> (Message, Vec<ToolRoundResult>, ConvState) {
        let assistant = Message {
            message_id: "assistant-round".into(),
            conversation_id: "transcript-round".into(),
            sequence_id: 10,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(vec![ContentBlock::ToolUse {
                id: "call-round".into(),
                name: name.into(),
                input: serde_json::json!({"reason":"done"}),
            }]),
            display_data: None,
            usage_data: None,
            origin: InputOrigin::SystemGenerated,
            created_at: Utc::now(),
        };
        (
            assistant,
            vec![ToolRoundResult::Finalized(result)],
            ConvState::Idle,
        )
    }

    #[test]
    fn round_shape_accepts_a_real_paired_tool_result() {
        let (assistant, results, state) = fixture(
            "search",
            ToolResult::success("call-round".into(), "found".into()),
        );
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 2).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        assert!(validate_round_shape(&checkpoint).is_ok());
    }

    #[test]
    fn finalized_success_cannot_bypass_report_commit_path() {
        let (assistant, results, state) = fixture(
            "goal_report",
            ToolResult::success("call-round".into(), "Complete".into()),
        );
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 2).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        assert!(validate_round_shape(&checkpoint).is_err());
        let (assistant, results, state) = fixture(
            "goal_report",
            ToolResult::error("call-round".into(), "rejected".into()),
        );
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 2).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        assert!(validate_round_shape(&checkpoint).is_ok());
    }
}
