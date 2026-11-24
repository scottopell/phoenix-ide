//! Source-bound execution of the exact admitted native resource plan.

use std::collections::{BTreeMap, BTreeSet};

use phoenix_core::domain::close::{
    CloseAttemptScope, CloseExpectedRetirementResource, CloseObligation, CloseRetirementSnapshot,
    CloseRetirementTarget, CloseRunRef, CloseStopCertainty, LossItemIdentity,
    RetiredResourceIdentity, RetiredResourceKind,
};
use phoenix_core::work_scope::{
    WorkScopeId, WorkScopeRetirementBlocker, WorkScopeRetirementOutcome,
    WorkScopeRetirementPrecondition,
};
use sqlx::{Row, Sqlite, Transaction};

use super::initial::{NativeCloseInitialEntryRelease, NativeCloseInitialFirstEffectError};
use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use crate::close_foundation::{
    close_obligation_for_update, parse_cleanup_resource_identity, retained_close_retry_progress_tx,
    BindCloseWorktreeFinalTombstoneObjectRequest, BindCloseWorktreeFinalTombstoneRequest,
    CloseCleanupFailureAuthority, CloseCleanupFailureResource, CloseCleanupResourceDisposition,
    CloseSafeRetryEffect, CloseSafeRetryProgress, RecordCloseRetirementDispatchRequest,
    RecordCloseWorktreeCleanupPlanRequest, StoppedCloseOriginalRetirementProof,
    TerminalizeInitialCloseCleanupFailureRequest,
};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

fn refused(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.to_owned())
}

/// Persisted original-generation inputs, not permission to dispatch resource effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseRetryExecutionContext {
    pub snapshot: CloseRetirementSnapshot,
    pub effects: Vec<CloseSafeRetryEffect>,
    pub progress: CloseSafeRetryProgress,
    pub scopes: Vec<CloseAttemptScope>,
    pub resources: Vec<CloseExpectedRetirementResource>,
    pub worktree_snapshots: Vec<(CloseRetirementTarget, CloseRetirementSnapshot)>,
}

/// Exact stopped original-generation evidence for read-only pre-admission assessment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCloseRetryPreAdmissionContext {
    pub original: StoppedCloseOriginalRetirementProof,
    pub worktree_snapshots: Vec<(CloseRetirementTarget, CloseRetirementSnapshot)>,
}

fn decode_snapshot_component(encoded: &str) -> DbResult<BTreeMap<WorkScopeId, &str>> {
    fn take_component<'a>(remaining: &mut &'a str) -> DbResult<&'a str> {
        let invalid = || refused("malformed original aggregate snapshot component");
        let (digits, rest) = remaining.split_once(':').ok_or_else(invalid)?;
        let length = digits.parse::<usize>().map_err(|_| invalid())?;
        if length.to_string() != digits {
            return Err(invalid());
        }
        let value = rest.get(..length).ok_or_else(invalid)?;
        *remaining = rest.get(length..).ok_or_else(invalid)?;
        Ok(value)
    }

    if encoded == "no-worktree" {
        return Ok(BTreeMap::new());
    }
    let mut remaining = encoded
        .strip_prefix("v1")
        .filter(|rest| !rest.is_empty())
        .ok_or_else(|| refused("original snapshot lacks a nonempty v1 aggregate"))?;
    let mut components = BTreeMap::new();
    while !remaining.is_empty() {
        let scope = WorkScopeId::parse(take_component(&mut remaining)?)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let value = take_component(&mut remaining)?;
        if components
            .last_key_value()
            .is_some_and(|(previous, _)| previous >= &scope)
        {
            return Err(refused(
                "original aggregate scopes are duplicate or unordered",
            ));
        }
        components.insert(scope, value);
    }
    Ok(components)
}

fn original_worktree_snapshots(
    snapshot: &CloseRetirementSnapshot,
    scopes: &[CloseAttemptScope],
) -> DbResult<Vec<(CloseRetirementTarget, CloseRetirementSnapshot)>> {
    let generations = decode_snapshot_component(snapshot.generation())?;
    let mut fingerprints = decode_snapshot_component(snapshot.fingerprint())?;
    let captured = scopes
        .iter()
        .filter(|scope| scope.captured_worktree.is_some())
        .map(|scope| &scope.scope)
        .collect::<BTreeSet<_>>();
    if generations.keys().ne(fingerprints.keys()) || generations.keys().ne(captured) {
        return Err(refused(
            "original aggregate differs from captured worktree scopes",
        ));
    }
    generations
        .into_iter()
        .map(|(scope, generation)| {
            let fingerprint = fingerprints
                .remove(&scope)
                .ok_or_else(|| refused("original aggregate fingerprint scope is missing"))?;
            let snapshot = CloseRetirementSnapshot::parse(generation, fingerprint)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            Ok((CloseRetirementTarget { scope }, snapshot))
        })
        .collect()
}

