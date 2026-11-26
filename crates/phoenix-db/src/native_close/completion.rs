//! Source-owned initial all-success interruption and historical completion receipt replay.

use phoenix_core::domain::close::{CloseObligation, CloseRunOrdinal, CloseRunRef};
use phoenix_core::domain::sm_state::ConvState;
use sqlx::{Row, Sqlite, Transaction};

use super::initial::require_initial_identity_tx;
use super::{NativeCloseCommand, NativeCloseReceipt, NativeCloseRunRef, NativeInitialSourceFence};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

/// Automatic successor binding, distinct from an explicit-retry source receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeCloseCompletionReceipt {
    pub(crate) origin: NativeCloseReceipt,
    pub(crate) successor: NativeCloseRunRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeCloseCompletionResult {
    Replay(NativeCloseCompletionReceipt),
}

impl Database {
    /// Records an unsuccessful interruption after every initial retirement effect succeeded.
    /// Keeps the executing source current and performs no physical effects or Close finalization.
    #[cfg(test)]
    pub(crate) async fn interrupt_native_close_initial_after_retirement(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &ConvState,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let admitted = require_initial_identity_tx(&mut tx, command, receipt, live_source).await?;
            require_positive_initial_retirement_tx(&mut tx, command, &admitted).await?;
            let status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM close_runs run
                 WHERE run.attempt_id = ?1 AND run.run_ordinal = 1
                   AND NOT EXISTS (SELECT 1 FROM close_runs later
                                   WHERE later.attempt_id = run.attempt_id AND later.run_ordinal > 1)",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            match status.as_deref() {
                Some("running") => {
                    Self::terminalize_native_initial_source_fence_tx(
                        &mut tx,
                        NativeInitialSourceFence::process_interrupted(CloseRunRef {
                            attempt_id: admitted.attempt_id.clone(),
                            ordinal: CloseRunOrdinal::INITIAL,
                        }),
                    )
                    .await?;
                }
                Some("stopped") => {}
                _ => {
                    return Err(precondition(
                        "all-success initial interruption requires the latest running or stopped initial run",
                    ));
                }
            }
            require_stopped_initial_interruption_tx(&mut tx, &admitted).await?;
            crate::close_foundation::close_obligation_for_update(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await
        }
        .await;
        match result {
            Ok(obligation) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(obligation)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    /// Finalizes the exact automatic completion successor without executing cleanup effects.
    #[cfg(test)]
    pub(crate) async fn complete_native_close_initial_completion_retry(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseCompletionReceipt,
        live_source: &ConvState,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let admitted =
                require_initial_identity_tx(&mut tx, command, &receipt.origin, live_source).await?;
            if receipt.origin.admitted != admitted
                || receipt.successor.attempt_id != admitted.attempt_id
                || receipt.successor.target != admitted.target
                || receipt.successor.run_ordinal.get() != 2
                || !read_completion_receipt_tx(&mut tx, command, receipt).await?
            {
                return Err(precondition("exact automatic completion receipt is missing"));
            }
            let run = CloseRunRef {
                attempt_id: receipt.successor.attempt_id.clone(),
                ordinal: receipt.successor.run_ordinal,
            };
            let status: Option<String> = sqlx::query_scalar(
                "SELECT successor_status FROM native_close_completion_retry_successors
                 WHERE attempt_id = ?1 AND successor_run_ordinal = ?2
                   AND origin_request_id = ?3 AND origin_request_fingerprint = ?4
                   AND product_conversation_id = ?5",
            )
            .bind(run.attempt_id.as_str())
            .bind(run.ordinal.get())
            .bind(command.request_id().to_string())
            .bind(command.fingerprint())
            .bind(receipt.successor.target.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            match status.as_deref() {
                Some("running") => Self::complete_close_retry_tx(&mut tx, &run).await,
                Some("completed") => {
                    let archived: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM native_close_completion_retry_successors successor
                         JOIN close_obligations obligation ON obligation.attempt_id = successor.attempt_id
                         WHERE successor.attempt_id = ?1 AND successor.successor_run_ordinal = ?2
                           AND successor.successor_status = 'completed'
                           AND obligation.product_conversation_id = ?3
                           AND obligation.native_managed = 1 AND obligation.phase = 'completed'
                           AND obligation.close_outcome = 'archived')",
                    )
                    .bind(run.attempt_id.as_str())
                    .bind(run.ordinal.get())
                    .bind(receipt.successor.target.as_str())
                    .fetch_one(&mut *tx)
                    .await?;
                    if !archived {
                        return Err(precondition("automatic completion replay lacks archived proof"));
                    }
                    crate::close_foundation::close_obligation_for_update(
                        &mut tx,
                        run.attempt_id.as_str(),
                    )
                    .await
                }
                _ => Err(precondition("automatic completion successor is not finalizable")),
            }
        }
        .await;
        match result {
            Ok(obligation) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(obligation)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    /// Replays an existing historical completion receipt; never creates a new successor.
    /// Revalidates live and durable source identity before consulting retained history.
    #[cfg(test)]
    pub(crate) async fn admit_native_close_initial_completion_retry(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &ConvState,
    ) -> LocalAuthorityResult<DbResult<NativeCloseCompletionResult>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let admitted =
                require_initial_identity_tx(&mut tx, command, receipt, live_source).await?;
            let completion = NativeCloseCompletionReceipt {
                origin: receipt.clone(),
                successor: NativeCloseRunRef {
                    target: admitted.target.clone(),
                    attempt_id: admitted.attempt_id.clone(),
                    run_ordinal: admitted
                        .run_ordinal
                        .checked_next()
                        .map_err(|error| precondition(&error.to_string()))?,
                },
            };
            if read_completion_receipt_tx(&mut tx, command, &completion).await? {
                return Ok(NativeCloseCompletionResult::Replay(completion));
            }
            Err(precondition(
                "new automatic completion successor admission is disabled",
            ))
        }
        .await;
        match result {
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

async fn require_positive_initial_retirement_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    admitted: &NativeCloseRunRef,
) -> DbResult<()> {
    let proved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM native_close_initial_complete_retirement_proofs proof
         WHERE proof.attempt_id = ?1 AND proof.request_id = ?2 AND proof.request_fingerprint = ?3
           AND proof.product_conversation_id = ?4 AND proof.run_ordinal = 1
           AND proof.source_transcript_id = ?5 AND proof.source_message_id = ?6
           AND proof.source_tool_use_id = ?7)",
    )
    .bind(admitted.attempt_id.as_str())
    .bind(command.request_id().to_string())
    .bind(command.fingerprint())
    .bind(admitted.target.as_str())
    .bind(command.source_transcript_id())
    .bind(command.source().message_id.as_str())
    .bind(command.source().tool_use_id.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if !proved {
        return Err(precondition(
            "initial completion requires exact positive original retirement and shutdown proof",
        ));
    }
    Ok(())
}

