use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAgentSource, GoalAttribution, GoalChange, GoalExecution, GoalId, GoalMutation,
    GoalReportCall, GoalText, GoalTimestampUs, GoalVersion, PendingSelfGoal, SelfGoalOperation,
};
use phoenix_core::domain::db_schema::{Message, MessageContent, ToolOutcome, ToolResult};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;
use sqlx::Row;

use super::{GoalDbError, GoalTx, Result};
use crate::Database;

impl Database {
    /// Resolve identity from the active server turn, never from tool arguments.
    pub async fn prepare_self_goal_tool(
        &self,
        transcript: &str,
        turn_id: i64,
        generation: i64,
        message_id: &str,
        tool_use_id: &str,
        operation: SelfGoalOperation,
    ) -> Result<PendingSelfGoal> {
        let row = sqlx::query(
            "SELECT t.workflow_id,c.product_conversation_id FROM durable_turns t
            JOIN conversations c ON c.id=t.conversation_id WHERE t.turn_id=? AND c.id=?",
        )
        .bind(turn_id)
        .bind(transcript)
        .fetch_optional(self.pool())
        .await?
        .ok_or(GoalDbError::Integrity("self-goal runtime turn identity"))?;
        Ok(PendingSelfGoal {
            owner: ProductConversationId::parse(
                row.try_get::<String, _>("product_conversation_id")?,
            )
            .map_err(|_| GoalDbError::Integrity("self-goal owner identity"))?,
            execution: GoalExecution::parse(row.try_get("workflow_id")?, turn_id)?,
            generation,
            call: GoalReportCall {
                transcript_id: GoalText::parse(transcript)?,
                message_id: GoalText::parse(message_id)?,
                tool_use_id: GoalText::parse(tool_use_id)?,
            },
            operation,
        })
    }
}

pub(super) async fn verify_source(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    execution: GoalExecution,
    pending: &PendingSelfGoal,
) -> Result<()> {
    if pending.execution != execution
        || pending.call.transcript_id.as_str() != assistant.conversation_id
        || pending.call.message_id.as_str() != assistant.message_id
    {
        return Err(GoalDbError::Integrity(
            "self-goal checkpoint source identity",
        ));
    }
    let expected = match &assistant.content {
        MessageContent::Agent(blocks) => blocks,
        _ => return Err(GoalDbError::Integrity("self-goal assistant blocks")),
    };
    let content: Option<String> = sqlx::query_scalar(
        "SELECT m.content FROM messages m
        JOIN conversations c ON c.id=m.conversation_id
        JOIN durable_turns t ON t.conversation_id=c.id
        WHERE m.conversation_id=? AND m.message_id=? AND m.message_type='agent'
        AND c.product_conversation_id=? AND t.turn_id=? AND t.workflow_id=?",
    )
    .bind(pending.call.transcript_id.as_str())
    .bind(pending.call.message_id.as_str())
    .bind(pending.owner.as_str())
    .bind(execution.turn_id())
    .bind(execution.workflow_id())
    .fetch_optional(&mut **tx)
    .await?;
    let blocks: Vec<ContentBlock> =
        serde_json::from_str(&content.ok_or(GoalDbError::Integrity("self-goal persisted source"))?)
            .map_err(|_| GoalDbError::Integrity("self-goal source content"))?;
    if &blocks != expected {
        return Err(GoalDbError::Integrity("self-goal persisted source differs"));
    }
    let calls: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input }
                if id == pending.call.tool_use_id.as_str() =>
            {
                Some((name, input))
            }
            _ => None,
        })
        .collect();
    let [(name, input)] = calls.as_slice() else {
        return Err(GoalDbError::Integrity("self-goal unique source call"));
    };
    if SelfGoalOperation::parse(name, (*input).clone())
        .ok()
        .as_ref()
        != Some(&pending.operation)
    {
        return Err(GoalDbError::Integrity("self-goal source name or arguments"));
    }
    Ok(())
}

