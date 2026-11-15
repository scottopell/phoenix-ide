//! Exact retained observation/adoption only: no admission, effects or allowance writes.

use phoenix_core::domain::close::{CloseObligation, CloseRunStatus};
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseObserveReceipt {
    pub request_id: Uuid,
    /// Immutable reference; never replaced by the operation's latest run.
    pub referenced: NativeCloseRunRef,
}

#[derive(Debug, Clone)]
pub struct NativeCloseObservation {
    pub receipt: NativeCloseObserveReceipt,
    /// Fresh retained status of precisely the receipt's run, including on replay.
    pub current_referenced_run_status: CloseRunStatus,
    /// Fresh operation state, which may have progressed beyond the referenced run.
    pub current_operation: CloseObligation,
}

#[derive(Debug, Clone)]
pub enum NativeCloseObserveResult {
    Observed(NativeCloseObservation),
    Replay(NativeCloseObservation),
}

impl Database {
    /// Observes and monotonically adopts one exact retained run atomically with
    /// its receipt. Source actor ownership and unclassified-outcome handling are
    /// identical to `apply_native_close_initial`; correlation is not authority.
    /// No missing original allowance or anchor is created by adoption.
    pub async fn apply_native_close_observe(
        &self,
        command: &NativeCloseCommand,
    ) -> LocalAuthorityResult<DbResult<NativeCloseObserveResult>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        match apply_observe_tx(&mut tx, command).await {
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

async fn apply_observe_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
) -> DbResult<NativeCloseObserveResult> {
    super::receipts::require_source_and_target_tx(tx, command).await?;
    let NativeCloseIntent::Observe { target, attempt_id, run_ordinal } = command.intent() else {
        return Err(DbError::CloseFoundationPrecondition("expected exact Close observation intent".into()));
    };
    let replay = super::receipts::read_receipt_tx(tx, command).await?;
    let referenced = replay.clone().unwrap_or_else(|| NativeCloseRunRef {
        target: target.clone(),
        attempt_id: attempt_id.clone(),
        run_ordinal: *run_ordinal,
    });
    let row = sqlx::query(
        "SELECT o.attempt_id, o.product_conversation_id, o.phase, o.inspection_generation,
                o.inspection_fingerprint, o.created_at, o.updated_at, o.completed_at, o.close_outcome
         FROM close_obligations o JOIN close_runs r ON r.attempt_id = o.attempt_id
         WHERE o.product_conversation_id = ?1 AND o.attempt_id = ?2 AND r.run_ordinal = ?3",
    )
    .bind(referenced.target.as_str())
    .bind(referenced.attempt_id.as_str())
    .bind(referenced.run_ordinal.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| DbError::CloseFoundationPrecondition("exact retained Close reference is missing or mismatched".into()))?;
    let current_operation = crate::close_foundation::parse_close_obligation_row(row)?;
    let row = sqlx::query(
        "SELECT attempt_id, run_ordinal, status FROM close_runs
         WHERE attempt_id = ?1 AND run_ordinal = ?2",
    )
    .bind(referenced.attempt_id.as_str())
    .bind(referenced.run_ordinal.get())
    .fetch_one(&mut **tx)
    .await?;
    let retained_run = crate::close_foundation::parse_close_run_row(row)?;
    if replay.is_none() {
        super::receipts::adopt_and_insert_receipt_tx(tx, command, &referenced).await?;
    }
    let observation = NativeCloseObservation {
        receipt: NativeCloseObserveReceipt { request_id: command.request_id(), referenced },
        current_referenced_run_status: retained_run.status,
        current_operation,
    };
    Ok(if replay.is_some() {
        NativeCloseObserveResult::Replay(observation)
    } else {
        NativeCloseObserveResult::Observed(observation)
    })
}

#[cfg(test)]
mod tests;
