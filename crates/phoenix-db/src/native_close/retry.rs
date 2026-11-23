//! Source-owned explicit native retry admission and fresh durable-member fencing.

use phoenix_core::domain::close::{
    CloseAttemptId, CloseRunRef, CloseRunStatus, RetiredResourceKind, RetirementOutcome,
};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseReceipt, NativeCloseRunRef};
use crate::close_foundation::{
    AdmitCloseSafeRetryRequest, CloseRetryRequestedBy, CloseSafeRetryEffect,
    StoppedCloseOriginalRetirementProof,
};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeCloseRetryResult {
    NewlyAdmitted(NativeCloseReceipt),
    Replay(NativeCloseReceipt),
}

/// Backend assessment boundary, not a wire request or caller-supplied safety flags.
/// The runtime owner must keep its implementation private and produce it only after
/// assessing the original retirement proof and the live resource identities.
pub trait NativeCloseRetrySourceAssessment: Send + Sync {
    fn command(&self) -> &NativeCloseCommand;
    fn original_proof(&self) -> &StoppedCloseOriginalRetirementProof;
    fn request(&self) -> &AdmitCloseSafeRetryRequest;
}

fn refused(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.to_owned())
}

impl Database {
    /// Rechecks current work of the immutable captured members, not their old
    /// captured state or the original run's settlement receipts. This is a
    /// durable-member gate only; live process/resource proof is independently
    /// required by the runtime owner. It does not acknowledge or stop any work.
    pub(super) async fn require_native_close_retry_members_quiescent_tx(
        tx: &mut Transaction<'_, Sqlite>,
        attempt: &CloseAttemptId,
    ) -> DbResult<()> {
        let active =
            Self::count_close_settlement_active_participants_tx(tx, attempt.as_str()).await?;
        if active != 0 {
            return Err(refused(&format!(
                "attempt {} still has {active} active durable captured member obligation(s)",
                attempt
            )));
        }
        Ok(())
    }

