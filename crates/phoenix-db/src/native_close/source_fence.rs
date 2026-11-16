use super::{NativeCloseCommand, NativeCloseIntent};
use crate::close_foundation::CloseCleanupFailureAuthority;
use crate::task_approval_inspection::require_executing_source;
use crate::{ConvState, Database, DbError, DbResult};
use phoenix_core::domain::close::{CloseAttemptId, CloseObligation, CloseRunOrdinal, CloseRunRef};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{Row, Sqlite, Transaction};

#[derive(Clone, Copy)]
enum NativeSourceLossCause {
    UserSourceLoss,
    ProcessInterrupted,
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
    fence_native_sources_tx(tx, transcript_id, next_state, NativeSourceLossCause::UserSourceLoss).await
}

/// Startup preserves both allowance bits while stopping admitted native resource retries.
pub(crate) async fn recover_native_retry_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
) -> DbResult<()> {
    fence_native_sources_tx(tx, transcript_id, next_state, NativeSourceLossCause::ProcessInterrupted).await
}

/// Returns None for runs outside the native explicit-retry protocol.
pub(crate) async fn interrupt_native_retry_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run: &CloseRunRef,
) -> DbResult<Option<CloseObligation>> {
    let native_plan: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM native_close_retry_plans
         WHERE attempt_id = ?1 AND run_ordinal = ?2)",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_one(&mut **tx)
    .await?;
    if !native_plan {
        return Ok(None);
    }
    Database::terminalize_native_retry_source_fence_tx(tx, NativeRetrySourceFence {
        run: run.clone(),
        cause: NativeSourceLossCause::ProcessInterrupted,
    }).await.map(Some)
}

async fn fence_native_sources_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    next_state: &ConvState,
    cause: NativeSourceLossCause,
) -> DbResult<()> {
    let rows = sqlx::query(
        "SELECT receipt.attempt_id, receipt.run_ordinal, receipt.intent,
                plan.failed_run_ordinal, run.status,
                receipt.source_message_id, receipt.source_tool_use_id,
                receipt.product_conversation_id, receipt.request_fingerprint
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
        let target = ProductConversationId::parse(row.try_get::<String, _>("product_conversation_id")?)
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
        let command = NativeCloseCommand::new(transcript_id.to_owned(), source.clone(), intent)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        if command.fingerprint() != row.try_get::<String, _>("request_fingerprint")? {
            return Err(DbError::Serialization("native Close source receipt differs from its intent".into()));
        }
        let input = command.intent().source_input();
        if require_executing_source(next_state, &source, "close_conversation", &input).is_err()
        {
            if matches!(cause, NativeSourceLossCause::UserSourceLoss) {
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
            }
        }
    }
    for fence in fences {
        Database::terminalize_native_retry_source_fence_tx(tx, fence).await?;
    }
    Ok(())
}
