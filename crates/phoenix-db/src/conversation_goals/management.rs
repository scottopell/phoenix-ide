use phoenix_core::domain::{
    conversation_goal::{
        GoalAgentSource, GoalExecution, GoalManagementRequest, GoalPendingManagement,
        GoalTimestampUs,
    },
    db_schema::ToolResult,
    llm_types::ContentBlock,
    product_conversation::ProductConversationId,
};
use sqlx::Row;

use super::{GoalDbError, GoalTx, Result};
use crate::{Database, Message, MessageContent};

/// Application target-resolution snapshot for a management checkpoint.
#[derive(Debug, Clone)]
pub enum GoalManagementTarget {
    Admitted {
        owner: ProductConversationId,
        transcript: String,
    },
    Refused {
        owner: ProductConversationId,
    },
}

impl GoalManagementTarget {
    #[must_use]
    pub fn owner(&self) -> &ProductConversationId {
        match self {
            Self::Admitted { owner, .. } | Self::Refused { owner } => owner,
        }
    }
}

#[derive(Clone, Copy)]
pub enum GoalManagementRoundKind<'a> {
    Runtime,
    Fork(&'a crate::ForkProposal),
    Terminal(&'a crate::workflow::DirectTurnTerminalObligationInput),
}

impl Database {
    /// Commit admitted Coordinator mutations only with their canonical owning tool round.
    ///
    /// # Errors
    /// Rejects changed source identity, stale execution, partial retained evidence, or write failures.
    #[allow(clippy::too_many_lines)]
    pub async fn persist_goal_management_round(
        &self,
        checkpoint: &super::round::GoalRoundCheckpoint<'_>,
        targets: &[GoalManagementTarget],
        kind: GoalManagementRoundKind<'_>,
        replay: Option<&phoenix_core::domain::provider_replay::ProviderReplayUpdate>,
    ) -> crate::DbResult<super::round::CommittedGoalRound> {
        use super::round::{write_round_tx, CommittedGoalRound, ToolRoundWrite};
        let input = ToolRoundWrite::Management {
            execution: checkpoint.execution,
            results: checkpoint.results,
            targets,
        };
        match kind {
            GoalManagementRoundKind::Runtime => {
                if let Some(replay) = replay {
                    return self
                        .persist_tool_round_state_and_replay_input(
                            &checkpoint.assistant.conversation_id,
                            checkpoint.assistant,
                            input,
                            checkpoint.state,
                            checkpoint.state_updated_at,
                            replay,
                        )
                        .await;
                }
                self.persist_tool_round_and_state_input(
                    &checkpoint.assistant.conversation_id,
                    checkpoint.assistant,
                    input,
                    checkpoint.state,
                    checkpoint.state_updated_at,
                )
                .await
            }
            GoalManagementRoundKind::Fork(proposal) => {
                self.persist_fork_tool_round_input(
                    checkpoint.assistant,
                    input,
                    proposal,
                    checkpoint.state,
                    checkpoint.state_updated_at,
                    replay,
                )
                .await
            }
            GoalManagementRoundKind::Terminal(obligation) => {
                if i64::try_from(obligation.turn_id.0).ok() != Some(checkpoint.execution.turn_id())
                {
                    return Err(crate::DbError::Serialization(
                        "management terminal execution differs".into(),
                    ));
                }
                if &obligation.projection.state != checkpoint.state
                    || obligation.projection.state_updated_at != checkpoint.state_updated_at
                {
                    return Err(crate::DbError::Serialization(
                        "management terminal projection differs".into(),
                    ));
                }
                if let Some(update) = replay {
                    crate::provider_replay::validate_goal_round_replay(
                        checkpoint.assistant,
                        update,
                    )?;
                }
                let repo = self.workflow_repository();
                let mut tx = repo.begin_immediate_tx().await?;
                let (results, replayed) = write_round_tx(
                    &mut tx.tx,
                    &checkpoint.assistant.conversation_id,
                    checkpoint.assistant,
                    input,
                    checkpoint.state,
                    checkpoint.state_updated_at,
                    self.goal_control_installation,
                )
                .await?;
                if !replayed {
                    if let Some(update) = replay {
                        crate::provider_replay::apply_replay_tx(
                            &mut tx.tx,
                            &checkpoint.assistant.conversation_id,
                            update,
                        )
                        .await?;
                    }
                }
                let command = match &obligation.terminal {
                    phoenix_workflow::TurnTerminal::Completed => {
                        phoenix_workflow::TurnCommand::Complete {
                            turn_id: obligation.turn_id,
                            expected_generation: obligation.expected_generation,
                        }
                    }
                    phoenix_workflow::TurnTerminal::Cancelled => {
                        phoenix_workflow::TurnCommand::Cancel {
                            turn_id: obligation.turn_id,
                            expected_generation: obligation.expected_generation,
                        }
                    }
                    phoenix_workflow::TurnTerminal::Failed { reason } => {
                        phoenix_workflow::TurnCommand::Fail {
                            turn_id: obligation.turn_id,
                            expected_generation: obligation.expected_generation,
                            reason: reason.clone(),
                        }
                    }
                };
                let terminal = repo.terminalize_authoritative_turn_in_tx(&mut tx, &crate::workflow::TerminalizeAuthoritativeTurnInput {
                    command, projection: Some(obligation.projection.clone()),
                    provider_replay_settlement: phoenix_core::domain::provider_replay::ProviderReplaySettlement::for_conversation_state(&checkpoint.assistant.conversation_id, checkpoint.state),
                }).await?;
                if !replayed
                    && matches!(
                        terminal.outcome,
                        phoenix_workflow::TurnOutcome::TerminalReplay { .. }
                    )
                {
                    return Err(crate::DbError::Serialization(
                        "settled management round lost exact pair".into(),
                    ));
                }
                tx.tx.commit().await?;
                Ok(CommittedGoalRound {
                    results,
                    state: checkpoint.state.clone(),
                    state_updated_at: checkpoint.state_updated_at,
                    replayed,
                })
            }
        }
    }

    /// Read an exact accepted or refused round before any application gates are acquired.
    ///
    /// # Errors
    /// Rejects partial or changed retained evidence.
    pub async fn retained_goal_management_round(
        &self,
        checkpoint: &super::round::GoalRoundCheckpoint<'_>,
    ) -> crate::DbResult<Option<Vec<Message>>> {
        super::round::validate_round_shape(checkpoint).map_err(super::round::db_error)?;
        let mut tx = self.pool().begin().await?;
        let retained = super::round::retained_round_tx(&mut tx, checkpoint)
            .await
            .map_err(super::round::db_error)?;
        tx.commit().await?;
        Ok(retained)
    }

    /// Resolve only the canonical Coordinator's live, generation-fenced Runtime turn.
    ///
    /// # Errors
    /// Refuses an ordinary, retired, non-head, stale, or unowned execution.
    pub async fn goal_management_execution(
        &self,
        transcript: &str,
        turn: i64,
        generation: u64,
    ) -> Result<(ProductConversationId, GoalExecution)> {
        let generation = i64::try_from(generation).map_err(|_| GoalDbError::BindingConflict)?;
        let row = sqlx::query("SELECT c.product_conversation_id,t.workflow_id FROM conversations c
            JOIN product_conversations p ON p.id=c.product_conversation_id
            JOIN durable_turns t ON t.conversation_id=c.id
            JOIN messages u ON u.message_id=t.canonical_message_id AND u.conversation_id=c.id AND u.message_type='user'
            WHERE c.id=? AND c.coordinator_head=1 AND c.runtime_role='coordinator' AND p.kind='coordinator'
              AND t.turn_id=? AND t.generation=? AND t.disposition='Runtime'
              AND t.owns_conversation=1 AND t.terminal_kind IS NULL")
            .bind(transcript).bind(turn).bind(generation).fetch_optional(self.pool()).await?
            .ok_or(GoalDbError::BindingConflict)?;
        Ok((
            super::product(row.try_get::<String, _>("product_conversation_id")?)?,
            GoalExecution::parse(row.try_get("workflow_id")?, turn)?,
        ))
    }
}

pub(super) async fn verify_source_tx(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    execution: GoalExecution,
    intent: &GoalPendingManagement,
    live: bool,
) -> Result<()> {
    let source = intent.source();
    if source.transcript_id.as_str() != assistant.conversation_id
        || source.message_id.as_str() != assistant.message_id
        || source.execution != execution
    {
        return Err(GoalDbError::Integrity("management owning round differs"));
    }
    let generation =
        i64::try_from(intent.generation()).map_err(|_| GoalDbError::BindingConflict)?;
    let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages m
        JOIN conversations c ON c.id=m.conversation_id
        JOIN product_conversations p ON p.id=c.product_conversation_id
        JOIN durable_turns t ON t.conversation_id=c.id
        JOIN messages u ON u.message_id=t.canonical_message_id AND u.conversation_id=c.id AND u.message_type='user'
        WHERE m.message_id=? AND m.conversation_id=? AND m.message_type='agent'
          AND c.product_conversation_id=? AND c.runtime_role='coordinator' AND p.kind='coordinator'
          AND t.turn_id=? AND t.workflow_id=?
          AND (?=0 OR (c.coordinator_head=1 AND t.generation=? AND t.disposition='Runtime' AND t.owns_conversation=1 AND t.terminal_kind IS NULL)))")
        .bind(source.message_id.as_str()).bind(source.transcript_id.as_str())
        .bind(source.product_conversation_id.as_str()).bind(execution.turn_id())
        .bind(execution.workflow_id()).bind(i64::from(live)).bind(generation)
        .fetch_one(&mut **tx).await?;
    if !found {
        return Err(GoalDbError::BindingConflict);
    }
    let MessageContent::Agent(blocks) = &assistant.content else {
        return Err(GoalDbError::Integrity("management assistant content"));
    };
    let actual = blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse { id, name, input }
                if id == source.tool_use_id.as_str() && name == "goal_manage" =>
            {
                Some(input)
            }
            ContentBlock::Text { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ToolUse { .. }
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
        .ok_or(GoalDbError::Integrity(
            "management canonical tool call missing",
        ))?;
    let actual: GoalManagementRequest = serde_json::from_value(actual.clone())
        .map_err(|_| GoalDbError::Integrity("management canonical arguments"))?;
    if &actual != intent.request() {
        return Err(GoalDbError::Integrity("management arguments changed"));
    }
    Ok(())
}

pub(super) async fn finalize_tx(
    tx: &mut GoalTx<'_>,
    checkpoint: &super::round::GoalRoundCheckpoint<'_>,
    intent: &GoalPendingManagement,
    duration: u64,
    targets: &[GoalManagementTarget],
    installation: super::control::GoalControlInstallation,
) -> Result<ToolResult> {
    let assistant = checkpoint.assistant;
    let execution = checkpoint.execution;
    let at = GoalTimestampUs::parse(checkpoint.state_updated_at.timestamp_micros())?;
    verify_source_tx(tx, assistant, execution, intent, false).await?;
    let mutation = intent.mutation();
    if let Some(entry) = super::retry_tx(tx, &mutation).await? {
        return accepted_result(intent.source(), &entry, duration);
    }
    verify_source_tx(tx, assistant, execution, intent, true).await?;
    let target = targets
        .iter()
        .find(|target| target.owner() == &mutation.owner)
        .ok_or(GoalDbError::Integrity(
            "management target admission missing",
        ))?;
    let GoalManagementTarget::Admitted { transcript, .. } = target else {
        return Ok(refused_result(intent.source(), duration));
    };
    let mut savepoint = sqlx::Acquire::begin(&mut **tx).await?;
    match super::apply_goal_tx(
        &mut savepoint,
        &mutation,
        Some(transcript),
        at,
        installation,
    )
    .await
    {
        Ok(entry) => {
            crate::coordinator_watches::goal_facts::settled_change_tx(
                &mut savepoint,
                installation,
                &entry,
                transcript,
            )
            .await?;
            let result = accepted_result(intent.source(), &entry, duration)?;
            savepoint.commit().await?;
            Ok(result)
        }
        Err(error) if semantic_refusal(&error) => {
            savepoint.rollback().await?;
            Ok(refused_result(intent.source(), duration))
        }
        Err(error) => Err(error),
    }
}

fn semantic_refusal(error: &GoalDbError) -> bool {
    matches!(
        error,
        GoalDbError::Domain(_)
            | GoalDbError::Busy
            | GoalDbError::TargetChanged
            | GoalDbError::Admission(
                crate::DbError::ConversationNotFound(_)
                    | crate::DbError::CloseAdmissionFenced(_)
                    | crate::DbError::ProductConversationUnavailable(_)
            )
    )
}

fn accepted_result(
    source: &GoalAgentSource,
    entry: &super::GoalHistoryEntry,
    duration: u64,
) -> Result<ToolResult> {
    let output = serde_json::to_string(&entry.snapshot)
        .map_err(|_| GoalDbError::Integrity("management accepted output"))?;
    let mut result = ToolResult::success(source.tool_use_id.as_str().into(), output);
    result.duration_ms = Some(duration);
    Ok(result)
}

fn refused_result(source: &GoalAgentSource, duration: u64) -> ToolResult {
    let mut result = ToolResult::error(
        source.tool_use_id.as_str().into(),
        "Goal management refused: target unavailable, busy, or version/state conflict".into(),
    );
    result.duration_ms = Some(duration);
    result
}

pub(super) async fn retained_result_tx(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    execution: GoalExecution,
    intent: &GoalPendingManagement,
    actual: &Message,
    duration: u64,
) -> Result<ToolResult> {
    verify_source_tx(tx, assistant, execution, intent, false).await?;
    if let Some(entry) = super::retry_tx(tx, &intent.mutation()).await? {
        return accepted_result(intent.source(), &entry, duration);
    }
    let refused = refused_result(intent.source(), duration);
    if !crate::message_payload_matches(
        actual,
        &super::round::result_message(
            assistant,
            usize::try_from(actual.sequence_id - assistant.sequence_id - 1)
                .map_err(|_| GoalDbError::Integrity("management retained ordinal"))?,
            &refused,
        )?,
    ) {
        return Err(GoalDbError::Integrity(
            "management retained refusal differs",
        ));
    }
    Ok(refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_goal_management_refuses_only_typed_target_admission_failures() {
        let owner = ProductConversationId::parse("target").unwrap();
        assert!(semantic_refusal(&GoalDbError::Admission(
            crate::DbError::ProductConversationUnavailable(owner)
        )));
        assert!(!semantic_refusal(&GoalDbError::BindingConflict));
        assert!(!semantic_refusal(&GoalDbError::Integrity("source differs")));
        assert!(!semantic_refusal(&GoalDbError::Admission(
            crate::DbError::Serialization("invalid state".into())
        )));
    }
}