async fn require_stopped_initial_interruption_tx(
    tx: &mut Transaction<'_, Sqlite>,
    admitted: &NativeCloseRunRef,
) -> DbResult<()> {
    let run = CloseRunRef {
        attempt_id: admitted.attempt_id.clone(),
        ordinal: CloseRunOrdinal::INITIAL,
    };
    let eligible: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_runs run
         JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
         JOIN close_cleanup_failures failure
           ON failure.attempt_id = run.attempt_id AND failure.cleanup_run_ordinal = run.run_ordinal
         WHERE run.attempt_id = ?1 AND run.run_ordinal = 1 AND run.status = 'stopped'
           AND obligation.product_conversation_id = ?2 AND obligation.native_managed = 1
           AND obligation.phase = 'completed'
           AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
           AND failure.failure_occurrence_id = ?3
           AND failure.source_product_conversation_id = obligation.product_conversation_id
           AND failure.authority_kind = 'attempt_interrupted' AND failure.reason = 'interrupted'
           AND failure.stop_certainty = 'conversation_and_processes_stopped'
           AND failure.confirmed_at_us >= run.created_at_us
           AND failure.confirmed_at_us <= failure.occurred_at_us
           AND failure.occurred_at_us = run.ended_at_us
           AND failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
           AND failure.scope IS NULL AND failure.resource_kind IS NULL
           AND failure.identity_kind IS NULL AND failure.identity_codec IS NULL
           AND failure.identity_value IS NULL
           AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                           WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
           AND EXISTS (SELECT 1 FROM coordinator_watch_events event
                       WHERE event.mandatory_failure_occurrence_id = failure.failure_occurrence_id
                         AND event.route_kind = 'mandatory_close_failure')
           AND NOT EXISTS (SELECT 1 FROM close_runs later
                           WHERE later.attempt_id = run.attempt_id AND later.run_ordinal > 1)
           AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan
                           WHERE plan.attempt_id = run.attempt_id)
           AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
                           WHERE effect.attempt_id = run.attempt_id))",
    )
    .bind(admitted.attempt_id.as_str())
    .bind(admitted.target.as_str())
    .bind(run.failure_occurrence_id())
    .fetch_one(&mut **tx)
    .await?;
    if !eligible {
        return Err(precondition(
            "completion-only admission requires the exact latest stopped initial interruption, primary failure and mandatory delivery with no resource failures",
        ));
    }
    Ok(())
}

