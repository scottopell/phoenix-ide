//! Dormant initial-only acceptance. No cleanup, dispatch, retry or allowance claim.

use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal, TranscriptConversationId};
use phoenix_core::domain::product_conversation::ProductConversationId;
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

#[allow(clippy::too_many_lines)] // Source-before-replay and initial capture share one transaction.
async fn apply_initial_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
) -> DbResult<NativeCloseInitialResult> {
    super::receipts::require_source_and_target_tx(tx, command).await?;
    let NativeCloseIntent::Initial { target } = command.intent() else {
        return Err(DbError::CloseFoundationPrecondition(
            "expected initial Close intent".into(),
        ));
    };
    if let Some(admitted) = super::receipts::read_receipt_tx(tx, command).await? {
        return Ok(NativeCloseInitialResult::Replay(NativeCloseReceipt {
            request_id: command.request_id(),
            admitted,
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
        return Ok(
            NativeCloseInitialResult::ExistingOperationNeedsExactReference(decode_run_ref(&row)?),
        );
    }
    let topology = crate::close_foundation::read_topology_tx(tx, target)
        .await?
        .ok_or_else(|| {
            DbError::CloseFoundationPrecondition("Close target has no retained topology".into())
        })?;
    // Allocate only after source, exact replay, existing operation and topology checks.
    let latest = TranscriptConversationId::parse(topology.latest.id)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let attempt_id = Uuid::new_v4().to_string();
    let obligation =
        Database::begin_close_foundation_inner(tx, target, &latest, &attempt_id, false).await?;
    super::receipts::adopt_and_insert_receipt_tx(
        tx,
        command,
        &NativeCloseRunRef {
            target: target.clone(),
            attempt_id: obligation.attempt_id().clone(),
            run_ordinal: CloseRunOrdinal::INITIAL,
        },
    )
    .await?;
    Ok(NativeCloseInitialResult::NewlyAdmitted(
        NativeCloseReceipt {
            request_id: command.request_id(),
            admitted: NativeCloseRunRef {
                target: target.clone(),
                attempt_id: obligation.attempt_id().clone(),
                run_ordinal: CloseRunOrdinal::INITIAL,
            },
        },
    ))
}

pub(super) fn decode_run_ref(row: &sqlx::sqlite::SqliteRow) -> DbResult<NativeCloseRunRef> {
    let target: String = row.try_get("product_conversation_id")?;
    let attempt: String = row.try_get("attempt_id")?;
    Ok(NativeCloseRunRef {
        target: ProductConversationId::parse(target)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        attempt_id: CloseAttemptId::parse(attempt)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        run_ordinal: CloseRunOrdinal::parse(row.try_get("run_ordinal")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
    })
}

#[cfg(test)]
mod tests;