struct NativeCloseRetryPlanIdentity {
    captured_origin: bool,
    effect_count: i64,
    snapshot: CloseRetirementSnapshot,
    running: bool,
    created_at_us: i64,
}

// Execution authority, intentionally NOT a StoppedCloseOriginalRetirementProof.
struct NativeCloseRetryAuthority {
    snapshot: CloseRetirementSnapshot,
    scopes: Vec<CloseAttemptScope>,
    resources: Vec<CloseExpectedRetirementResource>,
    pending: Vec<CloseSafeRetryEffect>,
}

enum NativeCloseRetryDispatchKind {
    Worktree,
    Tmux,
}

impl NativeCloseRetryDispatchKind {
    fn for_resource(resource: &RetiredResourceIdentity) -> DbResult<Self> {
        match resource.kind() {
            RetiredResourceKind::Worktree => Ok(Self::Worktree),
            RetiredResourceKind::TmuxServer => Ok(Self::Tmux),
            _ => Err(refused(
                "native retry dispatch requires a worktree or tmux effect",
            )),
        }
    }
}

enum NativeCloseRetryWorktreeMetadata {
    Plan(RecordCloseWorktreeCleanupPlanRequest),
    Tombstone(BindCloseWorktreeFinalTombstoneRequest),
    TombstoneObject(BindCloseWorktreeFinalTombstoneObjectRequest),
}

impl NativeCloseRetryWorktreeMetadata {
    fn identity(
        &self,
    ) -> (
        phoenix_core::domain::close::CloseAttemptId,
        WorkScopeId,
        CloseRetirementSnapshot,
        phoenix_core::domain::close::RetiredResourceIdentity,
    ) {
        match self {
            Self::Plan(request) => (
                request.attempt_id.clone(),
                request.scope.clone(),
                request.snapshot.clone(),
                request.resource.clone(),
            ),
            Self::Tombstone(request) => (
                request.attempt_id.clone(),
                request.scope.clone(),
                request.snapshot.clone(),
                request.resource.clone(),
            ),
            Self::TombstoneObject(request) => (
                request.attempt_id.clone(),
                request.scope.clone(),
                request.snapshot.clone(),
                request.resource.clone(),
            ),
        }
    }
}

impl Database {
    async fn read_native_close_retry_authority_tx(
        tx: &mut Transaction<'_, Sqlite>,
        failed_run: &CloseRunRef,
        plan: &NativeCloseRetryPlanIdentity,
    ) -> DbResult<NativeCloseRetryAuthority> {
        if !plan.captured_origin {
            let original =
                Self::read_stopped_close_original_retirement_proof_tx(tx, failed_run).await?;
            let pending = super::retry::original_pending_effects_tx(tx, &original).await?;
            return Ok(NativeCloseRetryAuthority {
                snapshot: original.snapshot,
                scopes: original.scopes,
                resources: original.resources,
                pending,
            });
        }
        let scopes = super::captured_loss_retry::read_captured_scopes_tx(tx, failed_run).await?;
        let effects = super::captured_loss_admission::captured_effects(&scopes)?;
        if i64::try_from(effects.len()).ok() != Some(plan.effect_count) {
            return Err(refused(
                "captured retry authority differs from its finite captured set",
            ));
        }
        let resources = effects
            .iter()
            .map(|effect| CloseExpectedRetirementResource {
                attempt_id: failed_run.attempt_id.clone(),
                scope: effect.scope.clone(),
                snapshot: plan.snapshot.clone(),
                resource: effect.resource.clone(),
            })
            .collect();
        Ok(NativeCloseRetryAuthority {
            snapshot: plan.snapshot.clone(),
            scopes,
            resources,
            pending: effects,
        })
    }

    /// Reads an exact stopped predecessor's original snapshot context in one read-only
    /// transaction. Historical stopped predecessors remain readable after later runs;
    /// this context confers no admission, dispatch, or cleanup authority.
    ///
    /// # Errors
    /// Refuses a run that is not an exact stopped original-generation predecessor or
    /// whose persisted aggregate does not match its captured scopes. The read does not
    /// mutate retained state.
    pub async fn read_native_close_retry_pre_admission_context(
        &self,
        failed_run: &CloseRunRef,
    ) -> DbResult<NativeCloseRetryPreAdmissionContext> {
        let mut tx = self.pool.begin().await?;
        let original =
            Self::read_stopped_close_original_retirement_proof_tx(&mut tx, failed_run).await?;
        let worktree_snapshots = original_worktree_snapshots(&original.snapshot, &original.scopes)?;
        tx.commit().await?;
        Ok(NativeCloseRetryPreAdmissionContext {
            original,
            worktree_snapshots,
        })
    }

