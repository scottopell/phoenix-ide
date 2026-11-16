//! Source-bound success recording for the exact admitted native resource plan.

use phoenix_core::domain::close::CloseRunRef;
use sqlx::{Sqlite, Transaction};

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use crate::close_foundation::CloseSafeRetryEffect;
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

fn refused(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.to_owned())
}

impl Database {
    /// Records one exact planned success while its native source and successor remain live.
    /// Identical evidence replays without another row, but never bypasses live authority
    /// or full effect equality (including captured worktree identity).
    /// The source actor must first hold and check its exclusive live source occurrence;
    /// this persisted transactional fence supports that owner, not dispatch authority.
    ///
    /// # Errors
    /// Returns classified source, receipt, plan, effect or evidence errors when rollback
    /// establishes that no mutation committed. Storage failures are classified likewise;
    /// commit or rollback uncertainty returns `DurableFactUnclassified`.
    pub async fn record_native_close_retry_success(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        detail: &str,
    ) -> LocalAuthorityResult<DbResult<()>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = Self::record_native_close_retry_success_tx(
            &mut tx, command, admitted, effect, detail,
        ).await;
        match result {
            Ok(()) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(())),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    async fn record_native_close_retry_success_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        detail: &str,
    ) -> DbResult<()> {
        // Source authority must precede receipt lookup, even for retained success replay.
        super::receipts::require_source_and_target_tx(tx, command).await?;
        let NativeCloseIntent::ExplicitRetry { failed_run_ordinal, .. } = command.intent() else {
            return Err(refused("native retry success requires an explicit retry intent"));
        };
        if super::receipts::read_receipt_tx(tx, command).await?.as_ref() != Some(admitted) {
            return Err(refused("native retry success requires its exact admitted receipt"));
        }
        // Original G1 authority is deliberate: the obligation may retain a later G2 inspection.
        let effect_count: Option<i64> = sqlx::query_scalar(
            "SELECT plan.effect_count FROM native_close_retry_plans plan
             JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = plan.attempt_id
               AND anchor.inspection_generation = plan.inspection_generation
               AND anchor.inspection_fingerprint = plan.inspection_fingerprint
             JOIN close_runs prior ON prior.attempt_id = plan.attempt_id
               AND prior.run_ordinal = plan.failed_run_ordinal AND prior.status = 'stopped'
             JOIN close_runs run ON run.attempt_id = plan.attempt_id AND run.run_ordinal = plan.run_ordinal
             JOIN close_obligations obligation ON obligation.attempt_id = plan.attempt_id
             WHERE plan.attempt_id = ?1 AND plan.run_ordinal = ?2
               AND plan.failed_run_ordinal = ?3 AND plan.run_ordinal = plan.failed_run_ordinal + 1
               AND plan.request_id = ?4 AND plan.request_fingerprint = ?5
               AND plan.evidence_kind = 'resource_plan' AND plan.effect_count > 0
               AND obligation.native_managed = 1 AND obligation.product_conversation_id = ?6
               AND obligation.phase = 'completed'
               AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
               AND run.status = 'running' AND run.retry_evidence_kind = 'resource_plan'
               AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = plan.attempt_id)
               AND NOT EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = plan.attempt_id)
               AND plan.effect_count = (SELECT COUNT(*) FROM close_run_retry_effects
                 WHERE attempt_id = plan.attempt_id AND run_ordinal = plan.run_ordinal)",
        )
        .bind(admitted.attempt_id.as_str())
        .bind(admitted.run_ordinal.get())
        .bind(failed_run_ordinal.get())
        .bind(command.request_id().to_string())
        .bind(command.fingerprint())
        .bind(admitted.target.as_str())
        .fetch_optional(&mut **tx)
        .await?;
        let effect_count = effect_count.ok_or_else(|| {
            refused("native retry success requires its exact latest running original-bound plan")
        })?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        Self::record_native_close_retry_success_effect_tx(tx, &run, effect, detail, effect_count).await
    }
}
