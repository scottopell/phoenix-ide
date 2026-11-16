//! Source-bound execution of the exact admitted native resource plan.

use phoenix_core::domain::close::{
    CloseObligation, CloseRetirementSnapshot, CloseRunRef, CloseStopCertainty,
};
use sqlx::{Row, Sqlite, Transaction};

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use crate::close_foundation::{
    close_obligation_for_update, retained_close_retry_progress_tx, CloseCleanupFailureAuthority,
    CloseCleanupFailureResource, CloseCleanupResourceDisposition, CloseSafeRetryEffect,
    TerminalizeInitialCloseCleanupFailureRequest,
};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

fn refused(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.to_owned())
}

struct NativeCloseRetryPlanIdentity {
    effect_count: i64,
    snapshot: CloseRetirementSnapshot,
    running: bool,
    created_at_us: i64,
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
        let result =
            Self::record_native_close_retry_success_tx(&mut tx, command, admitted, effect, detail)
                .await;
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

    /// Completes only the exact latest running native retry after every persisted effect
    /// (including independent `WorkScope` retirement) has durably succeeded.
    /// The source actor must first hold and check its exclusive live source occurrence;
    /// this transactional fence does not itself authorize dispatch.
    ///
    /// Historical command receipts remain replayable with a live source, but a second
    /// finalization deliberately refuses the completed run. It never allocates a run
    /// or inserts a second outcome message, and does not consume or renew an allowance.
    ///
    /// # Errors
    /// Source, receipt, original-plan, incomplete-proof and storage errors are classified
    /// only when rollback establishes no mutation committed. Commit or rollback
    /// uncertainty returns `DurableFactUnclassified`, as for success recording.
    pub async fn complete_native_close_retry(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = Self::complete_native_close_retry_tx(&mut tx, command, admitted).await;
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

    /// Records a resource failure under the admitted original-generation plan atomically.
    /// The source actor must hold its exclusive live source occurrence before calling.
    /// Exact stopped-run replay is read-only; it still requires the live source and latest plan.
    ///
    /// # Errors
    /// Refuses stale source, receipt, plan or failure evidence. Commit/rollback uncertainty
    /// returns `DurableFactUnclassified`; a successful rollback establishes no mutation.
    pub async fn terminalize_native_close_retry_resource_failure(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: TerminalizeInitialCloseCleanupFailureRequest,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = Self::terminalize_native_close_retry_resource_failure_tx(
            &mut tx, command, admitted, &request,
        ).await;
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

    async fn terminalize_native_close_retry_resource_failure_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> DbResult<CloseObligation> {
        let plan = Self::require_native_close_retry_plan_identity_tx(tx, command, admitted).await?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        let CloseCleanupFailureAuthority::ExpectedResource { scope, snapshot, resource } = &request.authority else {
            return Err(refused("native resource failure requires expected-resource authority"));
        };
        let now = chrono::Utc::now().timestamp_micros();
        if request.attempt_id != run.attempt_id
            || request.source_product_conversation_id != admitted.target
            || request.failure_occurrence_id != run.failure_occurrence_id()
            || snapshot != &plan.snapshot
            || request.detail.trim().is_empty()
            || request.occurred_at_us < 0
            || request.occurred_at_us < plan.created_at_us
            || request.occurred_at_us > now
            || chrono::DateTime::from_timestamp_micros(request.occurred_at_us).is_none()
            || request.stop_certainty.confirmed_at_us().is_some_and(|confirmed| {
                confirmed < 0 || confirmed > request.occurred_at_us
            })
        {
            return Err(refused("native resource failure requires exact original authority and valid occurrence evidence"));
        }
        let progress = retained_close_retry_progress_tx(tx, &run).await?;
        let failed = CloseSafeRetryEffect { scope: scope.clone(), resource: resource.clone() };
        if i64::try_from(progress.successful.len() + progress.pending.len()).ok() != Some(plan.effect_count)
            || !progress.pending.contains(&failed)
        {
            return Err(refused("native resource failure requires a pending full-identity planned effect"));
        }
        let remaining: Vec<_> = progress.pending.iter().map(|effect| CloseCleanupFailureResource {
            scope: effect.scope.clone(),
            resource: effect.resource.clone(),
            disposition: if effect == &failed {
                CloseCleanupResourceDisposition::Failed
            } else {
                CloseCleanupResourceDisposition::Unattempted
            },
        }).collect();
        if request.remaining_resources != remaining {
            return Err(refused("native failure children must equal the ordered pending original plan"));
        }
        if matches!(request.stop_certainty, CloseStopCertainty::ConversationAndProcessesStopped { .. }) {
            Self::require_native_close_original_process_proof_tx(tx, &run, &plan.snapshot).await?;
        }
        let obligation = close_obligation_for_update(tx, run.attempt_id.as_str()).await?;
        if !plan.running {
            let ended_at_us: Option<i64> = sqlx::query_scalar(
                "SELECT ended_at_us FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2",
            ).bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(&mut **tx).await?;
            if ended_at_us != Some(request.occurred_at_us)
                || !Self::retained_close_cleanup_failure_matches_tx(tx, &run, request).await?
            {
                return Err(refused("native resource failure replay conflicts with the retained terminal occurrence"));
            }
            return Ok(obligation);
        }
        Self::insert_and_finish_close_resource_failure_tx(tx, &run, request, obligation).await
    }

    async fn require_native_close_original_process_proof_tx(
        tx: &mut Transaction<'_, Sqlite>,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
    ) -> DbResult<()> {
        let unresolved: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM close_expected_retirement_resources expected
             WHERE expected.attempt_id = ?1 AND expected.inspection_generation = ?3
               AND expected.inspection_fingerprint = ?4
               AND expected.resource_kind IN ('bash_process_group', 'tmux_server', 'pty_session',
                 'browser_session', 'equivalent_live_resource')
               AND NOT EXISTS (SELECT 1 FROM close_retirement_resources retired
                 WHERE retired.attempt_id = expected.attempt_id AND retired.scope = expected.scope
                   AND retired.inspection_generation = expected.inspection_generation
                   AND retired.inspection_fingerprint = expected.inspection_fingerprint
                   AND retired.resource_kind = expected.resource_kind AND retired.identity_kind = expected.identity_kind
                   AND retired.identity_codec = expected.identity_codec AND retired.identity_value = expected.identity_value
                   AND retired.proof_kind IN ('retired', 'absence_adopted'))
               AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
                 JOIN close_run_retry_successes success ON success.attempt_id = effect.attempt_id
                   AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal
                 JOIN native_close_retry_plans plan ON plan.attempt_id = effect.attempt_id
                   AND plan.run_ordinal = effect.run_ordinal
                 JOIN native_close_initial_receipts receipt ON receipt.attempt_id = plan.attempt_id
                   AND receipt.run_ordinal = plan.run_ordinal AND receipt.request_id = plan.request_id
                   AND receipt.request_fingerprint = plan.request_fingerprint AND receipt.intent = 'explicit_retry'
                   AND receipt.product_conversation_id = (SELECT product_conversation_id
                     FROM close_obligations WHERE attempt_id = expected.attempt_id)
                 WHERE effect.attempt_id = expected.attempt_id AND effect.run_ordinal <= ?2
                   AND plan.inspection_generation = expected.inspection_generation
                   AND plan.inspection_fingerprint = expected.inspection_fingerprint
                   AND effect.scope = expected.scope AND effect.resource_kind = expected.resource_kind
                   AND effect.identity_kind = expected.identity_kind AND effect.identity_codec = expected.identity_codec
                   AND effect.identity_value = expected.identity_value))",
        ).bind(run.attempt_id.as_str()).bind(run.ordinal.get())
            .bind(snapshot.generation()).bind(snapshot.fingerprint()).fetch_one(&mut **tx).await?;
        if unresolved {
            return Err(refused("confirmed native stop requires exact original-generation process proof"));
        }
        Ok(())
    }

    async fn complete_native_close_retry_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> DbResult<CloseObligation> {
        Self::require_native_close_retry_plan_tx(tx, command, admitted).await?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        // The common storage boundary requires every exact effect's success before
        // atomically writing the outcome message, History and terminal run disposition.
        Self::complete_close_retry_tx(tx, &run).await
    }

    async fn record_native_close_retry_success_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        detail: &str,
    ) -> DbResult<()> {
        let effect_count = Self::require_native_close_retry_plan_tx(tx, command, admitted).await?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        Self::record_native_close_retry_success_effect_tx(tx, &run, effect, detail, effect_count)
            .await
    }

    // Shared by success recording and finalization; receipt identity never replaces
    // current source authority, and neither boundary rebinds original G1 to current G2.
    async fn require_native_close_retry_plan_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> DbResult<i64> {
        let plan = Self::require_native_close_retry_plan_identity_tx(tx, command, admitted).await?;
        if !plan.running {
            return Err(refused("native retry execution requires its exact latest running plan"));
        }
        Ok(plan.effect_count)
    }

    async fn require_native_close_retry_plan_identity_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> DbResult<NativeCloseRetryPlanIdentity> {
        // Source authority must precede receipt lookup, including stopped failure replay.
        super::receipts::require_source_and_target_tx(tx, command).await?;
        let NativeCloseIntent::ExplicitRetry {
            failed_run_ordinal, ..
        } = command.intent()
        else {
            return Err(refused(
                "native retry execution requires an explicit retry intent",
            ));
        };
        if super::receipts::read_receipt_tx(tx, command)
            .await?
            .as_ref()
            != Some(admitted)
        {
            return Err(refused(
                "native retry execution requires its exact admitted receipt",
            ));
        }
        // Original G1 authority is deliberate: the obligation may retain a later G2 inspection.
        let row = sqlx::query(
            "SELECT plan.effect_count, plan.inspection_generation, plan.inspection_fingerprint,
                    run.status = 'running' AS running, run.created_at_us FROM native_close_retry_plans plan
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
               AND run.status IN ('running', 'stopped') AND run.retry_evidence_kind = 'resource_plan'
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
        let row = row.ok_or_else(|| {
            refused("native retry execution requires its exact latest original-bound plan")
        })?;
        Ok(NativeCloseRetryPlanIdentity {
            effect_count: row.try_get("effect_count")?,
            snapshot: CloseRetirementSnapshot::parse(
                row.try_get::<String, _>("inspection_generation")?,
                row.try_get::<String, _>("inspection_fingerprint")?,
            ).map_err(|error| DbError::Serialization(error.to_string()))?,
            running: row.try_get("running")?,
            created_at_us: row.try_get("created_at_us")?,
        })
    }
}