    /// Reads the exact latest running resource retry in one read-only transaction.
    /// The actor must hold its exclusive source occurrence and execution lock; this
    /// context neither owns that authority nor authorizes dispatch or fresh inspection.
    ///
    /// # Errors
    /// Refuses stale source, mismatched receipt/plan, non-running successor, or
    /// unreadable original identities without changing any retained state.
    pub async fn read_native_close_retry_execution_context(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> DbResult<NativeCloseRetryExecutionContext> {
        let mut tx = self.pool.begin().await?;
        let plan =
            Self::require_native_close_retry_plan_identity_tx(&mut tx, command, admitted).await?;
        if !plan.running {
            return Err(refused(
                "native retry context requires the exact latest running plan",
            ));
        }
        Self::require_native_close_retry_members_quiescent_tx(&mut tx, &admitted.attempt_id)
            .await?;
        let effect_count = plan.effect_count;
        let NativeCloseIntent::ExplicitRetry {
            failed_run_ordinal, ..
        } = command.intent()
        else {
            return Err(refused(
                "native retry context requires an explicit retry intent",
            ));
        };
        let original = Self::read_native_close_retry_authority_tx(
            &mut tx,
            &CloseRunRef {
                attempt_id: admitted.attempt_id.clone(),
                ordinal: *failed_run_ordinal,
            },
            &plan,
        )
        .await?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        let progress = retained_close_retry_progress_tx(&mut tx, &run).await?;
        let effects = sqlx::query(
            "SELECT effect.*, captured.captured_worktree_fingerprint, captured.captured_worktree_locator
             FROM close_run_retry_effects effect
             JOIN close_attempt_scopes captured
               ON captured.attempt_id = effect.attempt_id AND captured.scope = effect.scope
             WHERE effect.attempt_id = ?1 AND effect.run_ordinal = ?2 ORDER BY effect.ordinal",
        )
        .bind(run.attempt_id.as_str())
        .bind(run.ordinal.get())
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| {
            Ok(CloseSafeRetryEffect {
                scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                resource: parse_cleanup_resource_identity(&row)?,
            })
        })
        .collect::<DbResult<Vec<_>>>()?;
        if i64::try_from(effects.len()).ok() != Some(effect_count)
            || effects.len() != progress.successful.len() + progress.pending.len()
            || effects.iter().any(|effect| {
                !original.resources.iter().any(|expected| {
                    expected.scope == effect.scope && expected.resource == effect.resource
                })
            })
        {
            return Err(refused(
                "native retry context requires every exact original planned identity",
            ));
        }
        let worktree_snapshots = original_worktree_snapshots(&original.snapshot, &original.scopes)?;
        tx.commit().await?;
        Ok(NativeCloseRetryExecutionContext {
            snapshot: original.snapshot,
            effects,
            progress,
            scopes: original.scopes,
            resources: original.resources,
            worktree_snapshots,
        })
    }

