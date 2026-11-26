use super::{NativeCloseCommand, NativeCloseIntent};
use crate::close_foundation::CloseCleanupFailureAuthority;
use crate::{ConvState, Database, DbError, DbResult};
use phoenix_core::domain::close::{CloseAttemptId, CloseObligation, CloseRunOrdinal, CloseRunRef};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{Row, Sqlite, Transaction};

#[derive(Clone, Copy)]
pub(crate) enum NativeSourceLossCause {
    UserSourceLoss,
    ProcessInterrupted,
}

pub(crate) struct NativeInitialSourceFence {
    run: CloseRunRef,
    cause: NativeSourceLossCause,
}

impl NativeInitialSourceFence {
    #[cfg(test)]
    pub(crate) fn process_interrupted(run: CloseRunRef) -> Self {
        Self {
            run,
            cause: NativeSourceLossCause::ProcessInterrupted,
        }
    }

    pub(crate) fn into_parts(self) -> (CloseRunRef, CloseCleanupFailureAuthority, &'static str) {
        let (authority, detail) = match self.cause {
            NativeSourceLossCause::UserSourceLoss => (
                CloseCleanupFailureAuthority::ExplicitlySuppressed,
                "source_fenced: initial native Close source stopped or replaced",
            ),
            NativeSourceLossCause::ProcessInterrupted => (
                CloseCleanupFailureAuthority::AttemptInterrupted,
                "process_interrupted: initial native Close admission stopped before effects",
            ),
        };
        (self.run, authority, detail)
    }
}

pub(crate) struct NativeRetrySourceFence {
    run: CloseRunRef,
    cause: NativeSourceLossCause,
}

impl NativeRetrySourceFence {
    pub(crate) fn into_parts(self) -> (CloseRunRef, CloseCleanupFailureAuthority, &'static str) {
        let (authority, detail) = match self.cause {
            NativeSourceLossCause::UserSourceLoss => (
                CloseCleanupFailureAuthority::ExplicitlySuppressed,
                "source_fenced: exact native retry source stopped or replaced",
            ),
            NativeSourceLossCause::ProcessInterrupted => (
                CloseCleanupFailureAuthority::AttemptInterrupted,
                "process_interrupted: retained native retry plan stopped without dispatch",
            ),
        };
        (self.run, authority, detail)
    }
}

/// Suppresses existing allowances on exact native source loss and stops running successors.
pub(crate) async fn fence_native_retry_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
) -> DbResult<()> {
    fence_native_sources_tx(
        tx,
        transcript_id,
        next_state,
        NativeSourceLossCause::UserSourceLoss,
    )
    .await
}

/// Startup preserves both allowance bits while stopping admitted native resource retries.
pub(crate) async fn recover_native_retry_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
) -> DbResult<()> {
    fence_native_sources_tx(
        tx,
        transcript_id,
        next_state,
        NativeSourceLossCause::ProcessInterrupted,
    )
    .await
}

/// Returns None for runs outside the native explicit-retry protocol.
pub(crate) async fn interrupt_native_retry_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<Option<CloseObligation>> {
    let automatic: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM native_close_completion_retry_successors
         WHERE attempt_id = ?1 AND successor_run_ordinal = ?2 AND successor_status = 'running')",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_one(&mut **tx)
    .await?;
    if automatic {
        revoke_completion_origin_tx(tx, run.attempt_id.as_str()).await?;
        return Database::terminalize_native_retry_source_fence_tx(
            tx,
            NativeRetrySourceFence {
                run: run.clone(),
                cause: NativeSourceLossCause::ProcessInterrupted,
            },
        )
        .await
        .map(Some);
    }

    let native_plan: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM native_close_retry_plans
         WHERE attempt_id = ?1 AND run_ordinal = ?2)",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_one(&mut **tx)
    .await?;
    if !native_plan {
        let native_initial: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM native_close_initial_receipts receipt
             JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = receipt.run_ordinal
             JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id
             WHERE receipt.attempt_id = ?1 AND receipt.run_ordinal = 1 AND receipt.intent = 'initial'
               AND receipt.product_conversation_id = obligation.product_conversation_id
               AND obligation.native_managed = 1 AND run.status = 'running')",
        )
        .bind(run.attempt_id.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !native_initial || run.ordinal != CloseRunOrdinal::INITIAL {
            return Ok(None);
        }
        revoke_completion_origin_tx(tx, run.attempt_id.as_str()).await?;
        let obligation = Database::terminalize_native_initial_source_fence_tx(
            tx,
            NativeInitialSourceFence {
                run: run.clone(),
                cause: NativeSourceLossCause::ProcessInterrupted,
            },
        )
        .await?;
        return Ok(Some(obligation));
    }
    Database::terminalize_native_retry_source_fence_tx(
        tx,
        NativeRetrySourceFence {
            run: run.clone(),
            cause: NativeSourceLossCause::ProcessInterrupted,
        },
    )
    .await
    .map(Some)
}

