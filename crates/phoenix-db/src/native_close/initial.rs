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
use crate::close_foundation::ensure_close_retirement_loss_free_tx;
use crate::workflow::LocalAuthorityResult;
use crate::{
    BindCloseWorktreeFinalTombstoneObjectRequest, BindCloseWorktreeFinalTombstoneRequest,
    CaptureCloseRetirementInventoryRequest, CloseCleanupFailureAuthority,
    CloseCleanupFailureResource, CloseCleanupResourceDisposition, Database, DbError, DbResult,
    RecordCloseRetirementDispatchRequest, RecordCloseRetirementEvidenceRequest,
    RecordCloseWorktreeCleanupPlanRequest, ReplaceCloseInspectionRequest,
    TerminalizeInitialCloseCleanupFailureRequest, WorkScopeRetirementBlocker,
    WorkScopeRetirementOutcome, WorkScopeRetirementPrecondition,
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

/// Snapshot authorization for one initial process-kind launch.
///
/// This token alone does not enforce single launch: it is minted against a
/// transaction-time snapshot, and repeated pre-effect checks are allowed. The
/// manager must consume the requested kind and serialize source revocation at
/// the actual launch sink. This database boundary performs no physical effects.
/// Fields are private so only the database source fence can mint one.
#[derive(Debug, PartialEq, Eq)]
pub struct NativeCloseInitialProcessAuthorization {
    run: NativeCloseRunRef,
    scope: WorkScopeId,
    snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
    kind: RetiredResourceKind,
    process_inventory: Vec<phoenix_core::domain::close::CloseExpectedRetirementResource>,
}

/// Synchronous handoff result; this closed enum cannot represent a future.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCloseInitialResourceEntryDecision {
    Entered,
    Refused,
}

#[derive(Debug)]
pub enum NativeCloseInitialEntryRelease {
    Confirmed,
    Unconfirmed(DbError),
}

#[must_use = "entry decisions and unconfirmed transaction release must be supervised"]
#[derive(Debug)]
pub enum NativeCloseInitialResourceEntryResult {
    NotStarted(DbError),
    Rejected {
        error: DbError,
        release: NativeCloseInitialEntryRelease,
    },
    Invoked {
        decision: NativeCloseInitialResourceEntryDecision,
        release: NativeCloseInitialEntryRelease,
    },
}

/// A returned refusal proves the first-effect callback was not invoked.
/// Cancellation or panic returns no such proof.
#[must_use = "unconfirmed transaction release must remain supervised"]
#[derive(Debug)]
pub enum NativeCloseInitialFirstEffectError {
    NotStarted(DbError),
    Rejected {
        error: DbError,
        release: NativeCloseInitialEntryRelease,
    },
}

impl NativeCloseInitialProcessAuthorization {
    /// Consume the authorization at the manager sink; it cannot be cloned or replayed.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        NativeCloseRunRef,
        WorkScopeId,
        phoenix_core::domain::close::CloseRetirementSnapshot,
        RetiredResourceKind,
        Vec<phoenix_core::domain::close::CloseExpectedRetirementResource>,
    ) {
        (
            self.run,
            self.scope,
            self.snapshot,
            self.kind,
            self.process_inventory,
        )
    }
}

fn is_initial_process_kind(kind: RetiredResourceKind) -> bool {
    matches!(
        kind,
        RetiredResourceKind::BashProcessGroup
            | RetiredResourceKind::TmuxServer
            | RetiredResourceKind::PtySession
            | RetiredResourceKind::BrowserSession
    )
}

// Bind the sealed inventory, dispatches, and absence of prior success to one writer snapshot.
#[allow(clippy::too_many_lines)]
async fn validate_initial_process_step_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
    scope: WorkScopeId,
    snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
    resource: &RetiredResourceIdentity,
) -> DbResult<NativeCloseInitialProcessAuthorization> {
    let kind = resource.kind();
    let result = async {
        if !is_initial_process_kind(kind) {
            return Err(DbError::CloseFoundationPrecondition(
                "initial process authorization requires a process resource kind".into(),
            ));
        }
        let (admitted, _) = require_initial_execution_tx(
            tx, command, receipt, live_source, ClosePhase::RetirementRequested,
        ).await?;
        let anchored: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM close_original_retirement_anchors anchor
                JOIN close_obligations obligation ON obligation.attempt_id = anchor.attempt_id
                WHERE anchor.attempt_id = ?1
                  AND anchor.inspection_generation = obligation.inspection_generation
                  AND anchor.inspection_fingerprint = obligation.inspection_fingerprint
            ) AND NOT EXISTS (
                SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = ?1
            ) AND EXISTS (
                SELECT 1 FROM close_attempt_scopes captured
                JOIN close_retirement_inventories inventory
                  ON inventory.attempt_id = captured.attempt_id AND inventory.scope = captured.scope
                JOIN close_obligations obligation ON obligation.attempt_id = captured.attempt_id
                WHERE captured.attempt_id = ?1 AND captured.scope = ?2
                  AND inventory.sealed = 1
                  AND inventory.inspection_generation = ?3
                  AND inventory.inspection_fingerprint = ?4
                  AND inventory.inspection_generation = obligation.inspection_generation
                  AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
            ) AND NOT EXISTS (
                SELECT 1 FROM close_attempt_scopes scope
                WHERE scope.attempt_id = ?1 AND NOT EXISTS (
                    SELECT 1 FROM close_retirement_inventories inventory
                    JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                    WHERE inventory.attempt_id = scope.attempt_id AND inventory.scope = scope.scope
                      AND inventory.sealed = 1
                      AND inventory.inspection_generation = obligation.inspection_generation
                      AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
                )
            )",
        ).bind(admitted.attempt_id.as_str()).bind(scope.as_str())
            .bind(snapshot.generation()).bind(snapshot.fingerprint())
            .fetch_one(&mut **tx).await?;
        if !anchored {
            return Err(DbError::CloseFoundationPrecondition(
                "initial process authorization requires the original anchored sealed scope inventory".into(),
            ));
        }
        let expected = crate::close_foundation::list_close_expected_retirement_resources_tx(
            tx, admitted.attempt_id.as_str(),
        ).await?;
        if expected.iter().any(|item| item.scope == scope && item.snapshot != snapshot) {
            return Err(DbError::CloseFoundationPrecondition(
                "initial process authorization snapshot differs from original inventory".into(),
            ));
        }
        let exact = expected.iter().find(|item| item.attempt_id == admitted.attempt_id
            && item.scope == scope && item.snapshot == snapshot
            && item.resource == *resource).cloned().ok_or_else(|| {
                DbError::CloseFoundationPrecondition(
                    "initial process entry requires the exact resource in the original scoped inventory".into(),
                )
            })?;
        let process_inventory = vec![exact];
        for item in &process_inventory {
            let dispatched: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_retirement_resource_dispatches
                 WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
                   AND inspection_fingerprint = ?4 AND resource_kind = ?5
                   AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8)",
            ).bind(item.attempt_id.as_str()).bind(item.scope.as_str())
                .bind(item.snapshot.generation()).bind(item.snapshot.fingerprint())
                .bind(item.resource.kind().as_str()).bind(item.resource.identity().identity_kind())
                .bind(item.resource.identity().codec()).bind(item.resource.identity().value())
                .fetch_one(&mut **tx).await?;
            if !dispatched {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process launch requires the exact resource dispatch to be durable".into(),
                ));
            }
        }
        for item in &process_inventory {
            let successful_process_receipt: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_process_step_successes
                 WHERE attempt_id = ?1 AND run_ordinal = ?2 AND scope = ?3
                   AND resource_kind = ?4 AND identity_value = ?5)",
            ).bind(admitted.attempt_id.as_str()).bind(admitted.run_ordinal.get())
                .bind(item.scope.as_str()).bind(item.resource.kind().as_str())
                .bind(item.resource.identity().value())
                .fetch_one(&mut **tx).await?;
            let positive_retirement_evidence: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_retirement_resource_history
                 WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
                   AND inspection_fingerprint = ?4 AND resource_kind = ?5
                   AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8
                   AND proof_kind IN ('retired', 'absence_adopted'))",
            ).bind(item.attempt_id.as_str()).bind(item.scope.as_str())
                .bind(item.snapshot.generation()).bind(item.snapshot.fingerprint())
                .bind(item.resource.kind().as_str()).bind(item.resource.identity().identity_kind())
                .bind(item.resource.identity().codec()).bind(item.resource.identity().value())
                .fetch_one(&mut **tx).await?;
            if successful_process_receipt || positive_retirement_evidence {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process authorization refuses an exact resource with prior success or positive retirement evidence".into(),
                ));
            }
        }
        Ok(NativeCloseInitialProcessAuthorization {
            run: admitted, scope, snapshot, kind, process_inventory,
        })
    }.await;
    result
}

