//! Explicit successor authority for the effect-free captured-loss origin.
//! Nothing here establishes or repairs an original retirement anchor.

use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptScope, CloseRetirementSnapshot, CloseRunRef,
    LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity, RetiredResourceKind,
};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};

use super::{
    NativeCloseCapturedLossRetryContext, NativeCloseCommand, NativeCloseIntent, NativeCloseReceipt,
    NativeCloseRetryResult, NativeCloseRunRef,
};
use crate::close_foundation::{
    encode_aggregate_snapshot_component, AdmitCloseSafeRetryRequest, CloseRetryRequestedBy,
    CloseSafeRetryEffect, ReplaceCloseInspectionRequest,
};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

/// Private runtime implementations attest fresh finite inspection and live safety.
/// This boundary is deliberately separate from the anchored retry assessment.
pub trait NativeCloseCapturedLossRetryAssessment: Send + Sync {
    fn command(&self) -> &NativeCloseCommand;
    fn context(&self) -> &NativeCloseCapturedLossRetryContext;
    fn inspection(&self) -> &ReplaceCloseInspectionRequest;
    fn observed_at_us(&self) -> i64;
    fn safety_evidence(&self) -> &str;
}

fn refused(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.to_owned())
}

pub(super) fn captured_effects(
    scopes: &[CloseAttemptScope],
) -> DbResult<Vec<CloseSafeRetryEffect>> {
    let mut effects = Vec::new();
    for scope in scopes {
        match &scope.captured_worktree {
            Some(CapturedWorktreeIdentity::Resolved(identity)) => {
                effects.push(CloseSafeRetryEffect {
                    scope: scope.scope.clone(),
                    resource: RetiredResourceIdentity::parse(
                        RetiredResourceKind::Worktree,
                        LossItemIdentity::Worktree(identity.clone()),
                    )
                    .map_err(|error| refused(&error.to_string()))?,
                })
            }
            Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                return Err(refused(
                    "captured retry cannot authorize an unresolved worktree",
                ))
            }
            None => {}
        }
        effects.push(CloseSafeRetryEffect {
            scope: scope.scope.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::WorkScope,
                LossItemIdentity::Opaque(
                    OpaqueIdentity::parse(scope.scope.as_str())
                        .map_err(|error| refused(&error.to_string()))?,
                ),
            )
            .map_err(|error| refused(&error.to_string()))?,
        });
    }
    if effects.is_empty() {
        return Err(refused(
            "captured retry requires a nonempty finite resource plan",
        ));
    }
    Ok(effects)
}

fn fresh_snapshot(
    context: &NativeCloseCapturedLossRetryContext,
    inspection: &ReplaceCloseInspectionRequest,
) -> DbResult<CloseRetirementSnapshot> {
    let captured: Vec<_> = context
        .scopes()
        .iter()
        .filter(|scope| scope.captured_worktree.is_some())
        .map(|scope| &scope.scope)
        .collect();
    let provided: Vec<_> = inspection.scopes.iter().map(|scope| &scope.scope).collect();
    if inspection.attempt_id != context.failed_run().attempt_id
        || captured != provided
        || inspection.scopes.iter().any(|scope| {
            !scope.losses.is_empty()
                || context.inspections().iter().any(|old| {
                    old.target.scope == scope.scope
                        && old.snapshot.generation() == scope.snapshot.generation()
                })
        })
    {
        return Err(refused(
            "captured retry requires an exact ordered full fresh clean inspection",
        ));
    }
    CloseRetirementSnapshot::parse(
        encode_aggregate_snapshot_component(
            inspection
                .scopes
                .iter()
                .map(|scope| (&scope.scope, scope.snapshot.generation())),
        ),
        encode_aggregate_snapshot_component(
            inspection
                .scopes
                .iter()
                .map(|scope| (&scope.scope, scope.snapshot.fingerprint())),
        ),
    )
    .map_err(|error| refused(&error.to_string()))
}

