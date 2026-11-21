//! Source-bound initial acceptance and durable execution prerequisites. No physical effects.

use phoenix_core::domain::close::{
    CloseAttemptId, CloseExpectedRetirementResource, CloseObligation, ClosePhase, CloseRunOrdinal,
    CloseStopCertainty, GitPathIdentity, RetiredResourceIdentity, RetiredResourceKind,
    RetirementFailureReason, RetirementOutcome, TranscriptConversationId, WorktreeFingerprint,
    WorktreeId, WorktreeIdentity,
};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::work_scope::WorkScopeId;
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::{NativeCloseCommand, NativeCloseIntent};
use crate::workflow::LocalAuthorityResult;
use crate::{
    CaptureCloseRetirementInventoryRequest, CloseCleanupFailureAuthority,
    CloseCleanupFailureResource, CloseCleanupResourceDisposition, Database, DbError, DbResult,
    RecordCloseRetirementDispatchRequest, RecordCloseRetirementEvidenceRequest,
    ReplaceCloseInspectionRequest, TerminalizeInitialCloseCleanupFailureRequest,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseInitialExecution {
    pub run: NativeCloseRunRef,
    pub phase: phoenix_core::domain::close::ClosePhase,
}

/// One source-owned settlement step, not a terminal Close result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeCloseInitialSettlement {
    WaitingForQuiescence,
    Advanced(NativeCloseInitialExecution),
}

