use sqlx::Acquire;

use phoenix_core::domain::conversation_goal::{
    GoalExecution, GoalPendingReport, GoalReportOutcome,
};
use phoenix_core::domain::db_schema::{
    Message, MessageContent, MessageType, ToolContent, ToolResult,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::tool_result_identity::tool_result_message_id;
use sqlx::Row;

use super::{
    GoalChange, GoalDbError, GoalError, GoalHistoryEntry, GoalState, GoalTimestampUs, GoalTx,
    Result, apply_goal_tx, binding_tx, retry_tx,
};

pub(super) async fn verify_source_tx(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    execution: GoalExecution,
    report: &GoalPendingReport,
) -> Result<()> {
    let mutation = report.mutation();
    let GoalChange::Report { binding, .. } = &mutation.change else {
        return Err(GoalDbError::Integrity("pending report command"));
    };
    if assistant.message_id != report.call().message_id.as_str()
        || assistant.conversation_id != report.call().transcript_id.as_str()
        || assistant.message_type != MessageType::Agent
        || binding.execution() != execution
    {
        return Err(GoalDbError::Integrity("pending report owning round"));
    }
    let MessageContent::Agent(expected_blocks) = &assistant.content else {
        return Err(GoalDbError::Integrity(
            "pending report owning assistant content",
        ));
    };
    if binding_tx(tx, binding.execution().turn_id())
        .await?
        .as_ref()
        != Some(binding)
    {
        return Err(GoalDbError::Integrity("pending report admitted binding"));
    }
    let content: Option<String> = sqlx::query_scalar(
        "SELECT m.content FROM messages m
         JOIN durable_turns t ON t.conversation_id=m.conversation_id
         JOIN conversations c ON c.id=m.conversation_id
         JOIN product_conversations p ON p.id=c.product_conversation_id
         WHERE m.conversation_id=? AND m.message_id=? AND m.message_type='agent'
           AND t.turn_id=? AND t.workflow_id=? AND p.id=? AND p.kind='ordinary'",
    )
    .bind(report.call().transcript_id.as_str())
    .bind(report.call().message_id.as_str())
    .bind(binding.execution().turn_id())
    .bind(binding.execution().workflow_id())
    .bind(binding.owner().as_str())
    .fetch_optional(&mut **tx)
    .await?;
    let blocks: Vec<ContentBlock> = serde_json::from_str(&content.ok_or(
        GoalDbError::Integrity("pending report genuine assistant source"),
    )?)
    .map_err(|_| GoalDbError::Integrity("pending report assistant content"))?;
    if &blocks != expected_blocks {
        return Err(GoalDbError::Integrity(
            "pending report persisted assistant differs",
        ));
    }
    let mut calls = blocks.iter().filter_map(|block| match block {
        ContentBlock::ToolUse { id, name, input } if id == report.call().tool_use_id.as_str() => {
            Some((name, input))
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
    });
    let Some((name, input)) = calls.next() else {
        return Err(GoalDbError::Integrity("pending report actual tool call"));
    };
    if calls.next().is_some() || name != "goal_report" {
        return Err(GoalDbError::Integrity("pending report unique tool call"));
    }
    let actual: GoalReportOutcome = serde_json::from_value(input.clone())
        .map_err(|_| GoalDbError::Integrity("pending report actual arguments"))?;
    if &actual != report.outcome() {
        return Err(GoalDbError::Integrity("pending report changed arguments"));
    }
    Ok(())
}

fn semantic_refusal(error: &GoalDbError) -> bool {
    matches!(
        error,
        GoalDbError::Domain(
            GoalError::Conflict | GoalError::Inapplicable | GoalError::ReportBindingRequired
        ) | GoalDbError::BindingConflict
            | GoalDbError::Busy
            | GoalDbError::TargetChanged
            | GoalDbError::Control(super::control::ControlError::AdmissionStopped)
    )
}

fn accepted_output(entry: &GoalHistoryEntry) -> Result<String> {
    let status = match entry.snapshot().state() {
        GoalState::Blocked(_) => "blocked",
        GoalState::Complete(_) => "complete",
        GoalState::Exhausted(_) => "exhausted",
        GoalState::Underway | GoalState::Suspended(_) => {
            return Err(GoalDbError::Integrity(
                "pending report accepted non-report state",
            ));
        }
    };
    Ok(format!(
        "Goal {} recorded {status} at owner version {}.",
        entry.snapshot().goal_id().as_str(),
        entry.snapshot().version().get()
    ))
}

pub(super) async fn retained_result_tx(
    tx: &mut GoalTx<'_>,
    report: &GoalPendingReport,
    result_message_id: &str,
) -> Result<Option<ToolResult>> {
    let row = sqlx::query(
        "SELECT message_type,content,display_data FROM messages WHERE conversation_id=? AND message_id=?",
    )
    .bind(report.call().transcript_id.as_str())
    .bind(result_message_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("message_type")? != "tool" {
        return Err(GoalDbError::Integrity(
            "pending report retained result type",
        ));
    }
    let ToolContent {
        tool_use_id,
        content,
        is_error,
        images,
    } = serde_json::from_str(&row.try_get::<String, _>("content")?)
        .map_err(|_| GoalDbError::Integrity("pending report retained result content"))?;
    if tool_use_id != report.call().tool_use_id.as_str() || !images.is_empty() {
        return Err(GoalDbError::Integrity(
            "pending report retained result identity",
        ));
    }
    let accepted = retry_tx(tx, &report.mutation()).await?;
    if is_error == accepted.is_some() {
        return Err(GoalDbError::Integrity(
            "pending report retained result history",
        ));
    }
    if let Some(entry) = accepted {
        if content != accepted_output(&entry)? {
            return Err(GoalDbError::Integrity(
                "pending report retained success differs",
            ));
        }
    }
    let display: Option<String> = row.try_get("display_data")?;
    let duration_ms = match display {
        None => None,
        Some(display) => {
            let display: serde_json::Value = serde_json::from_str(&display)
                .map_err(|_| GoalDbError::Integrity("pending report retained display data"))?;
            display
                .get("duration_ms")
                .map(|duration| {
                    duration
                        .as_u64()
                        .ok_or(GoalDbError::Integrity("pending report retained duration"))
                })
                .transpose()?
        }
    };
    let mut result = if is_error {
        ToolResult::error(tool_use_id, content)
    } else {
        ToolResult::success(tool_use_id, content)
    };
    result.duration_ms = duration_ms;
    Ok(Some(result))
}

pub(crate) async fn finalize_report_tx(
    tx: &mut GoalTx<'_>,
    assistant: &Message,
    execution: GoalExecution,
    report: &GoalPendingReport,
    duration_ms: u64,
    at: GoalTimestampUs,
    policies: super::execution::GoalWritePolicies,
) -> Result<ToolResult> {
    verify_source_tx(tx, assistant, execution, report).await?;
    let result_message_id =
        tool_result_message_id(&assistant.message_id, report.call().tool_use_id.as_str());
    if let Some(retained) = retained_result_tx(tx, report, &result_message_id).await? {
        return Ok(retained);
    }
    let mut report_tx = tx.begin().await?;
    let mutation = report.mutation();
    let outcome = apply_goal_tx(
        &mut report_tx,
        &mutation,
        Some(report.call().transcript_id.as_str()),
        at,
        policies,
    )
    .await;
    let mut result = match outcome {
        Ok(entry) => {
            let output = accepted_output(&entry)?;
            report_tx.commit().await?;
            ToolResult::success(report.call().tool_use_id.as_str().into(), output)
        }
        Err(error) if semantic_refusal(&error) => {
            report_tx.rollback().await?;
            ToolResult::error(
                report.call().tool_use_id.as_str().into(),
                format!("Goal report rejected: {error}"),
            )
        }
        Err(error) => return Err(error),
    };
    result.duration_ms = Some(duration_ms);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{GoalDbError, GoalError, semantic_refusal};

    #[test]
    fn only_typed_semantic_refusals_become_report_errors() {
        for error in [
            GoalDbError::Domain(GoalError::Conflict),
            GoalDbError::Domain(GoalError::Inapplicable),
            GoalDbError::Domain(GoalError::ReportBindingRequired),
            GoalDbError::BindingConflict,
            GoalDbError::Busy,
            GoalDbError::TargetChanged,
            GoalDbError::Control(super::super::control::ControlError::AdmissionStopped),
        ] {
            assert!(semantic_refusal(&error));
        }
        for error in [
            GoalDbError::Integrity("source differs"),
            GoalDbError::RetryConflict,
            GoalDbError::Domain(GoalError::Blank),
            GoalDbError::Domain(GoalError::Range),
            GoalDbError::Sqlx(sqlx::Error::RowNotFound),
            GoalDbError::Control(super::super::control::ControlError::Storage(
                sqlx::Error::RowNotFound,
            )),
        ] {
            assert!(!semantic_refusal(&error));
        }
    }
}