pub(super) async fn finalize(
    tx: &mut GoalTx<'_>,
    pending: &PendingSelfGoal,
    duration_ms: Option<u64>,
    policies: super::execution::GoalWritePolicies,
) -> Result<(ToolResult, Option<super::GoalVersionCommittedNotification>)> {
    let row = sqlx::query("SELECT p.kind,p.ordinary_lifecycle,p.auto_continue_on_context_exhaustion AS enabled,
        p.auto_continue_disable_epoch AS epoch,c.state,c.continued_in_conv_id,
        t.disposition,t.generation,w.generation AS workflow_generation,w.status AS workflow_status,
        a.disable_epoch AS admitted_epoch,a.generation AS admitted_generation,
        a.owner_id AS admitted_owner,a.transcript_id AS admitted_transcript
        FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id
        JOIN workflows w ON w.workflow_id=t.workflow_id
        JOIN product_conversations p ON p.id=c.product_conversation_id
        LEFT JOIN ordinary_self_goal_admissions a ON a.turn_id=t.turn_id AND a.workflow_id=t.workflow_id
        WHERE t.turn_id=? AND t.workflow_id=? AND p.id=? AND c.id=?")
        .bind(pending.execution.turn_id()).bind(pending.execution.workflow_id())
        .bind(pending.owner.as_str()).bind(pending.call.transcript_id.as_str())
        .fetch_optional(&mut **tx).await?
        .ok_or(GoalDbError::Integrity("self-goal execution owner"))?;
    let state: ConvState = serde_json::from_str(&row.try_get::<String, _>("state")?)
        .map_err(|_| GoalDbError::Integrity("self-goal persisted state"))?;
    let pending_close: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_obligations
        WHERE product_conversation_id=? AND phase<>'completed')",
    )
    .bind(pending.owner.as_str())
    .fetch_one(&mut **tx)
    .await?;
    let control_current =
        match super::control::require_current_control_tx(tx, &pending.owner, pending.execution)
            .await
        {
            Ok(()) => true,
            Err(super::control::ControlError::AdmissionStopped) => false,
            Err(super::control::ControlError::Sqlx(error)) => return Err(GoalDbError::Sqlx(error)),
            Err(_) => return Err(GoalDbError::Integrity("self-goal control authority")),
        };
    let refusal = if row.try_get::<String, _>("kind")? != "ordinary" {
        Some("ordinary_role_required")
    } else if row.try_get::<String, _>("ordinary_lifecycle")? != "open" || pending_close {
        Some("conversation_closed_or_closing")
    } else if !control_current || !matches!(state, ConvState::ToolExecuting { .. }) {
        Some("conversation_stopped_or_paused")
    } else if row
        .try_get::<Option<String>, _>("continued_in_conv_id")?
        .is_some()
    {
        Some("transcript_changed")
    } else if row.try_get::<String, _>("disposition")? != "Runtime"
        || row.try_get::<i64, _>("generation")? != pending.generation
        || row.try_get::<i64, _>("workflow_generation")? != pending.generation
        || row.try_get::<String, _>("workflow_status")? != "Active"
        || row.try_get::<Option<i64>, _>("admitted_generation")? != Some(pending.generation)
        || row
            .try_get::<Option<String>, _>("admitted_owner")?
            .as_deref()
            != Some(pending.owner.as_str())
        || row
            .try_get::<Option<String>, _>("admitted_transcript")?
            .as_deref()
            != Some(pending.call.transcript_id.as_str())
    {
        Some("execution_not_admitted")
    } else if row.try_get::<Option<i64>, _>("admitted_epoch")?
        != Some(row.try_get::<i64, _>("epoch")?)
    {
        Some("human_disable_since_admission")
    } else {
        None
    };
    let mut notification = None;
    let output = if let Some(reason) = refusal {
        Err(reason)
    } else {
        match &pending.operation {
            SelfGoalOperation::EnableAutoContinuation => {
                crate::automatic_continuation_settings::write_preference_on(
                    &mut **tx,
                    &pending.owner,
                    phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion::Enabled,
                ).await?;
                Ok("auto_continuation_enabled")
            }
            SelfGoalOperation::Create { objective } => {
                if !row.try_get::<bool, _>("enabled")? {
                    Err("auto_continuation_disabled")
                } else if super::current_tx(tx, &pending.owner).await?.is_some() {
                    Err("goal_already_exists")
                } else {
                    let actor = GoalActor::OwningAgent(GoalAgentSource {
                        product_conversation_id: pending.owner.clone(),
                        transcript_id: pending.call.transcript_id.clone(),
                        execution: pending.execution,
                        message_id: pending.call.message_id.clone(),
                        tool_use_id: pending.call.tool_use_id.clone(),
                    });
                    let mutation = GoalMutation {
                        owner: pending.owner.clone(),
                        expected_version: GoalVersion::INITIAL,
                        action_id: GoalText::parse(format!(
                            "self-goal:{}:{}",
                            pending.call.transcript_id.as_str(),
                            pending.call.tool_use_id.as_str()
                        ))?,
                        change: GoalChange::Create {
                            goal_id: GoalId::parse(uuid::Uuid::new_v4().to_string())?,
                            objective: objective.clone(),
                        },
                        attribution: GoalAttribution {
                            requested_by: actor.clone(),
                            applied_by: actor,
                        },
                    };
                    let now = GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())?;
                    let applied = super::apply_goal_tx(tx, &mutation, None, now, policies).await?;
                    notification = Some(super::GoalVersionCommittedNotification::new(
                        applied.snapshot().owner().clone(),
                        applied.snapshot().version(),
                    ));
                    Ok("goal_created")
                }
            }
        }
    };
    Ok((canonical_result(pending, output, duration_ms), notification))
}

fn canonical_result(
    pending: &PendingSelfGoal,
    output: std::result::Result<&str, &str>,
    duration_ms: Option<u64>,
) -> ToolResult {
    let outcome = match output {
        Ok(status) => ToolOutcome::Success {
            output: serde_json::json!({"status":status}).to_string(),
            display_data: None,
            images: vec![],
        },
        Err(reason) => ToolOutcome::Error {
            output: serde_json::json!({"status":"refused","reason":reason}).to_string(),
            display_data: None,
            images: vec![],
        },
    };
    ToolResult {
        tool_use_id: pending.call.tool_use_id.as_str().to_owned(),
        outcome,
        duration_ms,
    }
}

pub(super) async fn retained_result_tx(
    _tx: &mut GoalTx<'_>,
    pending: &PendingSelfGoal,
    duration_ms: Option<u64>,
    actual: &Message,
) -> Result<ToolResult> {
    let MessageContent::Tool(content) = &actual.content else {
        return Err(GoalDbError::Integrity("retained self-goal result content"));
    };
    let value: serde_json::Value = serde_json::from_str(&content.content)
        .map_err(|_| GoalDbError::Integrity("retained self-goal result JSON"))?;
    let output = match value.get("status").and_then(serde_json::Value::as_str) {
        Some("goal_created") if matches!(&pending.operation, SelfGoalOperation::Create { .. }) => {
            Ok("goal_created")
        }
        Some("auto_continuation_enabled")
            if matches!(
                &pending.operation,
                SelfGoalOperation::EnableAutoContinuation
            ) =>
        {
            Ok("auto_continuation_enabled")
        }
        Some("refused") => Err(value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .ok_or(GoalDbError::Integrity("retained self-goal refusal reason"))?),
        _ => return Err(GoalDbError::Integrity("retained self-goal result status")),
    };
    Ok(canonical_result(pending, output, duration_ms))
}
