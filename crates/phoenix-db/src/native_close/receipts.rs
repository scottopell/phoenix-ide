//! Shared source validation and the single durable receipt authority.

use phoenix_core::domain::sm_state::ConvState;
use sqlx::{Row, Sqlite, Transaction};

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use crate::{DbError, DbResult};

pub(super) async fn require_source_and_target_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
) -> DbResult<()> {
    // MUST precede receipt lookup, including the exact historical replay path.
    crate::product_conversation_rename::require_current_gc_tx(tx, command.source_transcript_id())
        .await?;
    let state_json: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
        .bind(command.source_transcript_id())
        .fetch_one(&mut **tx)
        .await?;
    let state: ConvState = serde_json::from_str(&state_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let target = command.intent().target();
    let input = command.intent().source_input();
    crate::task_approval_inspection::require_executing_source(
        &state,
        command.source(),
        "close_conversation",
        &input,
    )
    .map_err(|_| {
        DbError::CloseFoundationPrecondition("Close requires its current executing source".into())
    })?;
    let ordinary: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM product_conversations WHERE id = ?1 AND kind = 'ordinary')",
    )
    .bind(target.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if !ordinary {
        return Err(DbError::CloseFoundationPrecondition(
            "Close requires an ordinary target".into(),
        ));
    }
    Ok(())
}

pub(super) async fn read_receipt_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
) -> DbResult<Option<NativeCloseRunRef>> {
    if let Some(row) = sqlx::query(
        "SELECT source_transcript_id, source_message_id, source_tool_use_id,
                request_fingerprint, product_conversation_id, attempt_id, run_ordinal, intent
         FROM native_close_initial_receipts WHERE request_id = ?1",
    )
    .bind(command.request_id().to_string())
    .fetch_optional(&mut **tx)
    .await?
    {
        let source_transcript: String = row.try_get("source_transcript_id")?;
        let message: String = row.try_get("source_message_id")?;
        let tool: String = row.try_get("source_tool_use_id")?;
        let fingerprint: String = row.try_get("request_fingerprint")?;
        let retained_target: String = row.try_get("product_conversation_id")?;
        let intent: String = row.try_get("intent")?;
        let referenced = super::initial::decode_run_ref(&row)?;
        let exact_matches = match command.intent() {
            NativeCloseIntent::Initial { .. } => {
                referenced.run_ordinal == phoenix_core::domain::close::CloseRunOrdinal::INITIAL
            }
            NativeCloseIntent::Observe {
                attempt_id,
                run_ordinal,
                ..
            } => &referenced.attempt_id == attempt_id && referenced.run_ordinal == *run_ordinal,
            NativeCloseIntent::ExplicitRetry {
                attempt_id,
                failed_run_ordinal,
                ..
            } => {
                &referenced.attempt_id == attempt_id
                    && failed_run_ordinal.checked_next().ok() == Some(referenced.run_ordinal)
            },
        };
        if source_transcript != command.source_transcript_id()
            || message != command.source().message_id
            || tool != command.source().tool_use_id
            || fingerprint != command.fingerprint()
            || retained_target != command.intent().target().as_str()
            || intent != command.intent().receipt_intent()
            || !exact_matches
        {
            return Err(DbError::CloseFoundationConflict(
                "Close request identity conflict".into(),
            ));
        }
        return Ok(Some(referenced));
    }
    Ok(None)
}

pub(super) async fn adopt_and_insert_receipt_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    referenced: &NativeCloseRunRef,
) -> DbResult<()> {
    let marked = sqlx::query(
        "UPDATE close_obligations SET native_managed = 1
         WHERE attempt_id = ?1 AND product_conversation_id = ?2",
    )
    .bind(referenced.attempt_id.as_str())
    .bind(referenced.target.as_str())
    .execute(&mut **tx)
    .await?;
    if marked.rows_affected() != 1 {
        return Err(DbError::CloseFoundationPrecondition(
            "Close native operation binding disappeared".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO native_close_initial_receipts
         (request_id, source_transcript_id, source_message_id, source_tool_use_id,
          request_fingerprint, product_conversation_id, attempt_id, run_ordinal, intent)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(command.request_id().to_string())
    .bind(command.source_transcript_id())
    .bind(command.source().message_id.as_str())
    .bind(command.source().tool_use_id.as_str())
    .bind(command.fingerprint())
    .bind(referenced.target.as_str())
    .bind(referenced.attempt_id.as_str())
    .bind(referenced.run_ordinal.get())
    .bind(command.intent().receipt_intent())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