impl Database {
    /// Replaces only the exact source-owned initial inspection; does not dispatch effects.
    pub async fn replace_native_close_initial_inspection(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: ReplaceCloseInspectionRequest,
        empty_generation: Option<&str>,
    ) -> LocalAuthorityResult<DbResult<()>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::AwaitingRetirementInspection,
            )
            .await?;
            if request.attempt_id != admitted.attempt_id {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial inspection request does not match the admitted run".into(),
                ));
            }
            self.replace_close_inspection_tx(&mut tx, &request, empty_generation)
                .await
        }
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

    /// Records intent for one exact anchored initial resource, without performing teardown.
    /// Revalidates live/durable source, receipt, run and sealed inventory even on replay.
    /// Commit or rollback uncertainty remains `DurableFactUnclassified`.
    pub async fn record_native_close_initial_retirement_dispatch(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: RecordCloseRetirementDispatchRequest,
    ) -> LocalAuthorityResult<DbResult<()>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::RetirementRequested,
            )
            .await?;
            if request.attempt_id != admitted.attempt_id {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial dispatch request does not match the admitted run".into(),
                ));
            }
            let anchored: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM close_original_retirement_anchors anchor
                    JOIN close_obligations obligation ON obligation.attempt_id = anchor.attempt_id
                    WHERE anchor.attempt_id = ?1
                      AND anchor.inspection_generation = ?2
                      AND anchor.inspection_fingerprint = ?3
                      AND anchor.inspection_generation = obligation.inspection_generation
                      AND anchor.inspection_fingerprint = obligation.inspection_fingerprint
                ) AND NOT EXISTS (
                    SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = ?1
                )",
            )
            .bind(admitted.attempt_id.as_str())
            .bind(request.snapshot.generation())
            .bind(request.snapshot.fingerprint())
            .fetch_one(&mut *tx)
            .await?;
            if !anchored {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial dispatch requires its exact original retirement anchor".into(),
                ));
            }
            let expected = crate::close_foundation::list_close_expected_retirement_resources_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            // Compare the typed full identity (including worktree fingerprint and locator),
            // not just its encoded dispatch key. Require the whole inventory to be sealed
            // before consulting the insert helper's idempotent replay path.
            if !expected.iter().any(|resource| {
                resource.attempt_id == request.attempt_id
                    && resource.scope == request.scope
                    && resource.snapshot == request.snapshot
                    && resource.resource == request.resource
            }) {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial dispatch resource is not in the exact anchored sealed inventory"
                        .into(),
                ));
            }
            Self::insert_close_retirement_dispatch_tx(&mut tx, &request).await
        }
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

    /// Records successful native initial retirement evidence after exact dispatch intent.
    /// Revalidates source authority before replay and performs no physical retirement.
    /// Residual failures require a separate source-bound terminalization boundary.
    pub async fn record_native_close_initial_retirement_evidence(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: RecordCloseRetirementEvidenceRequest,
    ) -> LocalAuthorityResult<DbResult<()>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::RetirementRequested,
            )
            .await?;
            if request.attempt_id != admitted.attempt_id {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial retirement evidence does not match the admitted run".into(),
                ));
            }
            if !matches!(
                &request.outcome,
                RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. }
            ) {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial success evidence accepts only retired or adopted absence proof".into(),
                ));
            }
            let anchored: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM close_original_retirement_anchors anchor
                    JOIN close_obligations obligation ON obligation.attempt_id = anchor.attempt_id
                    WHERE anchor.attempt_id = ?1
                      AND anchor.inspection_generation = obligation.inspection_generation
                      AND anchor.inspection_fingerprint = obligation.inspection_fingerprint
                ) AND NOT EXISTS (
                    SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = ?1
                ) AND NOT EXISTS (
                    SELECT 1 FROM close_attempt_scopes scope WHERE scope.attempt_id = ?1
                      AND NOT EXISTS (
                        SELECT 1 FROM close_retirement_inventories inventory
                        JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                        WHERE inventory.attempt_id = scope.attempt_id AND inventory.scope = scope.scope
                          AND inventory.sealed = 1
                          AND inventory.inspection_generation = obligation.inspection_generation
                          AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
                      )
                )",
            ).bind(admitted.attempt_id.as_str()).fetch_one(&mut *tx).await?;
            if !anchored {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial evidence requires its exact anchored sealed retirement inventory".into(),
                ));
            }
            let expected = crate::close_foundation::list_close_expected_retirement_resources_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            if !expected.iter().any(|resource| {
                resource.attempt_id == request.attempt_id
                    && resource.scope == request.scope
                    && resource.snapshot == request.snapshot
                    && resource.resource == request.resource
            }) {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial evidence resource is not in the exact anchored sealed inventory".into(),
                ));
            }
            if let RetirementOutcome::AbsenceAdopted { absence_basis } = &request.outcome {
                crate::close_foundation::validate_adopted_absence_evidence(
                    &mut tx,
                    &request,
                    *absence_basis,
                )
                .await?;
            }
            let dispatched: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_retirement_resource_dispatches
                 WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
                   AND inspection_fingerprint = ?4 AND resource_kind = ?5
                   AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8)",
            )
            .bind(request.attempt_id.as_str()).bind(request.scope.as_str())
            .bind(request.snapshot.generation()).bind(request.snapshot.fingerprint())
            .bind(request.resource.kind().as_str()).bind(request.resource.identity().identity_kind())
            .bind(request.resource.identity().codec()).bind(request.resource.identity().value())
            .fetch_one(&mut *tx).await?;
            if !dispatched {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial retirement evidence requires its exact persisted dispatch".into(),
                ));
            }
            Database::record_close_retirement_evidence_sql_tx(
                &mut *tx, &request, request.snapshot.generation(),
            ).await
        }.await;
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

    /// Finalizes only the exact live source-owned initial run after its anchored inventory
    /// has concrete successful retirement proof. This archives; it is not an observation API.
    pub async fn complete_native_close_initial(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::RetirementRequested,
            )
            .await?;
            let anchored: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM close_original_retirement_anchors anchor
                    JOIN close_obligations obligation ON obligation.attempt_id = anchor.attempt_id
                    WHERE anchor.attempt_id = ?1
                      AND anchor.inspection_generation = obligation.inspection_generation
                      AND anchor.inspection_fingerprint = obligation.inspection_fingerprint
                ) AND NOT EXISTS (
                    SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = ?1
                ) AND NOT EXISTS (
                    SELECT 1 FROM close_attempt_scopes scope
                    WHERE scope.attempt_id = ?1 AND NOT EXISTS (
                        SELECT 1 FROM close_retirement_inventories inventory
                        JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                        WHERE inventory.attempt_id = scope.attempt_id
                          AND inventory.scope = scope.scope AND inventory.sealed = 1
                          AND inventory.inspection_generation = obligation.inspection_generation
                          AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
                    )
                )",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if !anchored {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial completion requires its exact anchored sealed retirement inventory".into(),
                ));
            }
            // The completion trigger checks every expected identity for concrete
            // retired/absence-adopted evidence and rejects residual proofs.
            Database::complete_close_retirement_tx(&mut tx, &admitted.attempt_id).await
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

    /// Records a typed inspection failure against the exact live initial source and captured scope.
    pub async fn terminalize_native_close_initial_inspection_failure(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: &WorkScopeId,
        reason: RetirementFailureReason,
        detail: &str,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::AwaitingRetirementInspection,
            )
            .await?;
            if detail.trim().is_empty()
                || !matches!(
                    reason,
                    RetirementFailureReason::IdentityNotProven
                        | RetirementFailureReason::Interrupted
                        | RetirementFailureReason::ManualRepairRequired
                )
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial inspection failure requires a nonempty detail and inspection failure reason".into(),
                ));
            }
            let scope_rows = sqlx::query(
                "SELECT scope, captured_worktree_identity, captured_worktree_fingerprint,
                        captured_worktree_locator FROM close_attempt_scopes
                 WHERE attempt_id = ?1 ORDER BY scope",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_all(&mut *tx)
            .await?;
            if !scope_rows.iter().any(|row| {
                row.try_get::<String, _>("scope")
                    .is_ok_and(|captured| captured == scope.as_str())
            }) {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial inspection failure scope is not captured by the admitted run".into(),
                ));
            }
            let work_scope_resource = |captured: &WorkScopeId| {
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::WorkScope,
                    phoenix_core::domain::close::LossItemIdentity::Opaque(
                        phoenix_core::domain::close::OpaqueIdentity::parse(captured.as_str())
                            .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                    ),
                )
                .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))
            };
            let authority_resource = work_scope_resource(scope)?;
            let mut remaining_resources = Vec::new();
            for row in scope_rows {
                let captured_scope = WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?;
                remaining_resources.push(CloseCleanupFailureResource {
                    scope: captured_scope.clone(),
                    resource: work_scope_resource(&captured_scope)?,
                    disposition: CloseCleanupResourceDisposition::Unknown,
                });
                let captured_identity = (
                    row.try_get::<Option<String>, _>("captured_worktree_identity")?,
                    row.try_get::<Option<String>, _>("captured_worktree_fingerprint")?,
                    row.try_get::<Option<String>, _>("captured_worktree_locator")?,
                );
                if let (Some(id), Some(fingerprint), Some(locator)) = captured_identity {
                    let identity = WorktreeIdentity::from_parts(
                        WorktreeId::parse(id)
                            .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                        WorktreeFingerprint::parse(fingerprint)
                            .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                        GitPathIdentity::decode_exact(&locator)
                            .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                    );
                    remaining_resources.push(CloseCleanupFailureResource {
                        scope: captured_scope,
                        resource: RetiredResourceIdentity::parse(
                            RetiredResourceKind::Worktree,
                            phoenix_core::domain::close::LossItemIdentity::Worktree(identity),
                        )
                        .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                        disposition: CloseCleanupResourceDisposition::Unknown,
                    });
                }
                // An unresolved captured identity has no exact Worktree resource to
                // assert. Its independent captured WorkScope remains Unknown above.
            }
            let run = phoenix_core::domain::close::CloseRunRef {
                attempt_id: admitted.attempt_id.clone(),
                ordinal: CloseRunOrdinal::INITIAL,
            };
            let request = TerminalizeInitialCloseCleanupFailureRequest {
                failure_occurrence_id: run.failure_occurrence_id(),
                attempt_id: admitted.attempt_id,
                source_product_conversation_id: receipt.admitted.target.clone(),
                authority: CloseCleanupFailureAuthority::CapturedScope {
                    scope: scope.clone(),
                    resource: authority_resource,
                },
                remaining_resources,
                reason,
                detail: detail.to_owned(),
                stop_certainty: CloseStopCertainty::ShutdownUncertain,
                occurred_at_us: chrono::Utc::now().timestamp_micros(),
            };
            Database::terminalize_close_run_cleanup_failure_tx(&mut tx, &run, &request).await
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

    /// Captures only the exact source-owned initial retirement inventory.
    pub async fn capture_native_close_initial_retirement_inventory(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: CaptureCloseRetirementInventoryRequest,
    ) -> LocalAuthorityResult<DbResult<Vec<CloseExpectedRetirementResource>>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::RetirementRequested,
            )
            .await?;
            if request.attempt_id != admitted.attempt_id {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial inventory request does not match the admitted run".into(),
                ));
            }
            self.capture_close_retirement_inventory_tx(&mut tx, &request)
                .await
        }
        .await;
        match result {
            Ok(resources) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(resources)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error @ DbError::CloseFoundationRepairRequired(_)) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }

    /// Prepares execution of the one exact newly admitted initial run.
    ///
    /// `live_source` must be the trusted source snapshot held by the source actor;
    /// command correlation alone is not authority. This prerequisite deliberately
    /// performs only the blocker decision phase transition and is not runtime-wired.
    pub async fn prepare_native_close_initial_execution(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> LocalAuthorityResult<DbResult<NativeCloseInitialExecution>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        match prepare_initial_execution_tx(&mut tx, command, receipt, live_source).await {
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

    /// Checks one exact source-owned initial settlement boundary without polling.
    /// Only captured durable work that has not quiesced yields `WaitingForQuiescence`;
    /// stale source, receipt, run, or phase remains an error after classified rollback.
    /// Waiting may commit reconciled exact-turn receipts, but dispatches no effects.
    pub async fn try_settle_native_close_initial(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> LocalAuthorityResult<DbResult<NativeCloseInitialSettlement>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                phoenix_core::domain::close::ClosePhase::SettlingActiveWork,
            )
            .await?;
            let pending_direct = Database::reconcile_close_direct_turn_settlement_receipts_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            let pending_participants = Database::count_close_settlement_active_participants_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            if pending_direct != 0 || pending_participants != 0 {
                return Ok(NativeCloseInitialSettlement::WaitingForQuiescence);
            }
            let obligation = Database::advance_close_settlement_when_quiescent_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            Ok(NativeCloseInitialSettlement::Advanced(
                NativeCloseInitialExecution {
                    run: admitted,
                    phase: obligation.phase(),
                },
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

    /// Advances only the exact source-owned initial run after durable work is quiescent.
    /// No effects, cancellation, blocker, retry, or historical-source fallback is accepted.
    /// Errors are classified only after rollback succeeds; commit/rollback uncertainty
    /// stays unclassified. An arbitrary precondition error is not permission to continue.
    pub async fn settle_native_close_initial(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> LocalAuthorityResult<DbResult<NativeCloseInitialExecution>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        match settle_initial_tx(&mut tx, command, receipt, live_source).await {
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

async fn prepare_initial_execution_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
) -> DbResult<NativeCloseInitialExecution> {
    let (admitted, busy) = require_initial_execution_tx(
        tx,
        command,
        receipt,
        live_source,
        phoenix_core::domain::close::ClosePhase::AwaitingBlockerResolution,
    )
    .await?;
    let next_phase = if busy {
        phoenix_core::domain::close::ClosePhase::AwaitingStopWorkConfirmation
    } else {
        phoenix_core::domain::close::ClosePhase::SettlingActiveWork
    };
    super::super::close_foundation::set_close_phase_tx(
        tx,
        admitted.attempt_id.as_str(),
        next_phase,
    )
    .await?;
    if !busy {
        Database::capture_close_direct_turn_settlement_targets_tx(tx, admitted.attempt_id.as_str())
            .await?;
    }
    Ok(NativeCloseInitialExecution {
        run: admitted,
        phase: next_phase,
    })
}

async fn settle_initial_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
) -> DbResult<NativeCloseInitialExecution> {
    let (admitted, _) = require_initial_execution_tx(
        tx,
        command,
        receipt,
        live_source,
        phoenix_core::domain::close::ClosePhase::SettlingActiveWork,
    )
    .await?;
    let obligation =
        Database::advance_close_settlement_when_quiescent_tx(tx, admitted.attempt_id.as_str())
            .await?;
    Ok(NativeCloseInitialExecution {
        run: admitted,
        phase: obligation.phase(),
    })
}

#[allow(clippy::too_many_lines)] // Live source, durable receipt and exact run share one boundary.
async fn require_initial_execution_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
    expected_phase: phoenix_core::domain::close::ClosePhase,
) -> DbResult<(NativeCloseRunRef, bool)> {
    let NativeCloseIntent::Initial { target } = command.intent() else {
        return Err(DbError::CloseFoundationPrecondition(
            "expected initial Close intent".into(),
        ));
    };
    crate::task_approval_inspection::require_executing_source(
        live_source,
        command.source(),
        "close_conversation",
        &command.intent().source_input(),
    )
    .map_err(|_| {
        DbError::CloseFoundationPrecondition("Close requires its current executing source".into())
    })?;

    if receipt.request_id != command.request_id()
        || receipt.admitted.target != *target
        || receipt.admitted.run_ordinal != CloseRunOrdinal::INITIAL
    {
        return Err(DbError::CloseFoundationPrecondition(
            "initial Close receipt does not match the command".into(),
        ));
    }
    // Validate durable source/target, then prove the caller's exact receipt was retained.
    super::receipts::require_source_and_target_tx(tx, command).await?;
    let admitted = super::receipts::read_receipt_tx(tx, command)
        .await?
        .ok_or_else(|| {
            DbError::CloseFoundationPrecondition("exact initial Close receipt is missing".into())
        })?;
    if admitted != receipt.admitted
        || admitted.target != *target
        || admitted.run_ordinal != CloseRunOrdinal::INITIAL
    {
        return Err(DbError::CloseFoundationPrecondition(
            "initial Close receipt does not bind the initial run".into(),
        ));
    }

    let row = sqlx::query(
        "SELECT o.phase, o.native_managed, r.run_ordinal, r.status,
                (SELECT MAX(latest.run_ordinal) FROM close_runs latest WHERE latest.attempt_id = o.attempt_id) AS latest_ordinal,
                (SELECT latest.status FROM close_runs latest WHERE latest.attempt_id = o.attempt_id
                   ORDER BY latest.run_ordinal DESC LIMIT 1) AS latest_status,
                (SELECT member.captured_state_kind FROM close_attempt_members member
                   WHERE member.attempt_id = o.attempt_id AND member.member_role IN ('latest', 'root_latest')) AS captured_state_kind
         FROM close_obligations o JOIN close_runs r ON r.attempt_id = o.attempt_id AND r.run_ordinal = ?2
         WHERE o.attempt_id = ?1 AND o.product_conversation_id = ?3",
    )
    .bind(admitted.attempt_id.as_str())
    .bind(CloseRunOrdinal::INITIAL.get())
    .bind(target.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| DbError::CloseFoundationPrecondition(
        "exact admitted initial Close run is missing or mismatched".into(),
    ))?;
    let phase_raw: String = row.try_get("phase")?;
    let phase = phoenix_core::domain::close::ClosePhase::from_db_str(&phase_raw)
        .ok_or_else(|| DbError::Serialization("unknown close phase".into()))?;
    let managed: i64 = row.try_get("native_managed")?;
    let run_status: String = row.try_get("status")?;
    let latest_ordinal: Option<i64> = row.try_get("latest_ordinal")?;
    let latest_status: Option<String> = row.try_get("latest_status")?;
    let captured_state: Option<String> = row.try_get("captured_state_kind")?;
    if managed != 1
        || phase != expected_phase
        || run_status != "running"
        || latest_ordinal != Some(CloseRunOrdinal::INITIAL.get())
        || latest_status.as_deref() != Some("running")
    {
        return Err(DbError::CloseFoundationPrecondition(
            "initial Close operation is stale or has advanced".into(),
        ));
    }
    let captured_state = captured_state.ok_or_else(|| {
        DbError::CloseFoundationPrecondition("initial Close has no captured latest state".into())
    })?;
    let busy =
        phoenix_core::domain::close::CapturedConversationStateKind::from_db_str(&captured_state)
            .ok_or_else(|| {
                DbError::Serialization("unknown captured conversation state kind".into())
            })?
            .is_busy();
    Ok((admitted, busy))
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