enum InitialWorktreeMetadata {
    Plan(RecordCloseWorktreeCleanupPlanRequest),
    Tombstone(BindCloseWorktreeFinalTombstoneRequest),
    TombstoneObject(BindCloseWorktreeFinalTombstoneObjectRequest),
}

async fn require_initial_original_dispatch_tx(
    tx: &mut Transaction<'_, Sqlite>,
    admitted: &NativeCloseRunRef,
    scope: &WorkScopeId,
    snapshot: &phoenix_core::domain::close::CloseRetirementSnapshot,
    resource: &RetiredResourceIdentity,
) -> DbResult<Vec<CloseExpectedRetirementResource>> {
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
        ) AND NOT EXISTS (
            SELECT 1 FROM close_attempt_scopes captured
            WHERE captured.attempt_id = ?1 AND NOT EXISTS (
                SELECT 1 FROM close_retirement_inventories inventory
                JOIN close_obligations obligation ON obligation.attempt_id = inventory.attempt_id
                WHERE inventory.attempt_id = captured.attempt_id AND inventory.scope = captured.scope
                  AND inventory.sealed = 1
                  AND inventory.inspection_generation = obligation.inspection_generation
                  AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
            )
        )",
    )
    .bind(admitted.attempt_id.as_str())
    .bind(snapshot.generation())
    .bind(snapshot.fingerprint())
    .fetch_one(&mut **tx)
    .await?;
    if !anchored {
        return Err(DbError::CloseFoundationPrecondition(
            "initial metadata requires the exact original anchored sealed inventory".into(),
        ));
    }
    let expected = crate::close_foundation::list_close_expected_retirement_resources_tx(
        tx,
        admitted.attempt_id.as_str(),
    )
    .await?;
    if !expected.iter().any(|item| {
        item.attempt_id == admitted.attempt_id
            && item.scope == *scope
            && item.snapshot == *snapshot
            && item.resource == *resource
    }) {
        return Err(DbError::CloseFoundationPrecondition(
            "initial metadata resource is not in the exact original inventory".into(),
        ));
    }
    let identity = resource.identity();
    let dispatched: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_retirement_resource_dispatches
         WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
           AND inspection_fingerprint = ?4 AND resource_kind = ?5
           AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8)",
    )
    .bind(admitted.attempt_id.as_str())
    .bind(scope.as_str())
    .bind(snapshot.generation())
    .bind(snapshot.fingerprint())
    .bind(resource.kind().as_str())
    .bind(identity.identity_kind())
    .bind(identity.codec())
    .bind(identity.value())
    .fetch_one(&mut **tx)
    .await?;
    if !dispatched {
        return Err(DbError::CloseFoundationPrecondition(
            "initial metadata requires exact persisted dispatch membership".into(),
        ));
    }
    Ok(expected)
}

async fn validate_initial_worktree_entry_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
    scope: &WorkScopeId,
    snapshot: &phoenix_core::domain::close::CloseRetirementSnapshot,
    resource: &RetiredResourceIdentity,
) -> DbResult<()> {
    let (admitted, _) = require_initial_execution_tx(
        tx,
        command,
        receipt,
        live_source,
        ClosePhase::RetirementRequested,
    )
    .await?;
    if resource.kind() != RetiredResourceKind::Worktree {
        return Err(DbError::CloseFoundationPrecondition(
            "initial worktree entry requires an exact worktree resource".into(),
        ));
    }
    require_initial_original_dispatch_tx(tx, &admitted, scope, snapshot, resource).await?;
    let identity = resource.identity();
    let already_proven: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_retirement_resource_history
         WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
           AND inspection_fingerprint = ?4 AND resource_kind = ?5
           AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8
           AND proof_kind IN ('retired', 'absence_adopted'))
         OR EXISTS(SELECT 1 FROM close_retirement_resources
         WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
           AND inspection_fingerprint = ?4 AND resource_kind = ?5
           AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8
           AND proof_kind IN ('retired', 'absence_adopted'))",
    )
    .bind(admitted.attempt_id.as_str())
    .bind(scope.as_str())
    .bind(snapshot.generation())
    .bind(snapshot.fingerprint())
    .bind(resource.kind().as_str())
    .bind(identity.identity_kind())
    .bind(identity.codec())
    .bind(identity.value())
    .fetch_one(&mut **tx)
    .await?;
    if already_proven {
        return Err(DbError::CloseFoundationPrecondition(
            "initial worktree entry refuses an exact resource with positive retirement evidence"
                .into(),
        ));
    }
    Ok(())
}