async fn read_completion_receipt_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    completion: &NativeCloseCompletionReceipt,
) -> DbResult<bool> {
    let Some(row) = sqlx::query(
        "SELECT admission.origin_request_id, admission.origin_request_fingerprint,
                admission.attempt_id, admission.product_conversation_id,
                admission.failed_run_ordinal, admission.successor_run_ordinal,
                EXISTS(SELECT 1 FROM close_runs run
                       JOIN native_close_completion_retry_origins origin
                         ON origin.attempt_id = admission.attempt_id
                        AND origin.origin_request_id = admission.origin_request_id
                        AND origin.origin_request_fingerprint = admission.origin_request_fingerprint
                       JOIN close_completion_retry_allowances allowance
                         ON allowance.attempt_id = admission.attempt_id
                       WHERE run.attempt_id = admission.attempt_id
                         AND run.run_ordinal = admission.successor_run_ordinal
                         AND run.status IN ('running', 'stopped', 'completed')
                         AND run.retry_evidence_kind = 'verified_completion'
                         AND run.retry_requested_by = 'global'
                         AND run.created_at_us = admission.admitted_at_us
                         AND run.retry_observed_at_us = admission.admitted_at_us
                         AND allowance.consumed = 1
                         AND (run.status = 'completed' OR (
                           allowance.suppressed = 0
                           AND NOT EXISTS (SELECT 1 FROM native_close_completion_retry_source_revocations revoked
                                           WHERE revoked.attempt_id = admission.attempt_id)
                         ))
                         AND origin.product_conversation_id = admission.product_conversation_id
                         AND origin.initial_run_ordinal = admission.failed_run_ordinal
                         AND origin.source_transcript_id = ?3
                         AND origin.source_message_id = ?4 AND origin.source_tool_use_id = ?5
                         AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
                                         WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal)
                         AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan
                                         WHERE plan.attempt_id = run.attempt_id AND plan.run_ordinal = run.run_ordinal)
                         AND NOT EXISTS (SELECT 1 FROM native_close_initial_receipts receipt
                                         WHERE receipt.attempt_id = run.attempt_id AND receipt.run_ordinal = run.run_ordinal)) AS exact_successor
         FROM native_close_completion_retry_admissions admission
         WHERE admission.attempt_id = ?1 OR admission.origin_request_id = ?2",
    )
    .bind(completion.successor.attempt_id.as_str())
    .bind(command.request_id().to_string())
    .bind(command.source_transcript_id())
    .bind(command.source().message_id.as_str())
    .bind(command.source().tool_use_id.as_str())
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(false);
    };
    let request_id: String = row.try_get("origin_request_id")?;
    let fingerprint: String = row.try_get("origin_request_fingerprint")?;
    let attempt_id: String = row.try_get("attempt_id")?;
    let target: String = row.try_get("product_conversation_id")?;
    let failed: i64 = row.try_get("failed_run_ordinal")?;
    let successor: i64 = row.try_get("successor_run_ordinal")?;
    let exact_successor: bool = row.try_get("exact_successor")?;
    if request_id != command.request_id().to_string()
        || fingerprint != command.fingerprint()
        || attempt_id != completion.successor.attempt_id.as_str()
        || target != completion.successor.target.as_str()
        || failed != completion.origin.admitted.run_ordinal.get()
        || successor != completion.successor.run_ordinal.get()
        || !exact_successor
    {
        return Err(precondition(
            "automatic completion admission does not bind this exact initial receipt and successor",
        ));
    }
    Ok(true)
}

fn precondition(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.into())
}