impl Database {
    /// Adopts one explicit source and the next run atomically. The stopped run's
    /// inspection, loss, failure, captured topology and absent anchor stay untouched.
    pub async fn apply_native_close_captured_loss_retry(
        &self,
        assessment: &impl NativeCloseCapturedLossRetryAssessment,
    ) -> LocalAuthorityResult<DbResult<NativeCloseRetryResult>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = Self::apply_native_close_captured_loss_retry_tx(&mut tx, assessment).await;
        match result {
            Ok(value) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(value)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    async fn apply_native_close_captured_loss_retry_tx(
        tx: &mut Transaction<'_, Sqlite>,
        assessment: &impl NativeCloseCapturedLossRetryAssessment,
    ) -> DbResult<NativeCloseRetryResult> {
        let command = assessment.command();
        super::receipts::require_source_and_target_tx(tx, command).await?;
        let NativeCloseIntent::ExplicitRetry {
            target,
            attempt_id,
            failed_run_ordinal,
        } = command.intent()
        else {
            return Err(refused("captured retry requires an explicit retry intent"));
        };
        if let Some(admitted) = super::receipts::read_receipt_tx(tx, command).await? {
            let captured: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1
                FROM native_close_captured_loss_retry_plans WHERE attempt_id = ?1 AND run_ordinal = ?2)")
                .bind(admitted.attempt_id.as_str()).bind(admitted.run_ordinal.get())
                .fetch_one(&mut **tx).await?;
            if !captured {
                return Err(refused("retry receipt belongs to a different origin"));
            }
            return Ok(NativeCloseRetryResult::Replay(NativeCloseReceipt {
                request_id: command.request_id(),
                admitted,
            }));
        }
        let failed_run = CloseRunRef {
            attempt_id: attempt_id.clone(),
            ordinal: *failed_run_ordinal,
        };
        let context =
            Self::read_native_close_captured_loss_retry_context_tx(tx, target, &failed_run).await?;
        if &context != assessment.context() {
            return Err(refused("captured loss origin changed after assessment"));
        }
        Self::require_native_close_retry_members_quiescent_tx(tx, attempt_id).await?;
        let now = chrono::Utc::now().timestamp_micros();
        if assessment.observed_at_us() <= context.failure().occurred_at_us
            || assessment.observed_at_us() > now
            || assessment.safety_evidence().trim().is_empty()
        {
            return Err(refused(
                "captured retry requires fresh safety evidence after the stopped run",
            ));
        }
        let snapshot = fresh_snapshot(&context, assessment.inspection())?;
        let effects = captured_effects(context.scopes())?;
        // Compare complete current scope identities, not merely a locator or worktree id.
        // No current topology is enumerated or substituted for the captured finite set.
        let identities_current: bool = sqlx::query_scalar("SELECT NOT EXISTS (
            SELECT 1 FROM close_attempt_scopes captured
            LEFT JOIN work_scopes live ON live.id = captured.scope
            LEFT JOIN work_scope_environments environment ON environment.work_scope_id = live.id
            WHERE captured.attempt_id = ?1 AND (live.id IS NULL OR live.lifecycle <> 'active'
                OR captured.captured_worktree_identity IS NOT environment.worktree_id
                OR captured.captured_worktree_fingerprint IS NOT environment.worktree_fingerprint
                OR (captured.captured_worktree_locator IS NOT NULL AND
                    captured.captured_worktree_locator IS NOT
                    'git_path_bytes_hex_v1:' || lower(hex(CAST(environment.worktree_path AS BLOB))))))")
            .bind(attempt_id.as_str()).fetch_one(&mut **tx).await?;
        if !identities_current {
            return Err(refused(
                "captured retry current scopes differ from full captured identities",
            ));
        }
        let run = CloseRunRef {
            attempt_id: attempt_id.clone(),
            ordinal: failed_run
                .ordinal
                .checked_next()
                .map_err(|error| refused(&error.to_string()))?,
        };
        let encoded_effects: Vec<_> = effects
            .iter()
            .map(|effect| (&effect.scope, &effect.resource))
            .collect();
        let bytes = serde_json::to_vec(&(
            command.fingerprint(),
            &failed_run,
            &snapshot,
            assessment.observed_at_us(),
            assessment.safety_evidence(),
            encoded_effects,
        ))
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let plan_fingerprint = Sha256::digest(bytes).iter().fold(
            String::from("native-close-captured-loss-plan-v1:"),
            |mut encoded, byte| {
                use std::fmt::Write as _;
                write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                encoded
            },
        );
        sqlx::query(
            "INSERT INTO native_close_captured_loss_retry_plans
            (attempt_id, run_ordinal, origin_generation, origin_fingerprint, observed_at_us,
                inspection_generation, inspection_fingerprint)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(attempt_id.as_str())
        .bind(run.ordinal.get())
        .bind(context.snapshot().generation())
        .bind(context.snapshot().fingerprint())
        .bind(assessment.observed_at_us())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .execute(&mut **tx)
        .await?;
        for scope in &assessment.inspection().scopes {
            sqlx::query("INSERT INTO native_close_captured_loss_retry_inspections
                (attempt_id, run_ordinal, scope, generation, fingerprint) VALUES (?1, ?2, ?3, ?4, ?5)")
                .bind(attempt_id.as_str()).bind(run.ordinal.get()).bind(scope.scope.as_str())
                .bind(scope.snapshot.generation()).bind(scope.snapshot.fingerprint())
                .execute(&mut **tx).await?;
        }
        for (ordinal, effect) in effects.iter().enumerate() {
            let identity = effect.resource.identity();
            let (kind, codec, value) =
                (identity.identity_kind(), identity.codec(), identity.value());
            sqlx::query(
                "INSERT INTO native_close_captured_loss_retry_resources
                (attempt_id, run_ordinal, ordinal, scope, resource_kind, identity_kind,
                    identity_codec, identity_value) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .bind(attempt_id.as_str())
            .bind(run.ordinal.get())
            .bind(ordinal as i64)
            .bind(effect.scope.as_str())
            .bind(effect.resource.kind().as_str())
            .bind(kind)
            .bind(codec)
            .bind(value)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO native_close_retry_plans (attempt_id, run_ordinal, failed_run_ordinal,
            inspection_generation, inspection_fingerprint, plan_fingerprint, request_id,
            request_fingerprint, evidence_kind, effect_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'resource_plan', ?9)",
        )
        .bind(attempt_id.as_str())
        .bind(run.ordinal.get())
        .bind(failed_run.ordinal.get())
        .bind(snapshot.generation())
        .bind(snapshot.fingerprint())
        .bind(plan_fingerprint)
        .bind(command.request_id().to_string())
        .bind(command.fingerprint())
        .bind(
            i64::try_from(effects.len())
                .map_err(|_| refused("captured retry plan is too large"))?,
        )
        .execute(&mut **tx)
        .await?;
        let request = AdmitCloseSafeRetryRequest {
            failed_run,
            requested_by: CloseRetryRequestedBy::User,
            observed_at_us: assessment.observed_at_us(),
            precondition_resolution: "fresh clean finite captured-loss inspection".to_owned(),
            safety_evidence: assessment.safety_evidence().to_owned(),
            remaining_effects: effects,
        };
        Self::insert_close_retry_plan_and_run_tx(tx, &request, &run, now).await?;
        let admitted = NativeCloseRunRef {
            target: target.clone(),
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