async fn fence_native_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
    cause: NativeSourceLossCause,
) -> DbResult<()> {
    fence_completion_sources_tx(tx, transcript_id, next_state, cause).await?;
    fence_pre_completion_native_sources_tx(tx, transcript_id, next_state, cause).await
}

pub(crate) async fn fence_pre_completion_native_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
    cause: NativeSourceLossCause,
) -> DbResult<()> {
    let rows = sqlx::query(
        "SELECT receipt.attempt_id, receipt.run_ordinal, receipt.intent,
                plan.failed_run_ordinal, run.status,
                receipt.source_message_id, receipt.source_tool_use_id,
                receipt.product_conversation_id, receipt.request_fingerprint,
                EXISTS (SELECT 1 FROM native_close_initial_result_acknowledgements ack
                        WHERE ack.request_id = receipt.request_id
                          AND ack.request_fingerprint = receipt.request_fingerprint
                          AND ack.attempt_id = receipt.attempt_id AND ack.run_ordinal = 1) AS acknowledged
         FROM native_close_initial_receipts receipt
         LEFT JOIN native_close_retry_plans plan
           ON receipt.request_id = plan.request_id
          AND receipt.request_fingerprint = plan.request_fingerprint
          AND receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
         JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = receipt.run_ordinal
         WHERE receipt.source_transcript_id = ?1
           AND (receipt.intent = 'initial'
                OR (receipt.intent = 'explicit_retry' AND plan.request_id IS NOT NULL))",
    )
    .bind(transcript_id)
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut fences = Vec::new();
    for row in rows {
        let source = SourceToolCall {
            message_id: row.try_get("source_message_id")?,
            tool_use_id: row.try_get("source_tool_use_id")?,
        };
        let run = CloseRunRef {
            attempt_id: CloseAttemptId::parse(row.try_get::<String, _>("attempt_id")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            ordinal: CloseRunOrdinal::parse(row.try_get("run_ordinal")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        };
        let target =
            ProductConversationId::parse(row.try_get::<String, _>("product_conversation_id")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let explicit_retry = row.try_get::<String, _>("intent")? == "explicit_retry";
        let intent = if explicit_retry {
            NativeCloseIntent::ExplicitRetry {
                target,
                attempt_id: run.attempt_id.clone(),
                failed_run_ordinal: CloseRunOrdinal::parse(row.try_get("failed_run_ordinal")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            }
        } else {
            NativeCloseIntent::Initial { target }
        };
        let command = NativeCloseCommand::new_from_retained_fingerprint(
            transcript_id.to_owned(),
            source.clone(),
            intent,
            &row.try_get::<String, _>("request_fingerprint")?,
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        if command
            .verify_executing_source(next_state, &source)
            .is_err()
        {
            let acknowledged_completed_initial = !explicit_retry
                && row.try_get::<String, _>("status")? == "stopped"
                && (row.try_get::<bool, _>("acknowledged")?
                    || completed_initial_result_matches(
                        next_state,
                        &command,
                        run.attempt_id.as_str(),
                    ));
            if matches!(cause, NativeSourceLossCause::UserSourceLoss)
                && !acknowledged_completed_initial
            {
                sqlx::query(
                    "UPDATE close_completion_retry_allowances SET suppressed = 1
                     WHERE attempt_id = ?1 AND suppressed = 0",
                )
                .bind(run.attempt_id.as_str())
                .execute(&mut **tx)
                .await?;
            }
            if explicit_retry && row.try_get::<String, _>("status")? == "running" {
                fences.push(NativeRetrySourceFence { run, cause });
            } else if !explicit_retry && row.try_get::<String, _>("status")? == "running" {
                Database::terminalize_native_initial_source_fence_tx(
                    tx,
                    NativeInitialSourceFence { run, cause },
                )
                .await?;
            }
        }
    }
    for fence in fences {
        Database::terminalize_native_retry_source_fence_tx(tx, fence).await?;
    }
    Ok(())
}
// Serial tool progression retains the exact successful initial result in the
// same assistant round before its canonical checkpoint. This is not an
// acknowledgement and does not survive Stop/Idle or process interruption.
fn completed_initial_result_matches(
    state: &ConvState,
    command: &NativeCloseCommand,
    attempt_id: &str,
) -> bool {
    use phoenix_core::domain::conversation_goal::ToolRoundResult;
    use phoenix_core::domain::db_schema::ToolOutcome;
    use phoenix_core::domain::llm_types::ContentBlock;
    let ConvState::ToolExecuting {
        assistant_message,
        completed_results,
        ..
    } = state
    else {
        return false;
    };
    if assistant_message.message_id != command.source().message_id
        || !assistant_message.content.iter().any(|block| {
            matches!(block,
            ContentBlock::ToolUse { id, name, input } if id == &command.source().tool_use_id
                && name == "close_conversation" && command.matches_source_input(input))
        })
    {
        return false;
    }
    let request_id = command.request_id().to_string();
    completed_results.iter().any(|result| {
        let ToolRoundResult::Finalized(result) = result else {
            return false;
        };
        if result.tool_use_id != command.source().tool_use_id {
            return false;
        }
        let ToolOutcome::Success { output, .. } = &result.outcome else {
            return false;
        };
        let Ok(output) = serde_json::from_str::<serde_json::Value>(output) else {
            return false;
        };
        output.get("action").and_then(serde_json::Value::as_str) == Some("initial")
            && matches!(
                output.get("status").and_then(serde_json::Value::as_str),
                Some("newly_admitted" | "replay")
            )
            && output.get("request_id").and_then(serde_json::Value::as_str)
                == Some(request_id.as_str())
            && output
                .pointer("/admitted/product_conversation_id")
                .and_then(serde_json::Value::as_str)
                == Some(command.intent().target().as_str())
            && output
                .pointer("/admitted/attempt_id")
                .and_then(serde_json::Value::as_str)
                == Some(attempt_id)
            && output
                .pointer("/admitted/run_ordinal")
                .and_then(serde_json::Value::as_i64)
                == Some(1)
    })
}

/// Consume only canonical rows returned by the round writer in this transaction.
/// Both finalized and pending self-goal rounds can contain a finalized Close result.
/// A source transition alone never acknowledges a result or revives a stopped source.
#[allow(
    clippy::too_many_lines,
    reason = "Keep canonical receipt replay, source eligibility, and acknowledgement in order in the round transaction"
)]
pub(crate) async fn acknowledge_initial_result_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    assistant: &crate::Message,
    results: &[crate::Message],
) -> DbResult<()> {
    use phoenix_core::domain::db_schema::MessageContent;
    use phoenix_core::domain::llm_types::ContentBlock;

    let MessageContent::Agent(blocks) = &assistant.content else {
        return Ok(());
    };
    let state_json: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
        .bind(transcript_id)
        .fetch_one(&mut **tx)
        .await?;
    let current_state: ConvState = serde_json::from_str(&state_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    for row in results {
        let MessageContent::Tool(tool_result) = &row.content else {
            continue;
        };
        if row.conversation_id != transcript_id
            || row.message_id
                != phoenix_core::domain::tool_result_identity::tool_result_message_id(
                    &assistant.message_id,
                    &tool_result.tool_use_id,
                )
        {
            continue;
        }
        if tool_result.is_error {
            continue;
        }
        let Ok(output) = serde_json::from_str::<serde_json::Value>(&tool_result.content) else {
            continue;
        };
        let Some((name, input)) = blocks.iter().find_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } if id == &tool_result.tool_use_id => {
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
        }) else {
            continue;
        };
        if name != "close_conversation" {
            continue;
        }
        let receipts = sqlx::query(
            "SELECT request_id, attempt_id, source_message_id, source_tool_use_id,
                    request_fingerprint, product_conversation_id
             FROM native_close_initial_receipts
             WHERE source_transcript_id = ?1 AND source_message_id = ?2
               AND source_tool_use_id = ?3 AND intent = 'initial' AND run_ordinal = 1",
        )
        .bind(transcript_id)
        .bind(&assistant.message_id)
        .bind(&tool_result.tool_use_id)
        .fetch_all(&mut **tx)
        .await?;
        for receipt in receipts {
            let target = ProductConversationId::parse(
                receipt.try_get::<String, _>("product_conversation_id")?,
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?;
            let source = SourceToolCall {
                message_id: assistant.message_id.clone(),
                tool_use_id: tool_result.tool_use_id.clone(),
            };
            let command = NativeCloseCommand::new_from_retained_fingerprint(
                transcript_id.to_owned(),
                source.clone(),
                NativeCloseIntent::Initial {
                    target: target.clone(),
                },
                &receipt.try_get::<String, _>("request_fingerprint")?,
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?;
            let request_id: String = receipt.try_get("request_id")?;
            let attempt_id: String = receipt.try_get("attempt_id")?;
            let fingerprint: String = receipt.try_get("request_fingerprint")?;
            let prior: Option<(String, String)> = sqlx::query_as(
                "SELECT assistant_message_id, result_message_id FROM native_close_initial_result_acknowledgements
                 WHERE request_id = ?1 AND request_fingerprint = ?2 AND attempt_id = ?3 AND run_ordinal = 1",
            )
            .bind(&request_id).bind(&fingerprint).bind(&attempt_id)
            .fetch_optional(&mut **tx).await?;
            if prior
                .as_ref()
                .is_some_and(|(prior_assistant, prior_result)| {
                    prior_assistant == &assistant.message_id && prior_result == &row.message_id
                })
            {
                continue;
            }
            if prior.is_some()
                || command.fingerprint() != fingerprint
                || !command.matches_source_input(input)
                || output.get("action").and_then(serde_json::Value::as_str) != Some("initial")
                || !matches!(
                    output.get("status").and_then(serde_json::Value::as_str),
                    Some("newly_admitted" | "replay")
                )
                || output.get("request_id").and_then(serde_json::Value::as_str)
                    != Some(request_id.as_str())
                || output
                    .pointer("/admitted/product_conversation_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(target.as_str())
                || output
                    .pointer("/admitted/attempt_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(attempt_id.as_str())
                || output
                    .pointer("/admitted/run_ordinal")
                    .and_then(serde_json::Value::as_i64)
                    != Some(1)
                || (command
                    .verify_executing_source(&current_state, &source)
                    .is_err()
                    && !completed_initial_result_matches(&current_state, &command, &attempt_id))
            {
                continue;
            }
            let eligible: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM native_close_initial_receipts receipt
                    JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = 1
                    JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id
                    JOIN product_conversations product ON product.id = obligation.product_conversation_id
                    JOIN close_completion_retry_allowances allowance ON allowance.attempt_id = receipt.attempt_id
                    WHERE receipt.request_id = ?1 AND receipt.request_fingerprint = ?2
                      AND receipt.attempt_id = ?3 AND receipt.run_ordinal = 1
                      AND receipt.intent = 'initial' AND receipt.source_transcript_id = ?4
                      AND receipt.source_message_id = ?5 AND receipt.source_tool_use_id = ?6
                      AND receipt.product_conversation_id = ?7
                      AND run.status = 'stopped'
                      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
                      AND obligation.native_managed = 1 AND obligation.phase = 'completed'
                      AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open'
                      AND allowance.suppressed = 0
                      AND EXISTS (SELECT 1 FROM messages assistant_message
                          WHERE assistant_message.message_id = ?5 AND assistant_message.conversation_id = ?4)
                      AND EXISTS (SELECT 1 FROM messages result_message
                          WHERE result_message.message_id = ?8 AND result_message.conversation_id = ?4)
                      AND EXISTS (SELECT 1 FROM conversations source WHERE source.id = ?4
                          AND source.runtime_role = 'coordinator' AND source.coordinator_head = 1)
                      AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = receipt.attempt_id)
                      AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = receipt.attempt_id)
                )",
            )
            .bind(&request_id)
            .bind(&fingerprint)
            .bind(&attempt_id)
            .bind(transcript_id)
            .bind(&assistant.message_id)
            .bind(&tool_result.tool_use_id)
            .bind(target.as_str())
            .bind(&row.message_id)
            .fetch_one(&mut **tx)
            .await?;
            if !eligible {
                continue;
            }
            sqlx::query(
                "INSERT INTO native_close_initial_result_acknowledgements
                 (request_id, request_fingerprint, attempt_id, run_ordinal, assistant_message_id, result_message_id)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            )
            .bind(&request_id)
            .bind(&fingerprint)
            .bind(&attempt_id)
            .bind(&assistant.message_id)
            .bind(&row.message_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

async fn revoke_completion_origin_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> DbResult<()> {
    sqlx::query("INSERT INTO native_close_completion_retry_source_revocations (attempt_id, revoked_at_us)
        SELECT attempt_id, ?2 FROM native_close_completion_retry_origins origin
        WHERE origin.attempt_id = ?1 AND NOT EXISTS (
            SELECT 1 FROM native_close_completion_retry_source_revocations revoked WHERE revoked.attempt_id = origin.attempt_id)")
        .bind(attempt_id).bind(chrono::Utc::now().timestamp_micros()).execute(&mut **tx).await?;
    Ok(())
}

async fn fence_completion_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
    cause: NativeSourceLossCause,
) -> DbResult<()> {
    let origins = sqlx::query("SELECT origin.attempt_id, origin.product_conversation_id,
        origin.source_message_id, origin.source_tool_use_id, origin.origin_request_fingerprint,
        successor.run_ordinal AS successor_run_ordinal, successor.status
        FROM native_close_completion_retry_origins origin
        LEFT JOIN native_close_completion_retry_admissions admission ON admission.attempt_id = origin.attempt_id
        LEFT JOIN close_runs successor ON successor.attempt_id = admission.attempt_id
            AND successor.run_ordinal = admission.successor_run_ordinal
        WHERE origin.source_transcript_id = ?1")
        .bind(transcript_id).fetch_all(&mut **tx).await?;
    for origin in origins {
        let source = SourceToolCall {
            message_id: origin.try_get("source_message_id")?,
            tool_use_id: origin.try_get("source_tool_use_id")?,
        };
        let target =
            ProductConversationId::parse(origin.try_get::<String, _>("product_conversation_id")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let command = NativeCloseCommand::new_from_retained_fingerprint(
            transcript_id.to_owned(),
            source.clone(),
            NativeCloseIntent::Initial { target },
            &origin.try_get::<String, _>("origin_request_fingerprint")?,
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let source_lost = command
            .verify_executing_source(next_state, &source)
            .is_err();
        if !source_lost && matches!(cause, NativeSourceLossCause::UserSourceLoss) {
            continue;
        }
        let attempt_id: String = origin.try_get("attempt_id")?;
        revoke_completion_origin_tx(tx, &attempt_id).await?;
        if source_lost
            && matches!(cause, NativeSourceLossCause::UserSourceLoss)
            && origin
                .try_get::<Option<i64>, _>("successor_run_ordinal")?
                .is_some()
        {
            sqlx::query("UPDATE close_completion_retry_allowances SET suppressed = 1 WHERE attempt_id = ?1 AND suppressed = 0")
                .bind(&attempt_id).execute(&mut **tx).await?;
        }
        if origin.try_get::<Option<String>, _>("status")?.as_deref() == Some("running") {
            let run = CloseRunRef {
                attempt_id: CloseAttemptId::parse(attempt_id)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                ordinal: CloseRunOrdinal::parse(origin.try_get("successor_run_ordinal")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            };
            Database::terminalize_native_retry_source_fence_tx(
                tx,
                NativeRetrySourceFence { run, cause },
            )
            .await?;
        }
    }
    Ok(())
}