    /// Runs a short synchronous handoff only after validating the exact live retry
    /// under the SQLite writer. The caller must supervise cancellation and panic,
    /// retain the callback outcome independently, and prevent repeated entry.
    /// An unconfirmed rollback is not proof that the writer was released.
    pub async fn with_native_close_retry_worktree_first_effect<F, T>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        first_effect: F,
    ) -> Result<(T, NativeCloseInitialEntryRelease), NativeCloseInitialFirstEffectError>
    where
        F: FnOnce() -> T,
    {
        self.with_native_close_retry_resource_first_effect(
            command,
            admitted,
            live_source,
            scope,
            snapshot,
            resource,
            NativeCloseRetryDispatchKind::Worktree,
            first_effect,
        )
        .await
    }

    /// Runs the synchronous tmux handoff only for an exact pending, dispatched retry
    /// identity. The caller has the same supervision duties as for worktree entry.
    pub async fn with_native_close_retry_tmux_first_effect<F, T>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        first_effect: F,
    ) -> Result<(T, NativeCloseInitialEntryRelease), NativeCloseInitialFirstEffectError>
    where
        F: FnOnce() -> T,
    {
        self.with_native_close_retry_resource_first_effect(
            command,
            admitted,
            live_source,
            scope,
            snapshot,
            resource,
            NativeCloseRetryDispatchKind::Tmux,
            first_effect,
        )
        .await
    }

    async fn with_native_close_retry_resource_first_effect<F, T>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: &RetiredResourceIdentity,
        kind: NativeCloseRetryDispatchKind,
        first_effect: F,
    ) -> Result<(T, NativeCloseInitialEntryRelease), NativeCloseInitialFirstEffectError>
    where
        F: FnOnce() -> T,
    {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|error| NativeCloseInitialFirstEffectError::NotStarted(error.into()))?;
        let validation = async {
            crate::task_approval_inspection::require_executing_source(
                live_source,
                command.source(),
                "close_conversation",
                &command.intent().source_input(),
            )
            .map_err(|_| refused("Close requires its current executing source"))?;
            let effect = CloseSafeRetryEffect {
                scope,
                resource: resource.clone(),
            };
            let original = Self::require_native_close_retry_pending_dispatch_tx(
                &mut tx, command, admitted, &effect, kind,
            )
            .await?;
            if snapshot != original {
                return Err(refused(
                    "native retry entry requires its exact original snapshot",
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
            .bind(effect.scope.as_str())
            .bind(snapshot.generation())
            .bind(snapshot.fingerprint())
            .bind(resource.kind().as_str())
            .bind(identity.identity_kind())
            .bind(identity.codec())
            .bind(identity.value())
            .fetch_one(&mut *tx)
            .await?;
            if !dispatched {
                return Err(refused(
                    "native retry entry requires exact persisted dispatch membership",
                ));
            }
            Ok(())
        }
        .await;
        if let Err(error) = validation {
            let release = match tx.rollback().await {
                Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
                Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
            };
            return Err(NativeCloseInitialFirstEffectError::Rejected { error, release });
        }
        let outcome = first_effect();
        let release = match tx.rollback().await {
            Ok(()) => NativeCloseInitialEntryRelease::Confirmed,
            Err(error) => NativeCloseInitialEntryRelease::Unconfirmed(error.into()),
        };
        Ok((outcome, release))
    }

    async fn require_native_close_retry_plan_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> DbResult<i64> {
        let plan = Self::require_native_close_retry_plan_identity_tx(tx, command, admitted).await?;
        if !plan.running {
            return Err(refused(
                "native retry mutation requires its exact latest running plan",
            ));
        }
        Self::require_native_close_retry_members_quiescent_tx(tx, &admitted.attempt_id).await?;
        Ok(plan.effect_count)
    }

    pub async fn record_native_close_retry_worktree_cleanup_plan(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: RecordCloseWorktreeCleanupPlanRequest,
    ) -> LocalAuthorityResult<DbResult<()>> {
        self.write_native_close_retry_worktree_metadata(
            command,
            admitted,
            NativeCloseRetryWorktreeMetadata::Plan(request),
            || Ok(()),
        )
        .await
        .0
    }

    pub async fn bind_native_close_retry_worktree_final_tombstone(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: BindCloseWorktreeFinalTombstoneRequest,
    ) -> LocalAuthorityResult<DbResult<()>> {
        self.write_native_close_retry_worktree_metadata(
            command,
            admitted,
            NativeCloseRetryWorktreeMetadata::Tombstone(request),
            || Ok(()),
        )
        .await
        .0
    }

    pub async fn bind_native_close_retry_worktree_final_tombstone_object(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: BindCloseWorktreeFinalTombstoneObjectRequest,
    ) -> LocalAuthorityResult<DbResult<()>> {
        self.write_native_close_retry_worktree_metadata(
            command,
            admitted,
            NativeCloseRetryWorktreeMetadata::TombstoneObject(request),
            || Ok(()),
        )
        .await
        .0
    }

    /// Entry callbacks must be short nonblocking in-memory source gates, with no
    /// I/O, SQL, waits, or nested writers. Do not hold source custody while waiting
    /// for this method. A successful lease is returned even on unknown release;
    /// the caller must retain it then and supervise cancellation/panic separately.
    pub async fn record_native_close_retry_worktree_cleanup_plan_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: RecordCloseWorktreeCleanupPlanRequest,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        self.write_native_close_retry_worktree_metadata(
            command,
            admitted,
            NativeCloseRetryWorktreeMetadata::Plan(request),
            entry,
        )
        .await
    }

    /// Uses the same custody and supervision contract as the cleanup-plan entry API.
    pub async fn bind_native_close_retry_worktree_final_tombstone_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: BindCloseWorktreeFinalTombstoneRequest,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        self.write_native_close_retry_worktree_metadata(
            command,
            admitted,
            NativeCloseRetryWorktreeMetadata::Tombstone(request),
            entry,
        )
        .await
    }

    /// Uses the same custody and supervision contract as the cleanup-plan entry API.
    pub async fn bind_native_close_retry_worktree_final_tombstone_object_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        request: BindCloseWorktreeFinalTombstoneObjectRequest,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        self.write_native_close_retry_worktree_metadata(
            command,
            admitted,
            NativeCloseRetryWorktreeMetadata::TombstoneObject(request),
            entry,
        )
        .await
    }

    async fn write_native_close_retry_worktree_metadata<F, L>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        metadata: NativeCloseRetryWorktreeMetadata,
        entry: F,
    ) -> (LocalAuthorityResult<DbResult<()>>, Option<L>)
    where
        F: FnOnce() -> DbResult<L>,
    {
        let (attempt_id, scope, snapshot, resource) = metadata.identity();
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => {
                return (
                    LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
                    None,
                )
            }
        };
        let mut lease = None;
        let result = async {
            let plan =
                Self::require_native_close_retry_plan_identity_tx(&mut tx, command, admitted)
                    .await?;
            if !plan.running
                || attempt_id != admitted.attempt_id
                || snapshot != plan.snapshot
                || resource.kind() != RetiredResourceKind::Worktree
            {
                return Err(refused(
                    "native retry metadata requires its exact original running worktree plan",
                ));
            }
            Self::require_native_close_retry_members_quiescent_tx(&mut tx, &admitted.attempt_id)
                .await?;
            let run = CloseRunRef {
                attempt_id: admitted.attempt_id.clone(),
                ordinal: admitted.run_ordinal,
            };
            let progress = retained_close_retry_progress_tx(&mut tx, &run).await?;
            let matching = progress
                .pending
                .iter()
                .any(|effect| effect.scope == scope && effect.resource == resource);
            if !matching
                || i64::try_from(progress.successful.len() + progress.pending.len()).ok()
                    != Some(plan.effect_count)
            {
                return Err(refused(
                    "native retry metadata requires an exact pending full-identity worktree effect",
                ));
            }
            lease = Some(entry()?);
            match metadata {
                NativeCloseRetryWorktreeMetadata::Plan(request) => {
                    Self::record_close_worktree_cleanup_plan_tx(&mut tx, request).await
                }
                NativeCloseRetryWorktreeMetadata::Tombstone(request) => {
                    Self::bind_close_worktree_final_tombstone_tx(&mut tx, request).await
                }
                NativeCloseRetryWorktreeMetadata::TombstoneObject(request) => {
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
        (classification, lease)
    }

    /// Records dispatch of one pending original-generation worktree or tmux effect under the
    /// exact live native command and latest running retry plan. No current inspection
    /// snapshot is accepted from the caller.
    ///
    /// # Errors
    /// Source, receipt, plan, full-identity and storage errors are classified only
    /// after rollback. Commit or rollback uncertainty is `DurableFactUnclassified`.
    pub async fn record_native_close_retry_retirement_dispatch(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
    ) -> LocalAuthorityResult<DbResult<()>> {
        self.record_native_close_retry_retirement_dispatch_with_entry(
            command,
            admitted,
            effect,
            || Ok(()),
        )
        .await
        .0
    }

    /// Acquires entry custody after exact writer validation and before inserting
    /// dispatch. Returns a successful lease through commit or rollback, including
    /// unclassified release. The callback must be short, nonblocking and in-memory;
    /// callers must supervise cancellation/panic and retain uncertain custody.
    pub async fn record_native_close_retry_retirement_dispatch_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
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
                )
            }
        };
        let mut lease = None;
        let result = Self::record_native_close_retry_retirement_dispatch_tx(
            &mut tx, command, admitted, effect, entry, &mut lease,
        )
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
        (classification, lease)
    }

    async fn require_native_close_retry_pending_worktree_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
    ) -> DbResult<CloseRetirementSnapshot> {
        let plan = Self::require_native_close_retry_plan_identity_tx(tx, command, admitted).await?;
        if !plan.running {
            return Err(refused(
                "native retry dispatch requires its exact latest running plan",
            ));
        }
        Self::require_native_close_retry_members_quiescent_tx(tx, &admitted.attempt_id).await?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        let progress = retained_close_retry_progress_tx(tx, &run).await?;
        if effect.resource.kind() != RetiredResourceKind::Worktree
            || i64::try_from(progress.successful.len() + progress.pending.len()).ok()
                != Some(plan.effect_count)
            || !progress.pending.contains(effect)
        {
            return Err(refused(
                "native retry dispatch requires a pending full-identity worktree effect",
            ));
        }
        Ok(plan.snapshot)
    }

    async fn require_native_close_retry_pending_tmux_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
    ) -> DbResult<CloseRetirementSnapshot> {
        if effect.resource.kind() != RetiredResourceKind::TmuxServer {
            return Err(refused(
                "native retry tmux entry requires a tmux server identity",
            ));
        }
        let plan = Self::require_native_close_retry_plan_identity_tx(tx, command, admitted).await?;
        if !plan.running {
            return Err(refused(
                "native retry tmux entry requires its exact latest running plan",
            ));
        }
        Self::require_native_close_retry_members_quiescent_tx(tx, &admitted.attempt_id).await?;
        let progress = retained_close_retry_progress_tx(
            tx,
            &CloseRunRef {
                attempt_id: admitted.attempt_id.clone(),
                ordinal: admitted.run_ordinal,
            },
        )
        .await?;
        if i64::try_from(progress.successful.len() + progress.pending.len()).ok()
            != Some(plan.effect_count)
            || !progress.pending.contains(effect)
        {
            return Err(refused(
                "native retry tmux entry requires its pending full-identity effect",
            ));
        }
        Ok(plan.snapshot)
    }

    async fn require_native_close_retry_pending_dispatch_tx(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        kind: NativeCloseRetryDispatchKind,
    ) -> DbResult<CloseRetirementSnapshot> {
        match kind {
            NativeCloseRetryDispatchKind::Worktree => {
                Self::require_native_close_retry_pending_worktree_tx(tx, command, admitted, effect)
                    .await
            }
            NativeCloseRetryDispatchKind::Tmux => {
                Self::require_native_close_retry_pending_tmux_tx(tx, command, admitted, effect)
                    .await
            }
        }
    }

    async fn record_native_close_retry_retirement_dispatch_tx<F, L>(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        entry: F,
        lease: &mut Option<L>,
    ) -> DbResult<()>
    where
        F: FnOnce() -> DbResult<L>,
    {
        let kind = NativeCloseRetryDispatchKind::for_resource(&effect.resource)?;
        let snapshot = Self::require_native_close_retry_pending_dispatch_tx(
            tx, command, admitted, effect, kind,
        )
        .await?;
        *lease = Some(entry()?);
        Self::insert_close_retirement_dispatch_tx(
            tx,
            &RecordCloseRetirementDispatchRequest {
                attempt_id: admitted.attempt_id.clone(),
                scope: effect.scope.clone(),
                snapshot,
                resource: effect.resource.clone(),
            },
        )
        .await
    }

    /// Retires one pending original `WorkScope` under the live native retry authority.
    /// The caller supplies runtime inventory evidence; success is recorded separately.
    ///
    /// # Errors
    /// Refuses stale source, mismatched plan or original identity, incomplete resource
    /// prerequisites, and empty reasons. Errors are classified after rollback; commit
    /// or rollback uncertainty returns `DurableFactUnclassified`.
    pub async fn retire_work_scope_for_native_close_retry(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        precondition: WorkScopeRetirementPrecondition,
        reason: &str,
    ) -> LocalAuthorityResult<DbResult<WorkScopeRetirementOutcome>> {
        self.retire_work_scope_for_native_close_retry_with_entry(
            command,
            admitted,
            effect,
            precondition,
            reason,
            || Ok(()),
        )
        .await
        .0
    }

    /// Holds a successful entry lease through commit or rollback and returns it
    /// even if release is unclassified. The callback has the same nonblocking
    /// and external-supervision contract as the metadata entry APIs.
    pub async fn retire_work_scope_for_native_close_retry_with_entry<F, L>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        precondition: WorkScopeRetirementPrecondition,
        reason: &str,
        entry: F,
    ) -> (
        LocalAuthorityResult<DbResult<WorkScopeRetirementOutcome>>,
        Option<L>,
    )
    where
        F: FnOnce() -> DbResult<L>,
    {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => {
                return (
                    LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
                    None,
                )
            }
        };
        let mut lease = None;
        let result = Self::retire_work_scope_for_native_close_retry_tx(
            &mut tx,
            command,
            admitted,
            effect,
            precondition,
            reason,
            entry,
            &mut lease,
        )
        .await;
        let committed = matches!(&result, Ok(WorkScopeRetirementOutcome::Retired));
        let finalized = if committed {
            tx.commit().await
        } else {
            tx.rollback().await
        };
        let classification = match finalized {
            Ok(()) => LocalAuthorityResult::DurableFactEstablished(result),
            Err(_) => LocalAuthorityResult::DurableFactUnclassified,
        };
        (classification, lease)
    }

    async fn retire_work_scope_for_native_close_retry_tx<F, L>(
        tx: &mut Transaction<'_, Sqlite>,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        precondition: WorkScopeRetirementPrecondition,
        reason: &str,
        entry: F,
        lease: &mut Option<L>,
    ) -> DbResult<WorkScopeRetirementOutcome>
    where
        F: FnOnce() -> DbResult<L>,
    {
        let plan = Self::require_native_close_retry_plan_identity_tx(tx, command, admitted).await?;
        if !plan.running {
            return Err(refused(
                "native WorkScope retirement requires its latest running plan",
            ));
        }
        Self::require_native_close_retry_members_quiescent_tx(tx, &admitted.attempt_id).await?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        let progress = retained_close_retry_progress_tx(tx, &run).await?;
        if effect.resource.kind() != RetiredResourceKind::WorkScope
            || !matches!(effect.resource.identity(), LossItemIdentity::Opaque(identity)
                if identity.as_str() == effect.scope.as_str())
            || &effect.scope != precondition.scope_id()
            || i64::try_from(progress.successful.len() + progress.pending.len()).ok()
                != Some(plan.effect_count)
            || !progress.pending.contains(effect)
        {
            return Err(refused(
                "native WorkScope retirement requires its pending full identity",
            ));
        }
        let NativeCloseIntent::ExplicitRetry {
            failed_run_ordinal, ..
        } = command.intent()
        else {
            return Err(refused(
                "native WorkScope retirement requires explicit retry intent",
            ));
        };
        let original = Self::read_native_close_retry_authority_tx(
            tx,
            &CloseRunRef {
                attempt_id: admitted.attempt_id.clone(),
                ordinal: *failed_run_ordinal,
            },
            &plan,
        )
        .await?;
        if original.snapshot != plan.snapshot
            || !original.resources.iter().any(|expected| {
                expected.scope == effect.scope && expected.resource == effect.resource
            })
        {
            return Err(refused(
                "native WorkScope retirement requires exact original proof",
            ));
        }
        let pending = &original.pending;
        if pending.iter().any(|required| {
            required.scope == effect.scope
                && required.resource.kind() != RetiredResourceKind::WorkScope
                && !progress.successful.contains(required)
        }) {
            return Err(refused(
                "native WorkScope retirement has unresolved original resources",
            ));
        }
        if reason.trim().is_empty() {
            return Err(DbError::Serialization(
                "work scope retirement reason must not be empty".to_owned(),
            ));
        }
        let coordinator_owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM coordinator_work_scope WHERE work_scope_id = ?1)",
        )
        .bind(precondition.scope_id().as_str())
        .fetch_one(&mut **tx)
        .await?;
        if coordinator_owned {
            return Ok(WorkScopeRetirementOutcome::Blocked(
                WorkScopeRetirementBlocker::CoordinatorOwned,
            ));
        }
        *lease = Some(entry()?);
        Self::retire_work_scope_lifecycle_tx(tx, precondition, reason, Some(&admitted.attempt_id))
            .await
    }

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
        )
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
        let CloseCleanupFailureAuthority::ExpectedResource {
            scope,
            snapshot,
            resource,
        } = &request.authority
        else {
            return Err(refused(
                "native resource failure requires expected-resource authority",
            ));
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
            || request
                .stop_certainty
                .confirmed_at_us()
                .is_some_and(|confirmed| confirmed < 0 || confirmed > request.occurred_at_us)
        {
            return Err(refused("native resource failure requires exact original authority and valid occurrence evidence"));
        }
        let progress = retained_close_retry_progress_tx(tx, &run).await?;
        let failed = CloseSafeRetryEffect {
            scope: scope.clone(),
            resource: resource.clone(),
        };
        if i64::try_from(progress.successful.len() + progress.pending.len()).ok()
            != Some(plan.effect_count)
            || !progress.pending.contains(&failed)
        {
            return Err(refused(
                "native resource failure requires a pending full-identity planned effect",
            ));
        }
        let remaining: Vec<_> = progress
            .pending
            .iter()
            .map(|effect| CloseCleanupFailureResource {
                scope: effect.scope.clone(),
                resource: effect.resource.clone(),
                disposition: if effect == &failed {
                    CloseCleanupResourceDisposition::Failed
                } else {
                    CloseCleanupResourceDisposition::Unattempted
                },
            })
            .collect();
        if request.remaining_resources != remaining {
            return Err(refused(
                "native failure children must equal the ordered pending original plan",
            ));
        }
        if matches!(
            request.stop_certainty,
            CloseStopCertainty::ConversationAndProcessesStopped { .. }
        ) {
            Self::require_native_close_original_process_proof_tx(tx, &run, &plan.snapshot).await?;
        }
        let obligation = close_obligation_for_update(tx, run.attempt_id.as_str()).await?;
        if !plan.running {
            let ended_at_us: Option<i64> = sqlx::query_scalar(
                "SELECT ended_at_us FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2",
            )
            .bind(run.attempt_id.as_str())
            .bind(run.ordinal.get())
            .fetch_one(&mut **tx)
            .await?;
            if ended_at_us != Some(request.occurred_at_us)
                || !Self::retained_close_cleanup_failure_matches_tx(tx, &run, request).await?
            {
                return Err(refused("native resource failure replay conflicts with the retained terminal occurrence"));
            }
            return Ok(obligation);
        }
        if matches!(
            request.stop_certainty,
            CloseStopCertainty::ConversationAndProcessesStopped { .. }
        ) {
            Self::require_native_close_retry_members_quiescent_tx(tx, &run.attempt_id).await?;
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
            return Err(refused(
                "confirmed native stop requires exact original-generation process proof",
            ));
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
                    run.status = 'running' AS running, run.created_at_us,
                    EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans capture
                        WHERE capture.attempt_id = plan.attempt_id AND capture.run_ordinal = plan.run_ordinal)
                        AS captured_origin FROM native_close_retry_plans plan
             JOIN native_close_retry_snapshot_authorities anchor ON anchor.attempt_id = plan.attempt_id
               AND anchor.run_ordinal = plan.run_ordinal
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
            captured_origin: row.try_get("captured_origin")?,
            effect_count: row.try_get("effect_count")?,
            snapshot: CloseRetirementSnapshot::parse(
                row.try_get::<String, _>("inspection_generation")?,
                row.try_get::<String, _>("inspection_fingerprint")?,
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?,
            running: row.try_get("running")?,
            created_at_us: row.try_get("created_at_us")?,
        })
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use phoenix_core::domain::close::{CapturedWorktreeIdentity, CloseAttemptId, GitPathIdentity};

    #[test]
    fn aggregate_component_requires_exact_canonical_byte_lengths_and_unique_scopes() {
        let decoded = decode_snapshot_component("v11:a2:é1:b3:x:y").unwrap();
        assert_eq!(decoded[&WorkScopeId::parse("a").unwrap()], "é");
        assert_eq!(decoded[&WorkScopeId::parse("b").unwrap()], "x:y");
        for malformed in [
            "",
            "v1",
            "v21:a1:g",
            "v11:a1:g!",
            "v11:a1:g1:a1:h",
            "v11:b1:g1:a1:h",
            "v11:a2:g",
            "v11:a1:é",
            "v11:é1:g",
            "v199999999999999999999999999999999999999:a1:g",
            "v101:a1:g",
            "v1+1:a1:g",
            "v10:1:g",
            "v11:a",
            "v11:a1:g1:b",
        ] {
            assert!(
                decode_snapshot_component(malformed).is_err(),
                "{malformed:?}"
            );
        }
    }

    #[test]
    fn original_snapshots_require_matching_captured_scopes_and_nonempty_values() {
        let mut scope = CloseAttemptScope {
            attempt_id: CloseAttemptId::parse("snapshot-codec").unwrap(),
            scope: WorkScopeId::parse("a").unwrap(),
            captured_worktree: Some(CapturedWorktreeIdentity::Unresolved {
                locator: GitPathIdentity::from_bytes(b"/snapshot-codec".to_vec()),
            }),
            captured_at: chrono::Utc::now(),
        };
        let original = CloseRetirementSnapshot::parse("v11:a2:é", "v11:a3:x:y").unwrap();
        let decoded = original_worktree_snapshots(&original, std::slice::from_ref(&scope)).unwrap();
        assert_eq!(
            decoded,
            vec![(
                CloseRetirementTarget {
                    scope: scope.scope.clone()
                },
                CloseRetirementSnapshot::parse("é", "x:y").unwrap(),
            )]
        );
        for (generation, fingerprint) in [
            ("v11:a1:g", "v11:b1:f"),
            ("v11:b1:g", "v11:b1:f"),
            ("v11:a0:", "v11:a1:f"),
            ("v11:a1:g", "v11:a0:"),
            ("no-worktree", "no-worktree"),
            ("no-worktree", "v11:a1:f"),
            ("v11:a1:g", "no-worktree"),
        ] {
            let snapshot = CloseRetirementSnapshot::parse(generation, fingerprint).unwrap();
            assert!(original_worktree_snapshots(&snapshot, std::slice::from_ref(&scope)).is_err());
        }
        let empty = CloseRetirementSnapshot::parse("no-worktree", "no-worktree").unwrap();
        assert!(original_worktree_snapshots(&empty, &[]).unwrap().is_empty());
        scope.captured_worktree = None;
        assert!(
            original_worktree_snapshots(&empty, std::slice::from_ref(&scope))
                .unwrap()
                .is_empty()
        );
        assert!(original_worktree_snapshots(&original, &[scope]).is_err());
    }
}
