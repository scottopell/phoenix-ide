//! Dormant initial-only acceptance. No cleanup, dispatch, retry or allowance claim.

use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal, TranscriptConversationId};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::{NativeCloseCommand, NativeCloseIntent};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseRunRef {
    pub target: ProductConversationId,
    pub attempt_id: CloseAttemptId,
    pub run_ordinal: CloseRunOrdinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseReceipt {
    pub request_id: Uuid,
    pub admitted: NativeCloseRunRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeCloseInitialResult {
    NewlyAdmitted(NativeCloseReceipt),
    Replay(NativeCloseReceipt),
    /// A fresh invocation must explicitly name retained work, not silently adopt it.
    ExistingOperationNeedsExactReference(NativeCloseRunRef),
}

impl Database {
    /// Accepts initial Close atomically, with one server-owned operation and receipt.
    ///
    /// `command` is correlation, NOT live source authority. The SOURCE actor must
    /// hold exclusive ownership before calling and until this result is classified.
    /// Persisted source checking is additional defense and deliberately has no
    /// historical canonical fallback. This dormant API is not a public tool ingress.
    ///
    /// An unclassified commit/rollback is NOT a refusal or a successful receipt:
    /// the source owner must enter its existing fatal-authority path. A cancelled
    /// caller must not release transferred actor/admission ownership prematurely.
    pub async fn apply_native_close_initial(
        &self,
        command: &NativeCloseCommand,
    ) -> LocalAuthorityResult<DbResult<NativeCloseInitialResult>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        match apply_initial_tx(&mut tx, command).await {
            Ok(result) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(result)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }
}

async fn apply_initial_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
) -> DbResult<NativeCloseInitialResult> {
    // MUST precede receipt lookup, including the exact historical replay path.
    crate::product_conversation_rename::require_current_gc_tx(tx, command.source_transcript_id())
        .await?;
    let state_json: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
        .bind(command.source_transcript_id())
        .fetch_one(&mut **tx)
        .await?;
    let state: ConvState = serde_json::from_str(&state_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let NativeCloseIntent::Initial { target } = command.intent();
    let input = serde_json::json!({"action":"initial", "product_conversation_id":target.as_str()});
    crate::task_approval_inspection::require_executing_source(
        &state,
        command.source(),
        "close_conversation",
        &input,
    )
    .map_err(|_| DbError::InvalidData("Close requires its current executing source".into()))?;
    let ordinary: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM product_conversations WHERE id = ?1 AND kind = 'ordinary')",
    )
    .bind(target.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if !ordinary {
        return Err(DbError::InvalidData(
            "Close requires an ordinary target".into(),
        ));
    }
    if let Some(row) = sqlx::query(
        "SELECT source_transcript_id, source_message_id, source_tool_use_id,
                request_fingerprint, product_conversation_id, attempt_id, run_ordinal
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
        if source_transcript != command.source_transcript_id()
            || message != command.source().message_id
            || tool != command.source().tool_use_id
            || fingerprint != command.fingerprint()
            || retained_target != target.as_str()
        {
            return Err(DbError::InvalidData("Close request identity conflict".into()));
        }
        return Ok(NativeCloseInitialResult::Replay(NativeCloseReceipt {
            request_id: command.request_id(),
            admitted: decode_run_ref(&row)?,
        }));
    }
    // Receipt replay precedes current target topology/latest checks. A different
    // request cannot adopt ANY retained predecessor, including completed/cancelled.
    if let Some(row) = sqlx::query(
        "SELECT o.product_conversation_id, o.attempt_id, MAX(r.run_ordinal) AS run_ordinal
         FROM close_obligations o JOIN close_runs r ON r.attempt_id = o.attempt_id
         WHERE o.product_conversation_id = ?1
         GROUP BY o.attempt_id ORDER BY o.chronology_ordinal DESC LIMIT 1",
    )
    .bind(target.as_str())
    .fetch_optional(&mut **tx)
    .await?
    {
        return Ok(NativeCloseInitialResult::ExistingOperationNeedsExactReference(
            decode_run_ref(&row)?,
        ));
    }
    let topology = crate::close_foundation::read_topology_tx(tx, target)
        .await?
        .ok_or_else(|| DbError::InvalidData("Close target has no retained topology".into()))?;
    // Allocate only after source, exact replay, existing operation and topology checks.
    let latest = TranscriptConversationId::parse(topology.latest.id)
        .map_err(|error| DbError::InvalidData(error.to_string()))?;
    let attempt_id = Uuid::new_v4().to_string();
    let obligation = Database::begin_close_foundation_inner(
        tx,
        target,
        &latest,
        &attempt_id,
        false,
    )
    .await?;
    let marked = sqlx::query(
        "UPDATE close_obligations SET native_managed = 1
         WHERE attempt_id = ?1 AND product_conversation_id = ?2",
    )
    .bind(obligation.attempt_id().as_str())
    .bind(target.as_str())
    .execute(&mut **tx)
    .await?;
    if marked.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Close native operation binding disappeared".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO native_close_initial_receipts
         (request_id, source_transcript_id, source_message_id, source_tool_use_id,
          request_fingerprint, product_conversation_id, attempt_id, run_ordinal)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
    )
    .bind(command.request_id().to_string())
    .bind(command.source_transcript_id())
    .bind(command.source().message_id.as_str())
    .bind(command.source().tool_use_id.as_str())
    .bind(command.fingerprint())
    .bind(target.as_str())
    .bind(obligation.attempt_id().as_str())
    .execute(&mut **tx)
    .await?;
    Ok(NativeCloseInitialResult::NewlyAdmitted(NativeCloseReceipt {
        request_id: command.request_id(),
        admitted: NativeCloseRunRef {
            target: target.clone(),
            attempt_id: obligation.attempt_id().clone(),
            run_ordinal: CloseRunOrdinal::INITIAL,
        },
    }))
}

fn decode_run_ref(row: &sqlx::sqlite::SqliteRow) -> DbResult<NativeCloseRunRef> {
    let target: String = row.try_get("product_conversation_id")?;
    let attempt: String = row.try_get("attempt_id")?;
    Ok(NativeCloseRunRef {
        target: ProductConversationId::parse(target)
            .map_err(|error| DbError::InvalidData(error.to_string()))?,
        attempt_id: CloseAttemptId::parse(attempt)
            .map_err(|error| DbError::InvalidData(error.to_string()))?,
        run_ordinal: CloseRunOrdinal::parse(row.try_get("run_ordinal")?)
            .map_err(|error| DbError::InvalidData(error.to_string()))?,
    })
}

#[cfg(test)]
mod tests;