    /// Commits adoption, successor, original-bound plan and source receipt together.
    /// A replay validates source authority before consulting the durable receipt;
    /// fresh admission additionally compares the assessed proof inside the write transaction.
    pub async fn apply_native_close_explicit_retry(
        &self,
        assessment: &impl NativeCloseRetrySourceAssessment,
    ) -> LocalAuthorityResult<DbResult<NativeCloseRetryResult>> {
        let command = assessment.command();
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = self
            .apply_native_close_explicit_retry_tx(&mut tx, command, assessment)
            .await;
        match result {
            Ok(application) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(application)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    /// Reads the durable receipt for an explicit retry without assessing or admitting a retry.
    /// The source actor must perform its exclusive live source occurrence check before this call;
    /// this persisted source/receipt check supports that owner and does not replace its authority.
    /// It does not establish fresh predecessor proof or grant execution/finalization authority.
    ///
    /// # Errors
    /// Rejects non-retry intents, invalid current sources or targets, and conflicting receipts.
    /// Returns storage errors if the source or retained receipt cannot be read.
    pub async fn read_native_close_explicit_retry_receipt(
        &self,
        command: &NativeCloseCommand,
    ) -> DbResult<Option<NativeCloseRunRef>> {
        if !matches!(command.intent(), NativeCloseIntent::ExplicitRetry { .. }) {
            return Err(refused("receipt lookup requires an explicit retry intent"));
        }
        let mut tx = self.pool.begin().await?;
        super::receipts::require_source_and_target_tx(&mut tx, command).await?;
        let receipt = super::receipts::read_receipt_tx(&mut tx, command).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    #[allow(clippy::too_many_lines)] // Keep the atomic admission/replay decision in one transaction.
    async fn apply_native_close_explicit_retry_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        assessment: &impl NativeCloseRetrySourceAssessment,
    ) -> DbResult<NativeCloseRetryResult> {
        super::receipts::require_source_and_target_tx(tx, command).await?;
        let NativeCloseIntent::ExplicitRetry {
            target: product,
            attempt_id,
            failed_run_ordinal,
        } = command.intent()
        else {
            return Err(refused("explicit retry requires its exact intent"));
        };
        if let Some(admitted) = super::receipts::read_receipt_tx(tx, command).await? {
            return Ok(NativeCloseRetryResult::Replay(NativeCloseReceipt {
                request_id: command.request_id(),
                admitted,
            }));
        }
        let failed_run = CloseRunRef {
            attempt_id: attempt_id.clone(),
            ordinal: *failed_run_ordinal,
        };
        let proof = Self::read_stopped_close_original_retirement_proof_tx(tx, &failed_run).await?;
        if &proof != assessment.original_proof() {
            return Err(refused(
                "original retirement proof changed after assessment",
            ));
        }
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM close_obligations
             WHERE attempt_id = ?1 AND product_conversation_id = ?2
               AND phase = 'completed'
               AND close_outcome IN ('close_incomplete', 'archived_cleanup_attention'))",
        )
        .bind(failed_run.attempt_id.as_str())
        .bind(product.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !eligible || proof.run.status != CloseRunStatus::Stopped {
            return Err(refused(
                "retry requires the exact latest stopped incomplete Close",
            ));
        }
        Self::require_native_close_retry_members_quiescent_tx(tx, &failed_run.attempt_id).await?;
        let request = assessment.request();
        if request.failed_run != failed_run || request.requested_by != CloseRetryRequestedBy::User {
            return Err(refused(
                "native explicit retry requires its exact user-requested predecessor",
            ));
        }
        let now = chrono::Utc::now().timestamp_micros();
        if request.observed_at_us < 0
            || request.observed_at_us > now
            || request.precondition_resolution.trim().is_empty()
            || request.safety_evidence.trim().is_empty()
        {
            return Err(refused(
                "native retry requires fresh read-only precondition and safety evidence",
            ));
        }
        if request.remaining_effects.is_empty() {
            return Err(refused(
                "native completion-only retry requires a separate completion proof",
            ));
        }
        let pending = original_pending_effects_tx(tx, &proof).await?;
        validate_plan(&pending, &request.remaining_effects)?;
        let next_ordinal = failed_run
            .ordinal
            .checked_next()
            .map_err(|error| refused(&error.to_string()))?;
        let effects: Vec<_> = request
            .remaining_effects
            .iter()
            .map(|effect| (&effect.scope, &effect.resource))
            .collect();
        let plan_bytes = serde_json::to_vec(&(
            command.fingerprint(),
            &failed_run,
            &proof.snapshot,
            request.observed_at_us,
            &request.precondition_resolution,
            &request.safety_evidence,
            effects,
        ))
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let plan_fingerprint = Sha256::digest(plan_bytes).iter().fold(
            String::from("native-close-retry-plan-v1:"),
            |mut encoded, byte| {
                use std::fmt::Write as _;
                write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                encoded
            },
        );

        sqlx::query("UPDATE close_obligations SET native_managed = 1 WHERE attempt_id = ?1")
            .bind(failed_run.attempt_id.as_str())
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO native_close_retry_plans
             (attempt_id, run_ordinal, failed_run_ordinal, inspection_generation, inspection_fingerprint,
              plan_fingerprint, request_id, request_fingerprint, evidence_kind, effect_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'resource_plan', ?9)",
        )
        .bind(failed_run.attempt_id.as_str())
        .bind(next_ordinal.get())
        .bind(failed_run.ordinal.get())
        .bind(proof.snapshot.generation())
        .bind(proof.snapshot.fingerprint())
        .bind(&plan_fingerprint)
        .bind(command.request_id().to_string())
        .bind(command.fingerprint())
        .bind(i64::try_from(request.remaining_effects.len()).map_err(|_| refused("retry plan is too large"))?)
        .execute(&mut **tx)
        .await?;
        let run = CloseRunRef {
            attempt_id: failed_run.attempt_id.clone(),
            ordinal: next_ordinal,
        };
        Self::insert_close_retry_plan_and_run_tx(tx, request, &run, now).await?;
        let admitted = NativeCloseRunRef {
            target: product.clone(),
            attempt_id: run.attempt_id,
            run_ordinal: run.ordinal,
        };
        super::receipts::adopt_and_insert_receipt_tx(tx, command, &admitted).await?;
        Ok(NativeCloseRetryResult::NewlyAdmitted(NativeCloseReceipt {
            request_id: command.request_id(),
            admitted,
        }))
    }
}