impl Database {
    /// Runs a bounded synchronous real first-effect callback under the same live and
    /// durable source, exact-run, original-dispatch, exact-resource and not-completed
    /// validation as `enter_native_close_initial_worktree_resource`.
    ///
    /// The `BEGIN IMMEDIATE` writer reservation remains held until the callback returns
    /// and rollback is attempted. The callback must not perform database work, wait for
    /// another operation that needs this database's writer, or await; any such work can
    /// deadlock behind this reservation. Keep the callback bounded and synchronous.
    /// This is the live first-effect entry path; it performs no physical effect itself.
    ///
    /// The callback's `T` is retained only in this future until rollback has been
    /// awaited. Callers must therefore run this future under an external,
    /// non-cancelled supervisor and independently retain any callback outcome
    /// needed for recovery. If this future is cancelled or panics, callback
    /// invocation and rollback classification are unknown: do not infer that the
    /// callback was not invoked and do not retry it on that basis. This API makes
    /// no cancellation-safety guarantee. `Err` means only `NotStarted` or
    /// `Rejected`; `Ok` means only that the callback returned, not that a physical
    /// effect succeeded. This seam does not prevent replay; an external affine
    /// owner is mandatory to ensure at-most-once entry across repeated calls or
    /// lost responses. Receipt replay is not an ownership mechanism.
    ///
    /// # Errors
    /// Returns `NotStarted` if the writer reservation cannot be acquired, or
    /// `Rejected` with the rollback classification if validation fails before entry.
    // The initial receipt, live source, and exact worktree identity are separate from the callback.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_native_close_initial_worktree_first_effect<F, T>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        first_effect: F,
    ) -> Result<(T, NativeCloseInitialEntryRelease), NativeCloseInitialFirstEffectError>
    where
        F: FnOnce() -> T,
    {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return Err(NativeCloseInitialFirstEffectError::NotStarted(error.into())),
        };
        if let Err(error) = validate_initial_worktree_entry_tx(
            &mut tx,
            command,
            receipt,
            live_source,
            &scope,
            &snapshot,
            resource,
        )
        .await
        {
            let release = match tx.rollback().await {
                Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
                Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
            };
            return Err(NativeCloseInitialFirstEffectError::Rejected { error, release });
        }
        let result = first_effect();
        let release = match tx.rollback().await {
            Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
            Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
        };
        Ok((result, release))
    }

    /// Validates and synchronously hands off one exact initial process resource while
    /// holding `SQLite`'s writer reservation. The callback must only perform a short,
    /// nonblocking in-memory handoff of an already selected exact owner: no I/O, locks,
    /// SQL, or waits. Any entered ownership must be stored externally before returning;
    /// later cleanup must not wait while this database transaction is held. This leaf
    /// does not solve cancellation/supervision or integrate physical effects.
    /// Cancellation during release can prevent any result after the handoff ran:
    /// absence of an `Invoked` return is not proof of non-entry and cannot justify
    /// retry. The externally retained entered owner must remain supervised.
    // Pass the initial receipt, live source, and exact process identity to the affine handoff gate.
    #[allow(clippy::too_many_arguments)]
    pub async fn enter_native_close_initial_process_resource<F>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        handoff: F,
    ) -> NativeCloseInitialResourceEntryResult
    where
        F: FnOnce() -> NativeCloseInitialResourceEntryDecision,
    {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return NativeCloseInitialResourceEntryResult::NotStarted(error.into()),
        };
        let validation = validate_initial_process_step_tx(
            &mut tx,
            command,
            receipt,
            live_source,
            scope,
            snapshot,
            resource,
        )
        .await;
        if let Err(error) = validation {
            let release = match tx.rollback().await {
                Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
                Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
            };
            return NativeCloseInitialResourceEntryResult::Rejected { error, release };
        }
        let decision = handoff();
        let release = match tx.rollback().await {
            Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
            Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
        };
        NativeCloseInitialResourceEntryResult::Invoked { decision, release }
    }

    /// Runs a synchronous callback only after validating the exact initial process
    /// resource against live and durable source authority, its original sealed scope
    /// and snapshot, durable dispatch, resource kind, and absence of prior success or
    /// positive retirement evidence. The callback runs while `BEGIN IMMEDIATE` is held,
    /// before rollback is attempted.
    ///
    /// This makes no cancellation-safety guarantee. The callback outcome is retained
    /// only in this future; callers must supervise it externally and independently
    /// retain any outcome needed for recovery. Cancellation or panic after callback
    /// entry does not prove non-invocation, and an absent return cannot justify retry.
    /// The callback must be a bounded, nonwaiting leaf: no full teardown, async wait,
    /// database recursion, or conflicting source-lock acquisition. The copied
    /// `live_source` is evidence, not independently synchronized actor authority;
    /// the caller must retain current source ownership across the real first effect.
    /// External retained outcomes remain caller-owned. This API does not integrate
    /// runtime or terminal effects.
    ///
    /// # Errors
    /// Returns `NotStarted` if the writer reservation cannot be acquired, or
    /// `Rejected` with the rollback classification if validation fails before entry.
    // The initial receipt, live source, and exact process identity are separate from the callback.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_native_close_initial_process_first_effect<F, T>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        first_effect: F,
    ) -> Result<(T, NativeCloseInitialEntryRelease), NativeCloseInitialFirstEffectError>
    where
        F: FnOnce() -> T,
    {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return Err(NativeCloseInitialFirstEffectError::NotStarted(error.into())),
        };
        if let Err(error) = validate_initial_process_step_tx(
            &mut tx,
            command,
            receipt,
            live_source,
            scope,
            snapshot,
            resource,
        )
        .await
        {
            let release = match tx.rollback().await {
                Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
                Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
            };
            return Err(NativeCloseInitialFirstEffectError::Rejected { error, release });
        }
        let result = first_effect();
        let release = match tx.rollback().await {
            Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
            Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
        };
        Ok((result, release))
    }

    /// Fence one exact initial worktree effect with the source and original dispatch
    /// while holding `SQLite`'s writer reservation. `handoff` is a one-shot affine
    /// ownership transfer, not a retryable check: it must synchronously install
    /// its owner under external supervision before returning `Entered`, without
    /// I/O, locks, SQL, waits, or a future. The caller must supervise that owner
    /// through release (including cancellation or an unconfirmed rollback); a
    /// missing `Invoked` result after handoff does not establish non-entry.
    /// This read-only writer reservation does not persist an entry marker. Before
    /// positive evidence exists, repeated calls can reach `handoff`; its retained
    /// affine owner must return `Refused` after its first transfer, even if the
    /// first caller lost its response. A callback that mints a fresh owner per
    /// call is invalid. The writer fence is not a replacement for owner custody.
    /// Receipt replay grants no new physical authority or permission to reenter
    /// an already proven effect. This database boundary performs no physical work.
    // Pass the initial receipt, live source, and exact worktree identity to the affine handoff gate.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) async fn enter_native_close_initial_worktree_resource<F>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        handoff: F,
    ) -> NativeCloseInitialResourceEntryResult
    where
        F: FnOnce() -> NativeCloseInitialResourceEntryDecision,
    {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return NativeCloseInitialResourceEntryResult::NotStarted(error.into()),
        };
        let validation = validate_initial_worktree_entry_tx(
            &mut tx,
            command,
            receipt,
            live_source,
            &scope,
            &snapshot,
            resource,
        )
        .await;
        if let Err(error) = validation {
            let release = match tx.rollback().await {
                Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
                Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
            };
            return NativeCloseInitialResourceEntryResult::Rejected { error, release };
        }
        let decision = handoff();
        let release = match tx.rollback().await {
            Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
            Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
        };
        NativeCloseInitialResourceEntryResult::Invoked { decision, release }
    }

    /// Records a cleanup plan only if the original source still enters inside
    /// the writer transaction, after exact receipt and dispatch validation.
    ///
    /// `entry` must be a short, nonblocking in-memory gate: no I/O, SQL, waits,
    /// or nested writer. Do not hold a source lock while awaiting this method.
    /// Its error refuses the write. Its successful lease is retained across the
    /// metadata mutation and commit/rollback, then returned with the classification
    /// (also on unknown release). The caller must retain that lease on unclassified
    /// release and independently supervise cancellation/panic; a callback result
    /// alone does not prove writer release. No `Send` bound is required for a
    /// borrowed thread-local source guard used with synchronous `block_on`.
    pub async fn record_native_close_initial_worktree_cleanup_plan_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: RecordCloseWorktreeCleanupPlanRequest,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        self.write_native_close_initial_worktree_metadata(
            command,
            receipt,
            live_source,
            request.attempt_id.clone(),
            request.scope.clone(),
            request.snapshot.clone(),
            request.resource.clone(),
            InitialWorktreeMetadata::Plan(request),
            entry,
        )
        .await
    }

    /// Entry-gated final-root binding. The callback and supervision contract is
    /// identical to `record_native_close_initial_worktree_cleanup_plan_with_entry`.
    pub async fn bind_native_close_initial_worktree_final_tombstone_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: BindCloseWorktreeFinalTombstoneRequest,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        self.write_native_close_initial_worktree_metadata(
            command,
            receipt,
            live_source,
            request.attempt_id.clone(),
            request.scope.clone(),
            request.snapshot.clone(),
            request.resource.clone(),
            InitialWorktreeMetadata::Tombstone(request),
            entry,
        )
        .await
    }

    /// Entry-gated final-object binding. The callback and supervision contract is
    /// identical to `record_native_close_initial_worktree_cleanup_plan_with_entry`.
    pub async fn bind_native_close_initial_worktree_final_tombstone_object_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: BindCloseWorktreeFinalTombstoneObjectRequest,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        self.write_native_close_initial_worktree_metadata(
            command,
            receipt,
            live_source,
            request.attempt_id.clone(),
            request.scope.clone(),
            request.snapshot.clone(),
            request.resource.clone(),
            InitialWorktreeMetadata::TombstoneObject(request),
            entry,
        )
        .await
    }

    // Validate the dispatch key before taking the entry lease and consuming the metadata request.
    #[allow(clippy::too_many_arguments)]
    async fn write_native_close_initial_worktree_metadata<F, L>(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        attempt_id: CloseAttemptId,
        scope: WorkScopeId,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        resource: RetiredResourceIdentity,
        metadata: InitialWorktreeMetadata,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => {
                return (
                    LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
                    None,
                );
            }
        };
        let mut lease = None;
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx,
                command,
                receipt,
                live_source,
                ClosePhase::RetirementRequested,
            )
            .await?;
            if attempt_id != admitted.attempt_id || resource.kind() != RetiredResourceKind::Worktree
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial worktree metadata must match its admitted run and worktree resource"
                        .into(),
                ));
            }
            require_initial_original_dispatch_tx(&mut tx, &admitted, &scope, &snapshot, &resource)
                .await?;
            // Lock order is SQLite writer -> short source-entry callback. The
            // callback must not wait, perform I/O or SQL, or open another writer.
            // A refusal rolls back without touching any metadata. Successful
            // source custody spans the SQL mutation and transaction release.
            lease = Some(entry()?);
            match metadata {
                InitialWorktreeMetadata::Plan(request) => {
                    Self::record_close_worktree_cleanup_plan_tx(&mut tx, request).await
                }
                InitialWorktreeMetadata::Tombstone(request) => {
                    Self::bind_close_worktree_final_tombstone_tx(&mut tx, request).await
                }
                InitialWorktreeMetadata::TombstoneObject(request) => {
                    Self::bind_close_worktree_final_tombstone_object_tx(&mut tx, request).await
                }
            }
        }
        .await;
        let classification = match result {
            Ok(()) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(())),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        };
        // In particular, an unknown release must not drop source custody here.
        (classification, lease)
    }

    /// Retires one exact original `WorkScope` in the same transaction as its success proof.
    // Validate the exact scoped dispatch and child proofs before writing retirement and its proof atomically.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn retire_work_scope_for_native_close_initial(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        resource: RetiredResourceIdentity,
        precondition: WorkScopeRetirementPrecondition,
        reason: &str,
    ) -> LocalAuthorityResult<DbResult<WorkScopeRetirementOutcome>> {
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
            if resource.kind() != RetiredResourceKind::WorkScope
                || precondition.scope_id() != &scope
                || !matches!(resource.identity(),
                    phoenix_core::domain::close::LossItemIdentity::Opaque(identity)
                        if identity.as_str() == scope.as_str())
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial WorkScope retirement requires its exact opaque scope identity".into(),
                ));
            }
            let expected = require_initial_original_dispatch_tx(
                &mut tx, &admitted, &scope, &snapshot, &resource,
            )
            .await?;
            for item in expected.iter().filter(|item| {
                item.scope == scope && item.resource.kind() != RetiredResourceKind::WorkScope
            }) {
                let identity = item.resource.identity();
                let proven: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM close_retirement_resources
                     WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
                       AND inspection_fingerprint = ?4 AND resource_kind = ?5
                       AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8
                       AND proof_kind IN ('retired', 'absence_adopted'))",
                )
                .bind(admitted.attempt_id.as_str())
                .bind(item.scope.as_str())
                .bind(item.snapshot.generation())
                .bind(item.snapshot.fingerprint())
                .bind(item.resource.kind().as_str())
                .bind(identity.identity_kind())
                .bind(identity.codec())
                .bind(identity.value())
                .fetch_one(&mut *tx)
                .await?;
                if !proven {
                    return Err(DbError::CloseFoundationPrecondition(
                        "initial WorkScope retirement has unresolved original resources".into(),
                    ));
                }
            }
            if reason.trim().is_empty() {
                return Err(DbError::Serialization(
                    "work scope retirement reason must not be empty".into(),
                ));
            }
            let coordinator_owned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM coordinator_work_scope WHERE work_scope_id = ?1)",
            )
            .bind(precondition.scope_id().as_str())
            .fetch_one(&mut *tx)
            .await?;
            if coordinator_owned {
                return Ok(WorkScopeRetirementOutcome::Blocked(
                    WorkScopeRetirementBlocker::CoordinatorOwned,
                ));
            }
            let outcome = Database::retire_work_scope_lifecycle_tx(
                &mut tx,
                precondition,
                reason,
                Some(&admitted.attempt_id),
            )
            .await?;
            if matches!(outcome, WorkScopeRetirementOutcome::Retired) {
                Database::record_close_retirement_evidence_sql_tx(
                    &mut tx,
                    &RecordCloseRetirementEvidenceRequest {
                        attempt_id: admitted.attempt_id,
                        scope,
                        snapshot: snapshot.clone(),
                        resource,
                        outcome: RetirementOutcome::Retired,
                        detail: Some(reason.to_owned()),
                    },
                    snapshot.generation(),
                )
                .await?;
            }
            Ok(outcome)
        }
        .await;
        let committed = matches!(result, Ok(WorkScopeRetirementOutcome::Retired));
        let finalized = if committed {
            tx.commit().await
        } else {
            tx.rollback().await
        };
        match finalized {
            Ok(()) => LocalAuthorityResult::DurableFactEstablished(result),
            Err(_) => LocalAuthorityResult::DurableFactUnclassified,
        }
    }

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

    /// Persists the exact initial inspection and terminals a loss-bearing result atomically.
    /// Loss rows remain inspectable; no retirement effects are admitted.
    pub async fn terminalize_native_close_initial_losses(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: ReplaceCloseInspectionRequest,
        empty_generation: Option<&str>,
        detail: &str,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let (admitted, _) = require_initial_execution_tx(
                &mut tx, command, receipt, live_source, ClosePhase::AwaitingRetirementInspection,
            ).await?;
            if request.attempt_id != admitted.attempt_id || detail.trim().is_empty()
                || !request.scopes.iter().any(|scope| !scope.losses.is_empty())
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "loss terminalization requires the exact initial inspection with retained losses".into(),
                ));
            }
            self.replace_close_inspection_tx(&mut tx, &request, empty_generation).await?;
            let scope_rows = sqlx::query(
                "SELECT scope, captured_worktree_identity, captured_worktree_fingerprint, captured_worktree_locator\n                 FROM close_attempt_scopes WHERE attempt_id = ?1 ORDER BY scope",
            ).bind(admitted.attempt_id.as_str()).fetch_all(&mut *tx).await?;
            let work_scope_resource = |scope: &WorkScopeId| -> DbResult<RetiredResourceIdentity> {
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::WorkScope,
                    phoenix_core::domain::close::LossItemIdentity::Opaque(
                        phoenix_core::domain::close::OpaqueIdentity::parse(scope.as_str())
                            .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                    ),
                ).map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))
            };
            let authority_scope = request.scopes.iter().find(|scope| !scope.losses.is_empty())
                .map(|scope| scope.scope.clone()).ok_or_else(|| DbError::CloseFoundationPrecondition("loss scope missing".into()))?;
            let authority_resource = work_scope_resource(&authority_scope)?;
            let mut remaining_resources = Vec::new();
            for row in scope_rows {
                let scope = WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?;
                remaining_resources.push(CloseCleanupFailureResource {
                    scope: scope.clone(), resource: work_scope_resource(&scope)?,
                    disposition: CloseCleanupResourceDisposition::Unknown,
                });
                if let (Some(id), Some(fingerprint), Some(locator)) = (
                    row.try_get::<Option<String>, _>("captured_worktree_identity")?,
                    row.try_get::<Option<String>, _>("captured_worktree_fingerprint")?,
                    row.try_get::<Option<String>, _>("captured_worktree_locator")?,
                ) {
                    let identity = WorktreeIdentity::from_parts(
                        WorktreeId::parse(id).map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                        WorktreeFingerprint::parse(fingerprint).map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                        GitPathIdentity::decode_exact(&locator).map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                    );
                    remaining_resources.push(CloseCleanupFailureResource {
                        scope, resource: RetiredResourceIdentity::parse(
                            RetiredResourceKind::Worktree,
                            phoenix_core::domain::close::LossItemIdentity::Worktree(identity),
                        ).map_err(|error| DbError::CloseFoundationPrecondition(error.to_string()))?,
                        disposition: CloseCleanupResourceDisposition::Unknown,
                    });
                }
            }
            let run = phoenix_core::domain::close::CloseRunRef {
                attempt_id: admitted.attempt_id.clone(), ordinal: CloseRunOrdinal::INITIAL,
            };
            let terminal = TerminalizeInitialCloseCleanupFailureRequest {
                failure_occurrence_id: run.failure_occurrence_id(),
                attempt_id: admitted.attempt_id,
                source_product_conversation_id: receipt.admitted.target.clone(),
                authority: CloseCleanupFailureAuthority::CapturedScope {
                    scope: authority_scope, resource: authority_resource,
                },
                remaining_resources,
                reason: RetirementFailureReason::ManualRepairRequired,
                detail: detail.to_owned(),
                stop_certainty: CloseStopCertainty::ShutdownUncertain,
                occurred_at_us: chrono::Utc::now().timestamp_micros(),
            };
            Database::terminalize_close_run_cleanup_failure_inner_tx(&mut tx, &run, &terminal).await
        }.await;
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

    /// Records process teardown success only under the exact source-owned initial authority.
    // Check the source, receipt timing, and scoped dispatch in the transaction that records success.
    #[allow(clippy::too_many_lines)]
    pub async fn record_native_close_initial_process_step_success(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
        success: crate::CloseProcessStepSuccess,
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
            if success.run.attempt_id != admitted.attempt_id
                || success.run.ordinal != admitted.run_ordinal
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process success does not match the admitted run".into(),
                ));
            }
            let (created_at_us,): (i64,) = sqlx::query_as(
                "SELECT created_at_us FROM close_runs
                 WHERE attempt_id = ?1 AND run_ordinal = ?2 AND status = 'running'",
            )
            .bind(admitted.attempt_id.as_str())
            .bind(admitted.run_ordinal.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| DbError::CloseFoundationPrecondition(
                "initial process success requires its running admitted run".into(),
            ))?;
            let now = chrono::Utc::now().timestamp_micros();
            if chrono::DateTime::from_timestamp_micros(success.observed_at_us).is_none()
                || success.observed_at_us < created_at_us
                || success.observed_at_us > now
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process success observation time is outside the run interval".into(),
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
                ) AND NOT EXISTS (
                    SELECT 1 FROM close_attempt_scopes scope WHERE scope.attempt_id = ?1
                      AND NOT EXISTS (
                        SELECT 1 FROM close_retirement_inventories inventory
                        WHERE inventory.attempt_id = scope.attempt_id AND inventory.scope = scope.scope
                          AND inventory.sealed = 1 AND inventory.inspection_generation = ?2
                          AND inventory.inspection_fingerprint = ?3
                      )
                )",
            )
            .bind(admitted.attempt_id.as_str())
            .bind(snapshot.generation())
            .bind(snapshot.fingerprint())
            .fetch_one(&mut *tx)
            .await?;
            if !anchored {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process success requires the exact original sealed inventory".into(),
                ));
            }
            let resource_kind = match success.resource_kind {
                crate::CloseProcessResourceKind::BashProcessGroup => RetiredResourceKind::BashProcessGroup,
                crate::CloseProcessResourceKind::PtySession => RetiredResourceKind::PtySession,
                crate::CloseProcessResourceKind::BrowserSession => RetiredResourceKind::BrowserSession,
            };
            let process_identity = RetiredResourceIdentity::parse(
                resource_kind,
                phoenix_core::domain::close::LossItemIdentity::Opaque(success.identity.clone()),
            )
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let expected = crate::close_foundation::list_close_expected_retirement_resources_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            let member = expected.iter().find(|resource| {
                resource.attempt_id == success.run.attempt_id
                    && resource.scope == success.scope
                    && resource.snapshot == snapshot
                    && resource.resource == process_identity
            });
            if member.is_none() {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process success is not an exact typed member of the anchored inventory".into(),
                ));
            }
            let dispatched: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_retirement_resource_dispatches
                 WHERE attempt_id = ?1 AND scope = ?2 AND inspection_generation = ?3
                   AND inspection_fingerprint = ?4 AND resource_kind = ?5
                   AND identity_kind = ?6 AND identity_codec = ?7 AND identity_value = ?8)",
            )
            .bind(admitted.attempt_id.as_str())
            .bind(success.scope.as_str())
            .bind(snapshot.generation())
            .bind(snapshot.fingerprint())
            .bind(process_identity.kind().as_str())
            .bind(process_identity.identity().identity_kind())
            .bind(process_identity.identity().codec())
            .bind(process_identity.identity().value())
            .fetch_one(&mut *tx)
            .await?;
            if !dispatched {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial process success requires exact persisted dispatch intent".into(),
                ));
            }
            Database::insert_close_process_step_success_tx(&mut tx, &success).await
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
    // Validate the original inventory, proof kind, and dispatch in the transaction that writes the proof.
    #[allow(clippy::too_many_lines)]
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
                &mut tx, &request, request.snapshot.generation(),
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

    /// Stops the exact native initial run after a source-bound resource failure.
    /// The live source and retained command receipt are checked before either fresh
    /// terminalization or an exact read-only replay.
    ///
    /// # Errors
    /// Refuses stale source authority, mismatched retained evidence, or persistence failure.
    /// Commit or rollback uncertainty returns `DurableFactUnclassified`.
    #[allow(clippy::too_many_lines)] // Authority, exact tail and terminal outcome are one atomic boundary.
    pub async fn terminalize_native_close_initial_resource_failure(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        request: TerminalizeInitialCloseCleanupFailureRequest,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let admitted = require_initial_identity_tx(&mut tx, command, receipt, live_source).await?;
            let run = phoenix_core::domain::close::CloseRunRef {
                attempt_id: admitted.attempt_id.clone(),
                ordinal: CloseRunOrdinal::INITIAL,
            };
            let CloseCleanupFailureAuthority::ExpectedResource { scope, snapshot, resource } =
                &request.authority
            else {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial resource failure requires expected-resource authority".into(),
                ));
            };
            let now = chrono::Utc::now().timestamp_micros();
            if request.attempt_id != admitted.attempt_id
                || request.source_product_conversation_id != admitted.target
                || request.failure_occurrence_id != run.failure_occurrence_id()
                || request.detail.trim().is_empty()
                || request.occurred_at_us < 0
                || request.occurred_at_us > now
                || chrono::DateTime::from_timestamp_micros(request.occurred_at_us).is_none()
                || request.stop_certainty.confirmed_at_us().is_some_and(|confirmed| {
                    confirmed < 0 || confirmed > request.occurred_at_us
                })
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure requires exact authority and valid occurrence evidence".into(),
                ));
            }
            let obligation = crate::close_foundation::close_obligation_for_update(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            if obligation.product_conversation_id() != &admitted.target
                || obligation.snapshot() != Some(snapshot)
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure differs from retained target or snapshot".into(),
                ));
            }
            let run_state = sqlx::query(
                "SELECT run.status, run.ended_at_us, run.created_at_us,
                        (SELECT MAX(latest.run_ordinal) FROM close_runs latest WHERE latest.attempt_id = run.attempt_id) AS latest_ordinal,
                        (SELECT latest.status FROM close_runs latest WHERE latest.attempt_id = run.attempt_id ORDER BY latest.run_ordinal DESC LIMIT 1) AS latest_status
                 FROM close_runs run WHERE run.attempt_id = ?1 AND run.run_ordinal = 1",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| DbError::CloseFoundationPrecondition("retained initial run is missing".into()))?;
            let status: String = run_state.try_get("status")?;
            let ended_at_us: Option<i64> = run_state.try_get("ended_at_us")?;
            let created_at_us: i64 = run_state.try_get("created_at_us")?;
            let latest_ordinal: Option<i64> = run_state.try_get("latest_ordinal")?;
            let latest_status: Option<String> = run_state.try_get("latest_status")?;
            let has_failure: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM close_cleanup_failures WHERE attempt_id = ?1 AND cleanup_run_ordinal = 1)",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if request.occurred_at_us < created_at_us {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure occurrence predates its run".into(),
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
                )",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if !anchored {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure requires its exact anchored sealed inventory".into(),
                ));
            }
            let expected = crate::close_foundation::list_close_expected_retirement_resources_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
            )
            .await?;
            if !expected.iter().any(|item| {
                item.scope == *scope && item.snapshot == *snapshot && item.resource == *resource
            }) {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure authority is not in the exact sealed inventory".into(),
                ));
            }
            let mut tail = crate::close_foundation::expected_close_cleanup_failure_resources_tx(
                &mut tx,
                admitted.attempt_id.as_str(),
                scope,
                resource,
            )
            .await?;
            if !tail.iter().any(|item| {
                item.scope == *scope && item.resource == *resource
                    && item.disposition == CloseCleanupResourceDisposition::Failed
            }) {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure authority must remain unresolved".into(),
                ));
            }
            let has_prior_residual = tail.iter().any(|item| {
                (item.scope != *scope || item.resource != *resource)
                    && item.disposition == CloseCleanupResourceDisposition::Residual
            });
            for item in &mut tail {
                if item.disposition != CloseCleanupResourceDisposition::Failed {
                    item.disposition = CloseCleanupResourceDisposition::Unattempted;
                }
            }
            if request.remaining_resources != tail {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure children must equal the exact unresolved inventory tail".into(),
                ));
            }
            if matches!(request.stop_certainty, CloseStopCertainty::ConversationAndProcessesStopped { .. }) {
                let unresolved: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM close_expected_retirement_resources expected
                     LEFT JOIN close_retirement_resources proof ON proof.attempt_id=expected.attempt_id
                       AND proof.scope=expected.scope AND proof.inspection_generation=expected.inspection_generation
                       AND proof.inspection_fingerprint=expected.inspection_fingerprint
                       AND proof.resource_kind=expected.resource_kind AND proof.identity_kind=expected.identity_kind
                       AND proof.identity_codec=expected.identity_codec AND proof.identity_value=expected.identity_value
                     WHERE expected.attempt_id=?1 AND expected.resource_kind IN
                       ('bash_process_group','tmux_server','pty_session','browser_session','equivalent_live_resource')
                       AND (proof.proof_kind IS NULL OR proof.proof_kind='residual')",
                )
                .bind(admitted.attempt_id.as_str())
                .fetch_one(&mut *tx)
                .await?;
                if unresolved != 0 {
                    return Err(DbError::CloseFoundationPrecondition(
                        "confirmed source stop requires success proof for every process resource".into(),
                    ));
                }
            }
            if has_failure {
                if status != "stopped"
                    || latest_ordinal != Some(1)
                    || latest_status.as_deref() != Some("stopped")
                    || obligation.phase() != ClosePhase::Completed
                    || obligation.close_outcome() != Some(request.stop_certainty.completion_outcome())
                    || ended_at_us != Some(request.occurred_at_us)
                    || !Database::retained_close_cleanup_failure_matches_tx(&mut tx, &run, &request).await?
                {
                    return Err(DbError::CloseFoundationPrecondition(
                        "native initial failure replay conflicts with retained terminal evidence".into(),
                    ));
                }
                return Ok(obligation);
            }
            if status != "running"
                || latest_ordinal != Some(1)
                || latest_status.as_deref() != Some("running")
                || obligation.phase() != ClosePhase::RetirementRequested
                || has_prior_residual
            {
                return Err(DbError::CloseFoundationPrecondition(
                    "native initial failure requires the latest running retirement-requested run without prior residuals".into(),
                ));
            }
            Database::terminalize_close_run_cleanup_failure_inner_tx(&mut tx, &run, &request).await
        }.await;
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

    /// Finalizes only the exact live source-owned initial run after its anchored inventory
    /// has concrete successful retirement proof, or reads its exact completed result.
    /// Live and durable source authority are required before fresh completion and replay.
    // Classify exact replay or finalize against the original inventory and dispatches under one writer.
    #[allow(clippy::too_many_lines)]
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
            let admitted = require_initial_identity_tx(&mut tx, command, receipt, live_source).await?;
            let retained_success: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM close_obligations obligation
                    JOIN close_runs run ON run.attempt_id = obligation.attempt_id AND run.run_ordinal = 1
                    JOIN product_conversations product ON product.id = obligation.product_conversation_id
                    JOIN close_attempt_members member ON member.attempt_id = obligation.attempt_id
                      AND member.member_role IN ('latest', 'root_latest')
                    JOIN messages outcome ON outcome.message_id = 'close-outcome:' || obligation.attempt_id
                      AND outcome.conversation_id = member.conversation_id
                    WHERE obligation.attempt_id = ?1 AND obligation.product_conversation_id = ?2
                      AND obligation.native_managed = 1 AND obligation.phase = 'completed'
                      AND obligation.close_outcome = 'archived' AND run.status = 'completed'
                      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = obligation.attempt_id)
                      AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'history'
                      AND EXISTS (SELECT 1 FROM conversations archived WHERE archived.id = member.conversation_id AND archived.archived = 1)
                )",
            )
            .bind(admitted.attempt_id.as_str())
            .bind(receipt.admitted.target.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if retained_success {
                return crate::close_foundation::close_obligation_for_update(
                    &mut tx,
                    admitted.attempt_id.as_str(),
                )
                .await;
            }
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
            let dispatched: bool = sqlx::query_scalar(
                "SELECT NOT EXISTS (
                    SELECT 1 FROM close_expected_retirement_resources expected
                    WHERE expected.attempt_id = ?1 AND NOT EXISTS (
                        SELECT 1 FROM close_retirement_resource_dispatches dispatch
                        WHERE dispatch.attempt_id = expected.attempt_id
                          AND dispatch.scope = expected.scope
                          AND dispatch.inspection_generation = expected.inspection_generation
                          AND dispatch.inspection_fingerprint = expected.inspection_fingerprint
                          AND dispatch.resource_kind = expected.resource_kind
                          AND dispatch.identity_kind = expected.identity_kind
                          AND dispatch.identity_codec = expected.identity_codec
                          AND dispatch.identity_value = expected.identity_value
                    )
                )",
            )
            .bind(admitted.attempt_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if !dispatched {
                return Err(DbError::CloseFoundationPrecondition(
                    "initial completion requires dispatch authority for every expected resource".into(),
                ));
            }
            // The completion trigger checks every expected identity for concrete
            // retired/absence-adopted evidence and rejects residual proofs.
            Database::complete_close_retirement_core_tx(&mut tx, &admitted.attempt_id).await
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
    // Derive the unresolved resources from captured scopes in the transaction that stops the run.
    #[allow(clippy::too_many_lines)]
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
            Database::terminalize_close_run_cleanup_failure_inner_tx(&mut tx, &run, &request).await
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
    #[cfg(test)]
    pub(crate) async fn settle_native_close_initial(
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

#[cfg(test)]
pub(crate) async fn settle_initial_tx(
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
pub(super) async fn require_initial_identity_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
) -> DbResult<NativeCloseRunRef> {
    let NativeCloseIntent::Initial { target } = command.intent() else {
        return Err(DbError::CloseFoundationPrecondition(
            "expected initial Close intent".into(),
        ));
    };
    crate::executing_source::require_executing_source(
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
    Ok(admitted)
}

async fn require_initial_execution_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &phoenix_core::domain::sm_state::ConvState,
    expected_phase: phoenix_core::domain::close::ClosePhase,
) -> DbResult<(NativeCloseRunRef, bool)> {
    let admitted = require_initial_identity_tx(tx, command, receipt, live_source).await?;
    let target = &receipt.admitted.target;
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
    ensure_close_retirement_loss_free_tx(tx, admitted.attempt_id.as_str()).await?;
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
    // Deferred exact receipt/operation FKs bind original source before creation.
    // A retained unreceipted operation cannot acquire provenance by late adoption.
    sqlx::query(
        "INSERT INTO native_close_completion_retry_origins
         (attempt_id, origin_request_id, origin_request_fingerprint, source_transcript_id,
          source_message_id, source_tool_use_id, product_conversation_id, initial_run_ordinal)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
    )
    .bind(&attempt_id)
    .bind(command.request_id().to_string())
    .bind(command.fingerprint())
    .bind(command.source_transcript_id())
    .bind(&command.source().message_id)
    .bind(&command.source().tool_use_id)
    .bind(target.as_str())
    .execute(&mut **tx)
    .await?;
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
mod tests {
    // Reuse the native-initial fixtures and legacy API compatibility coverage.
    include!("initial/tests.rs");

    #[tokio::test]
    // One captured run checks callback refusal, metadata writes, and lease lifetime through release.
    #[allow(clippy::too_many_lines)]
    async fn worktree_metadata_entry_runs_after_validation_and_refusal_writes_nothing() {
        use std::cell::Cell;

        struct Lease<'a>(&'a Cell<bool>);
        impl Drop for Lease<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let f = Fixture::new().await;
        let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
        let worktree = expected
            .iter()
            .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
            .unwrap();
        let plan = || crate::RecordCloseWorktreeCleanupPlanRequest {
            attempt_id: receipt.admitted.attempt_id.clone(),
            scope: worktree.scope.clone(),
            snapshot: worktree.snapshot.clone(),
            resource: worktree.resource.clone(),
            administrative_dir_incarnation: "entry-gated-admin".into(),
            administrative_dir: "/tmp/entry-gated-admin".into(),
        };
        let entries = Cell::new(0);
        let refuse = || -> DbResult<()> {
            entries.set(entries.get() + 1);
            Err(DbError::CloseFoundationPrecondition(
                "source revoked at entry".into(),
            ))
        };

        // A valid source snapshot is not enough: the exact original dispatch
        // must be durable before even the short source gate can be invoked.
        assert!(matches!(
            f.db.record_native_close_initial_worktree_cleanup_plan_with_entry(
                &command,
                &receipt,
                &live_source,
                plan(),
                refuse,
            )
            .await,
            (LocalAuthorityResult::DurableFactEstablished(Err(_)), None)
        ));
        assert_eq!(entries.get(), 0);
        f.db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &live_source,
            initial_dispatch_request(&receipt, worktree),
        )
        .await
        .established()
        .unwrap()
        .unwrap();

        assert!(matches!(
            f.db.record_native_close_initial_worktree_cleanup_plan_with_entry(
                &command,
                &receipt,
                &live_source,
                plan(),
                refuse,
            )
            .await,
            (LocalAuthorityResult::DurableFactEstablished(Err(_)), None)
        ));
        assert_eq!(entries.get(), 1);
        let mut foreign_receipt = receipt.clone();
        foreign_receipt.request_id = Uuid::nil();
        assert!(matches!(
            f.db.record_native_close_initial_worktree_cleanup_plan_with_entry(
                &command,
                &foreign_receipt,
                &live_source,
                plan(),
                refuse,
            )
            .await,
            (LocalAuthorityResult::DurableFactEstablished(Err(_)), None)
        ));
        assert_eq!(entries.get(), 1);
        assert!(f
            .db
            .close_worktree_cleanup_plan(
                &receipt.admitted.attempt_id,
                &worktree.scope,
                &worktree.snapshot,
                &worktree.resource,
            )
            .await
            .unwrap()
            .is_none());

        // A successful entry writes and releases the same transaction. Existing
        // no-callback wrappers remain covered by the adjacent legacy API tests.
        let dropped = Cell::new(false);
        let (classification, lease) =
            f.db.record_native_close_initial_worktree_cleanup_plan_with_entry(
                &command,
                &receipt,
                &live_source,
                plan(),
                || Ok(Lease(&dropped)),
            )
            .await;
        classification.established().unwrap().unwrap();
        assert!(lease.is_some());
        assert!(
            !dropped.get(),
            "lease must outlive the SQL mutation and commit"
        );
        assert!(f
            .db
            .close_worktree_cleanup_plan(
                &receipt.admitted.attempt_id,
                &worktree.scope,
                &worktree.snapshot,
                &worktree.resource,
            )
            .await
            .unwrap()
            .is_some());
        drop(lease);
        assert!(dropped.get());
        // Entry can succeed but the mutation can still refuse. The same lease
        // must survive the rollback, not just the successful commit path.
        dropped.set(false);
        let (classification, lease) =
            f.db.bind_native_close_initial_worktree_final_tombstone_object_with_entry(
                &command,
                &receipt,
                &live_source,
                crate::BindCloseWorktreeFinalTombstoneObjectRequest {
                    attempt_id: receipt.admitted.attempt_id.clone(),
                    scope: worktree.scope.clone(),
                    snapshot: worktree.snapshot.clone(),
                    resource: worktree.resource.clone(),
                    object_device: 29,
                    object_inode: 31,
                },
                || Ok(Lease(&dropped)),
            )
            .await;
        assert!(matches!(
            classification,
            LocalAuthorityResult::DurableFactEstablished(Err(_))
        ));
        assert!(lease.is_some());
        assert!(
            !dropped.get(),
            "lease must outlive a refused mutation's rollback"
        );
        drop(lease);
        assert!(dropped.get());

        let root = || crate::BindCloseWorktreeFinalTombstoneRequest {
            attempt_id: receipt.admitted.attempt_id.clone(),
            scope: worktree.scope.clone(),
            snapshot: worktree.snapshot.clone(),
            resource: worktree.resource.clone(),
            tombstone: crate::CloseWorktreeFinalTombstone {
                root: "/tmp/entry-gated-tombstone".into(),
                device: 17,
                inode: 23,
                object_device: None,
                object_inode: None,
            },
        };
        assert!(matches!(
            f.db.bind_native_close_initial_worktree_final_tombstone_with_entry(
                &command,
                &receipt,
                &live_source,
                root(),
                refuse,
            )
            .await,
            (LocalAuthorityResult::DurableFactEstablished(Err(_)), None)
        ));
        let retained =
            f.db.close_worktree_cleanup_plan(
                &receipt.admitted.attempt_id,
                &worktree.scope,
                &worktree.snapshot,
                &worktree.resource,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(retained.final_tombstone.is_none());
        f.db.bind_native_close_initial_worktree_final_tombstone_with_entry(
            &command,
            &receipt,
            &live_source,
            root(),
            || Ok(()),
        )
        .await
        .0
        .established()
        .unwrap()
        .unwrap();
        let object = || crate::BindCloseWorktreeFinalTombstoneObjectRequest {
            attempt_id: receipt.admitted.attempt_id.clone(),
            scope: worktree.scope.clone(),
            snapshot: worktree.snapshot.clone(),
            resource: worktree.resource.clone(),
            object_device: 29,
            object_inode: 31,
        };
        assert!(matches!(
            f.db.bind_native_close_initial_worktree_final_tombstone_object_with_entry(
                &command,
                &receipt,
                &live_source,
                object(),
                refuse,
            )
            .await,
            (LocalAuthorityResult::DurableFactEstablished(Err(_)), None)
        ));
        let retained =
            f.db.close_worktree_cleanup_plan(
                &receipt.admitted.attempt_id,
                &worktree.scope,
                &worktree.snapshot,
                &worktree.resource,
            )
            .await
            .unwrap()
            .unwrap()
            .final_tombstone
            .unwrap();
        assert_eq!(
            (retained.object_device, retained.object_inode),
            (None, None)
        );
        f.db.bind_native_close_initial_worktree_final_tombstone_object_with_entry(
            &command,
            &receipt,
            &live_source,
            object(),
            || Ok(()),
        )
        .await
        .0
        .established()
        .unwrap()
        .unwrap();
        let retained =
            f.db.close_worktree_cleanup_plan(
                &receipt.admitted.attempt_id,
                &worktree.scope,
                &worktree.snapshot,
                &worktree.resource,
            )
            .await
            .unwrap()
            .unwrap()
            .final_tombstone
            .unwrap();
        assert_eq!((retained.device, retained.inode), (17, 23));
        assert_eq!(
            (retained.object_device, retained.object_inode),
            (Some(29), Some(31))
        );
        assert_eq!(entries.get(), 3);

        // A captured source snapshot must not authorize entry after the stored
        // coordinator source has changed. Validation refuses before the gate.
        f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
            .await
            .unwrap();
        assert!(matches!(
            f.db.record_native_close_initial_worktree_cleanup_plan_with_entry(
                &command,
                &receipt,
                &live_source,
                plan(),
                refuse,
            )
            .await,
            (LocalAuthorityResult::DurableFactEstablished(Err(_)), None)
        ));
        assert_eq!(entries.get(), 3);
    }
}