pub(super) async fn original_pending_effects_tx(
    tx: &mut Transaction<'_, Sqlite>,
    proof: &StoppedCloseOriginalRetirementProof,
) -> DbResult<Vec<CloseSafeRetryEffect>> {
    let mut pending = Vec::new();
    for expected in &proof.resources {
        let receipt = proof
            .outcomes
            .iter()
            .find(|receipt| {
                receipt.attempt_id == expected.attempt_id
                    && receipt.scope == expected.scope
                    && receipt.snapshot == expected.snapshot
                    && receipt.resource == expected.resource
            })
            .ok_or_else(|| {
                refused("original resource is missing its original-generation receipt")
            })?;
        if !matches!(receipt.outcome, RetirementOutcome::Residual { .. }) {
            continue;
        }
        let identity = expected.resource.identity();
        // The effect join supplies identity and scope; success rows carry only an ordinal.
        let succeeded: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM close_run_retry_successes success
             JOIN close_run_retry_effects effect
               ON effect.attempt_id = success.attempt_id AND effect.run_ordinal = success.run_ordinal
              AND effect.ordinal = success.ordinal
             JOIN native_close_retry_plans plan
               ON plan.attempt_id = effect.attempt_id AND plan.run_ordinal = effect.run_ordinal
             JOIN native_close_initial_receipts receipt
               ON receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
              AND receipt.request_id = plan.request_id AND receipt.request_fingerprint = plan.request_fingerprint
              AND receipt.intent = 'explicit_retry'
             WHERE effect.attempt_id = ?1 AND effect.run_ordinal <= ?2
               AND plan.inspection_generation = ?3 AND plan.inspection_fingerprint = ?4
               AND effect.scope = ?5 AND effect.resource_kind = ?6
               AND effect.identity_kind = ?7 AND effect.identity_codec = ?8
               AND effect.identity_value = ?9)",
        )
        .bind(proof.run.run.attempt_id.as_str())
        .bind(proof.run.run.ordinal.get())
        .bind(proof.snapshot.generation())
        .bind(proof.snapshot.fingerprint())
        .bind(expected.scope.as_str())
        .bind(expected.resource.kind().as_str())
        .bind(identity.identity_kind())
        .bind(identity.codec())
        .bind(identity.value())
        .fetch_one(&mut **tx)
        .await?;
        if !succeeded {
            pending.push(CloseSafeRetryEffect {
                scope: expected.scope.clone(),
                resource: expected.resource.clone(),
            });
        }
    }
    Ok(pending)
}

fn validate_plan(
    pending: &[CloseSafeRetryEffect],
    effects: &[CloseSafeRetryEffect],
) -> DbResult<()> {
    if pending.is_empty() || effects.len() != pending.len() {
        return Err(refused(
            "native retry requires a nonempty complete original residual plan",
        ));
    }
    for (index, effect) in effects.iter().enumerate() {
        if !pending.contains(effect) || effects[..index].contains(effect) {
            return Err(refused(
                "retry effect is duplicate or not an exact original residual identity",
            ));
        }
        if !matches!(
            effect.resource.kind(),
            RetiredResourceKind::Worktree
                | RetiredResourceKind::WorkScope
                | RetiredResourceKind::TmuxServer
        ) {
            return Err(refused(
                "native retry cannot replay process-local resources",
            ));
        }
        if effect.resource.kind() == RetiredResourceKind::WorkScope
            && effects[index + 1..].iter().any(|later| {
                later.scope == effect.scope
                    && later.resource.kind() != RetiredResourceKind::WorkScope
            })
        {
            return Err(refused("work scope retry must follow its resource effects"));
        }
    }
    Ok(())
}
