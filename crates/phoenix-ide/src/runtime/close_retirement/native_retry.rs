#[cfg(test)]
use phoenix_core::domain::close::RetiredResourceKind;
use phoenix_core::domain::close::{
    CloseRunRef, CloseStopCertainty, LossItemIdentity, RetirementFailureReason, RetirementOutcome,
};
use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeRetirementPrecondition};
use phoenix_db::native_close::{
    NativeCloseCommand, NativeCloseRetryExecutionContext, NativeCloseRunRef,
};
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{
    CloseCleanupFailureAuthority, CloseCleanupFailureResource, CloseCleanupResourceDisposition,
    CloseSafeRetryEffect, TerminalizeInitialCloseCleanupFailureRequest,
};
use phoenix_tools::tmux::registry::{
    PersistentTmuxDiscovery, TmuxRetirementRehydration, TmuxServerInstanceIdentity,
};
#[cfg(test)]
use std::path::Path;
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::{runtime::Handle, task};

#[cfg(test)]
use crate::db::{
    BindCloseWorktreeFinalTombstoneObjectRequest, BindCloseWorktreeFinalTombstoneRequest,
    RecordCloseWorktreeCleanupPlanRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeScopeRuntimeDrain {
    Drained,
    Blocked,
    Unclassified,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NativeRetryNoLiveResourcesError {
    Blocked,
    Unclassified,
}

fn native_retry_no_live_resources_result(
    drain: NativeScopeRuntimeDrain,
) -> Result<(), NativeRetryNoLiveResourcesError> {
    match drain {
        NativeScopeRuntimeDrain::Drained => Ok(()),
        NativeScopeRuntimeDrain::Blocked => Err(NativeRetryNoLiveResourcesError::Blocked),
        NativeScopeRuntimeDrain::Unclassified => Err(NativeRetryNoLiveResourcesError::Unclassified),
    }
}

fn classify_native_scope_runtime_drain(
    has_live_other: bool,
    tmux: Option<phoenix_core::domain::work_scope_inventory::TmuxServerStatus>,
) -> NativeScopeRuntimeDrain {
    use phoenix_core::domain::work_scope_inventory::TmuxServerStatus;
    if has_live_other || tmux == Some(TmuxServerStatus::Live) {
        NativeScopeRuntimeDrain::Blocked
    } else if tmux == Some(TmuxServerStatus::NotProbed) {
        NativeScopeRuntimeDrain::Unclassified
    } else {
        NativeScopeRuntimeDrain::Drained
    }
}

use crate::runtime::executor::native_close_retry::NativeRetryExecutionError;

impl From<String> for NativeRetryExecutionError {
    fn from(detail: String) -> Self {
        unclassified_native_retry_error("execution", &detail)
    }
}

fn unclassified_native_retry_error(
    stage: &'static str,
    error: &impl std::fmt::Display,
) -> NativeRetryExecutionError {
    tracing::warn!(stage, %error, "native Close retry execution failed without terminal evidence");
    #[cfg(test)]
    eprintln!("native Close retry execution failed at {stage}: {error}");
    NativeRetryExecutionError::Unclassified
}

#[cfg(test)]
fn classify_native_retry_execution(
    result: Result<(), NativeRetryExecutionError>,
) -> LocalAuthorityResult<Result<(), String>> {
    match result {
        Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(())),
        Err(NativeRetryExecutionError::TerminalizedFailure(detail)) => {
            LocalAuthorityResult::DurableFactEstablished(Err(detail))
        }
        Err(
            NativeRetryExecutionError::Unclassified | NativeRetryExecutionError::ResourceFailure(_),
        ) => LocalAuthorityResult::DurableFactUnclassified,
    }
}

/// Outcome of proving that an initial native Close may retire a `WorkScope`.
/// DB read failures are kept distinct from an established failed precondition.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NativeInitialWorkScopePrecondition {
    Ready(WorkScopeRetirementPrecondition),
    Failed(String),
    Unclassified,
}

impl super::RuntimeManager {
    pub(crate) async fn native_initial_work_scope_precondition(
        &self,
        key: &ResourceScopeKey,
    ) -> NativeInitialWorkScopePrecondition {
        let ResourceScopeKey::Work(scope) = key else {
            return NativeInitialWorkScopePrecondition::Failed(
                "native initial Close requires a Work scope".into(),
            );
        };
        match self
            .db()
            .work_scope_has_unresolved_product_ownership(scope)
            .await
        {
            Ok(true) => {
                return NativeInitialWorkScopePrecondition::Failed(
                    "work scope has unresolved ProductConversation ownership".into(),
                )
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "native initial WorkScope ownership check unclassified");
                return NativeInitialWorkScopePrecondition::Unclassified;
            }
        }
        match self.native_scope_runtime_drain(scope).await {
            NativeScopeRuntimeDrain::Drained => {}
            NativeScopeRuntimeDrain::Blocked => {
                return NativeInitialWorkScopePrecondition::Failed(
                    "initial Close scope still has a live runtime resource; target preserved"
                        .into(),
                );
            }
            NativeScopeRuntimeDrain::Unclassified => {
                return NativeInitialWorkScopePrecondition::Unclassified;
            }
        }
        NativeInitialWorkScopePrecondition::Ready(
            WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(
                scope.clone(),
            ),
        )
    }

    #[cfg(test)]
    pub(crate) async fn execute_native_close_retry(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> LocalAuthorityResult<Result<(), String>> {
        classify_native_retry_execution(
            self.execute_native_close_retry_inner(command, admitted)
                .await,
        )
    }

    #[cfg(test)]
    async fn execute_native_close_retry_inner(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> Result<(), NativeRetryExecutionError> {
        let context = self
            .db()
            .read_native_close_retry_execution_context(command, admitted)
            .await
            .map_err(|error| unclassified_native_retry_error("execution_context", &error))?;
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        // The retained immutable original-generation plan is authoritative. A fully
        // successful empty pending set still requires normal durable finalization.
        for index in super::close_safe_retry_execution_order(&context.progress.pending) {
            let effect = &context.progress.pending[index];
            match effect.resource.kind() {
                RetiredResourceKind::TmuxServer => {
                    self.native_retry_tmux(command, admitted, &context, effect)
                        .await?;
                }
                RetiredResourceKind::Worktree => {
                    self.native_retry_worktree(command, admitted, &context, &run, effect)
                        .await?;
                }
                RetiredResourceKind::WorkScope => {
                    self.native_retry_scope(command, admitted, &context, effect)
                        .await?;
                }
                _ => {
                    return self
                        .native_retry_failure(
                            command,
                            admitted,
                            &context,
                            effect,
                            "unsupported native retry resource kind",
                        )
                        .await
                }
            }
        }
        match self
            .db()
            .complete_native_close_retry(command, admitted)
            .await
        {
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(NativeRetryExecutionError::Unclassified)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                Err(error.to_string().into())
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(_)) => Ok(()),
        }
    }

    #[cfg(test)]
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn native_retry_tmux(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        context: &NativeCloseRetryExecutionContext,
        effect: &CloseSafeRetryEffect,
    ) -> Result<(), NativeRetryExecutionError> {
        let LossItemIdentity::Opaque(value) = effect.resource.identity() else {
            return self
                .native_retry_failure(
                    command,
                    admitted,
                    context,
                    effect,
                    "tmux identity is not opaque",
                )
                .await;
        };
        let Some(identity) = TmuxServerInstanceIdentity::parse_stable_identity(value.as_str())
        else {
            return self
                .native_retry_failure(
                    command,
                    admitted,
                    context,
                    effect,
                    "tmux identity is malformed",
                )
                .await;
        };
        let key = ResourceScopeKey::Work(effect.scope.clone());
        let captured = context
            .scopes
            .iter()
            .find(|s| s.scope == effect.scope)
            .ok_or(NativeRetryExecutionError::Unclassified)?;
        let legacy = captured.captured_worktree.as_ref().and_then(|wt| match wt {
            super::CapturedWorktreeIdentity::Resolved(wt) => Some(super::worktree_path(wt)),
            super::CapturedWorktreeIdentity::Unresolved { .. } => None,
        });
        let discovery = self
            .tmux_registry()
            .discover_persistent_identity(
                &key,
                legacy.as_deref(),
                None,
                phoenix_tools::tmux::registry::close_deadline(),
            )
            .await
            .map_err(|_| NativeRetryExecutionError::Unclassified)?;
        if !matches!(discovery, PersistentTmuxDiscovery::Exact(ref found) if found == &identity)
            && !matches!(
                discovery,
                PersistentTmuxDiscovery::EndpointAbsent | PersistentTmuxDiscovery::ServerAbsent
            )
        {
            return self
                .native_retry_failure(
                    command,
                    admitted,
                    context,
                    effect,
                    "tmux discovery found an ambiguous or different server",
                )
                .await;
        }
        match self
            .tmux_registry()
            .rehydrate_retirement(
                &key,
                &identity,
                phoenix_tools::tmux::registry::close_deadline(),
            )
            .await
            .map_err(|_| NativeRetryExecutionError::Unclassified)?
        {
            TmuxRetirementRehydration::Permit(permit) => {
                let outcome = self
                    .tmux_registry()
                    .complete_retirement(permit.as_ref())
                    .await
                    .map_err(|_| NativeRetryExecutionError::Unclassified)?;
                if !matches!(
                    super::tmux_retirement_outcome(outcome),
                    Ok(RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. })
                ) {
                    return self
                        .native_retry_failure(
                            command,
                            admitted,
                            context,
                            effect,
                            "tmux retirement remained residual",
                        )
                        .await;
                }
            }
            TmuxRetirementRehydration::AbsenceVerified => {}
            TmuxRetirementRehydration::Residual { reason } => {
                return self
                    .native_retry_failure(command, admitted, context, effect, &reason)
                    .await
            }
        }
        self.native_retry_record_success(
            command,
            admitted,
            effect,
            "exact native retry tmux retirement",
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_native_retry_tmux_with_source_entry(
        self: &Arc<Self>,
        command: NativeCloseCommand,
        admitted: NativeCloseRunRef,
        context: NativeCloseRetryExecutionContext,
        effect: CloseSafeRetryEffect,
        live_source: phoenix_core::domain::sm_state::ConvState,
        source_entry: crate::runtime::executor::SourceEntryWorker,
        deadline: std::time::Instant,
    ) -> tokio::task::JoinHandle<Result<(), NativeRetryExecutionError>> {
        use crate::runtime::executor::WorkerEntryResult;
        use phoenix_db::native_close::{
            NativeCloseInitialEntryRelease, NativeCloseInitialFirstEffectError,
        };
        let backend = Arc::clone(self);
        let runtime = Handle::current();
        task::spawn_blocking(move || {
            runtime.block_on(async {
            let LossItemIdentity::Opaque(value) = effect.resource.identity() else {
                return Err(NativeRetryExecutionError::ResourceFailure("tmux identity is not opaque".into()));
            };
            let identity = TmuxServerInstanceIdentity::parse_stable_identity(value.as_str())
                .ok_or_else(|| NativeRetryExecutionError::ResourceFailure("tmux identity is malformed".into()))?;
            let key = ResourceScopeKey::Work(effect.scope.clone());
            let captured = context.scopes.iter().find(|scope| scope.scope == effect.scope)
                .ok_or(NativeRetryExecutionError::Unclassified)?;
            let legacy = captured.captured_worktree.as_ref().and_then(|worktree| match worktree {
                super::CapturedWorktreeIdentity::Resolved(worktree) => Some(super::worktree_path(worktree)),
                super::CapturedWorktreeIdentity::Unresolved { .. } => None,
            });
            let expires = tokio::time::Instant::from_std(deadline);
            // Only read-only inventory is cancellable; the retained worker joins
            // registry preparation and completion using the same original deadline.
            let discovery = tokio::time::timeout_at(expires,
                backend.tmux_registry().discover_persistent_identity(&key, legacy.as_deref(), None, expires),
            ).await.map_err(|_| NativeRetryExecutionError::ResourceFailure("original tmux retry inventory deadline expired".into()))?
                .map_err(|_| NativeRetryExecutionError::Unclassified)?;
            if !matches!(discovery, PersistentTmuxDiscovery::Exact(ref found) if found == &identity)
                && !matches!(discovery, PersistentTmuxDiscovery::EndpointAbsent | PersistentTmuxDiscovery::ServerAbsent)
            {
                return Err(NativeRetryExecutionError::ResourceFailure("tmux discovery found an ambiguous or different server".into()));
            }
            let prepared = match backend.tmux_registry().rehydrate_retirement(&key, &identity, expires).await
                .map_err(|_| NativeRetryExecutionError::Unclassified)?
            {
                TmuxRetirementRehydration::Permit(permit) => Some(backend.tmux_registry()
                    .prepare_retirement(permit.as_ref()).await
                    .map_err(|outcome| NativeRetryExecutionError::ResourceFailure(format!("tmux preparation remained residual: {outcome:?}")))?),
                TmuxRetirementRehydration::AbsenceVerified => None,
                TmuxRetirementRehydration::Residual { reason } => return Err(NativeRetryExecutionError::ResourceFailure(reason)),
            };
            let (dispatch, lease) = backend.db().record_native_close_retry_retirement_dispatch_with_entry(
                &command, &admitted, &effect,
                || match source_entry.try_acquire_first_effect(&command) {
                    WorkerEntryResult::Ran(lease) if std::time::Instant::now() < deadline => Ok(lease),
                    _ => Err(crate::db::DbError::CloseFoundationPrecondition("original native retry source unavailable at tmux dispatch".into())),
                },
            ).await;
            if matches!(dispatch, LocalAuthorityResult::DurableFactUnclassified) {
                backend.fatal_local_authority_fence.close("native_retry_tmux_dispatch_release_unclassified");
                // Retain uncertain writer/source custody without preventing the
                // actor from joining this worker and taking its fatal exit.
                std::mem::forget(lease);
                std::mem::forget(source_entry);
                return Err(NativeRetryExecutionError::Unclassified);
            }
            drop(lease);
            match dispatch {
                LocalAuthorityResult::DurableFactEstablished(Ok(())) => {},
                LocalAuthorityResult::DurableFactEstablished(Err(error)) => return Err(NativeRetryExecutionError::ResourceFailure(error.to_string())),
                LocalAuthorityResult::DurableFactUnclassified => unreachable!(),
            }
            let Some(prepared) = prepared else { return Ok(()); };
            let outcome = prepared.complete_with_sink(|sink| {
                // Completion is polled inside Handle::block_on on this blocking worker.
                // Leave that async context before entering the writer-gated sink.
                tokio::task::block_in_place(|| runtime.block_on(async {
                    let mut lease = None;
                    let result = backend.db().with_native_close_retry_tmux_first_effect(
                        &command, &admitted, &live_source, effect.scope.clone(), context.snapshot.clone(), &effect.resource,
                        || match source_entry.try_acquire_first_effect(&command) {
                            WorkerEntryResult::Ran(acquired) if std::time::Instant::now() < deadline => {
                                lease = Some(acquired);
                                sink()
                            },
                            _ => Err("original native retry source unavailable at tmux sink".into()),
                        },
                    ).await;
                    match &result {
                        Ok((_, NativeCloseInitialEntryRelease::Unconfirmed(_)))
                        | Err(NativeCloseInitialFirstEffectError::Rejected { release: NativeCloseInitialEntryRelease::Unconfirmed(_), .. }) => {
                            backend.fatal_local_authority_fence.close("native_retry_tmux_sink_release_unclassified");
                            std::mem::forget(lease);
                            std::mem::forget(source_entry.clone());
                            return Err("native retry tmux writer release unconfirmed".into());
                        },
                        _ => {},
                    }
                    drop(lease);
                    match result {
                        Ok((outcome, NativeCloseInitialEntryRelease::Confirmed)) => outcome,
                        Err(error) => Err(format!("native retry tmux entry refused: {error:?}")),
                        Ok((_, NativeCloseInitialEntryRelease::Unconfirmed(_))) => unreachable!(),
                    }
                }))
            }).await.map_err(|_| NativeRetryExecutionError::Unclassified)?;
            match super::tmux_retirement_outcome(outcome) {
                Ok(RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. }) => Ok(()),
                _ => Err(NativeRetryExecutionError::ResourceFailure("tmux retirement remained residual".into())),
            }
        })
        })
    }

    pub(crate) fn spawn_native_retry_scope_with_source_entry(
        self: &Arc<Self>,
        command: NativeCloseCommand,
        admitted: NativeCloseRunRef,
        effect: CloseSafeRetryEffect,
        source_entry: crate::runtime::executor::SourceEntryWorker,
        deadline: std::time::Instant,
    ) -> tokio::task::JoinHandle<Result<(), NativeRetryExecutionError>> {
        let backend = Arc::clone(self);
        let runtime = Handle::current();
        task::spawn_blocking(move || {
            runtime.block_on(async {
            // Only read-only pre-entry work is cancellable at the original deadline.
            // The mutation below is outside this timeout and retains its source lease
            // through confirmed release (including deliberately retained unknown custody).
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                if backend.db().work_scope_has_unresolved_product_ownership(&effect.scope).await
                    .map_err(|_| NativeRetryExecutionError::Unclassified)? {
                    return Err(NativeRetryExecutionError::ResourceFailure("work scope has unresolved ProductConversation ownership".into()));
                }
                match backend.native_retry_no_live_resources(&effect.scope).await {
                    Ok(()) => Ok(()),
                    Err(NativeRetryNoLiveResourcesError::Blocked) => Err(NativeRetryExecutionError::ResourceFailure("native retry scope still has a live runtime resource; target preserved".into())),
                    Err(NativeRetryNoLiveResourcesError::Unclassified) => Err(NativeRetryExecutionError::Unclassified),
                }
            }).await.map_err(|_| NativeRetryExecutionError::Unclassified)??;
            let precondition = WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(effect.scope.clone());
            let (result, lease) = backend.db().retire_work_scope_for_native_close_retry_with_entry(
                &command, &admitted, &effect, precondition, "native explicit Close retry",
                || {
                    use crate::runtime::executor::WorkerEntryResult;
                    match source_entry.try_acquire_first_effect(&command) {
                        WorkerEntryResult::Ran(lease) if std::time::Instant::now() < deadline => Ok(lease),
                        _ => Err(crate::db::DbError::CloseFoundationPrecondition("original native retry source unavailable at WorkScope mutation".into())),
                    }
                },
            ).await;
            if matches!(result, LocalAuthorityResult::DurableFactUnclassified) {
                backend.fatal_local_authority_fence.close("native_retry_scope_release_unclassified");
                // The transaction could still own its writer; keep source custody fenced.
                std::mem::forget(lease);
                std::mem::forget(source_entry);
                return Err(NativeRetryExecutionError::Unclassified);
            }
            drop(lease);
            match result {
                LocalAuthorityResult::DurableFactEstablished(Ok(phoenix_core::work_scope::WorkScopeRetirementOutcome::Retired)) => {},
                LocalAuthorityResult::DurableFactEstablished(Ok(_)) => return Err(NativeRetryExecutionError::ResourceFailure("work scope retirement precondition was blocked".into())),
                LocalAuthorityResult::DurableFactEstablished(Err(error)) => return Err(NativeRetryExecutionError::ResourceFailure(error.to_string())),
                LocalAuthorityResult::DurableFactUnclassified => unreachable!(),
            }
            Ok(())
        })
        })
    }

    pub(crate) fn native_retry_close_writer_probe(
        &self,
        receipt: &phoenix_db::native_close::NativeCloseReceipt,
        effect: &CloseSafeRetryEffect,
        snapshot: &phoenix_core::domain::close::CloseRetirementSnapshot,
    ) -> super::CloseWriterProbe {
        super::CloseWriterProbe {
            persistence_uncertain: Some(Arc::new(AtomicBool::new(false))),
            #[cfg(test)]
            clean_writer_fixture: self.clean_close_writer_fixture,
            db: self.db().clone(),
            runtime: Handle::current(),
            run: CloseRunRef {
                attempt_id: receipt.admitted.attempt_id.clone(),
                ordinal: receipt.admitted.run_ordinal,
            },
            scope: effect.scope.clone(),
            snapshot: snapshot.clone(),
            resource: effect.resource.clone(),
        }
    }

    #[cfg(test)]
    async fn native_retry_scope(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        context: &NativeCloseRetryExecutionContext,
        effect: &CloseSafeRetryEffect,
    ) -> Result<(), NativeRetryExecutionError> {
        if self
            .db()
            .work_scope_has_unresolved_product_ownership(&effect.scope)
            .await
            .map_err(|_| NativeRetryExecutionError::Unclassified)?
        {
            return self
                .native_retry_failure(
                    command,
                    admitted,
                    context,
                    effect,
                    "work scope has unresolved ProductConversation ownership",
                )
                .await;
        }
        if let Err(error) = self.native_retry_no_live_resources(&effect.scope).await {
            return match error {
                NativeRetryNoLiveResourcesError::Blocked => {
                    self.native_retry_failure(
                        command,
                        admitted,
                        context,
                        effect,
                        "native retry scope still has a live runtime resource; target preserved",
                    )
                    .await
                }
                NativeRetryNoLiveResourcesError::Unclassified => {
                    Err(NativeRetryExecutionError::Unclassified)
                }
            };
        }
        let precondition =
            WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(
                effect.scope.clone(),
            );
        let retired = match self
            .db()
            .retire_work_scope_for_native_close_retry(
                command,
                admitted,
                effect,
                precondition,
                "native explicit Close retry",
            )
            .await
        {
            LocalAuthorityResult::DurableFactUnclassified => {
                return Err(NativeRetryExecutionError::Unclassified)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(e)) => {
                return Err(e.to_string().into())
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(outcome)) => outcome,
        };
        if retired != phoenix_core::work_scope::WorkScopeRetirementOutcome::Retired {
            return self
                .native_retry_failure(
                    command,
                    admitted,
                    context,
                    effect,
                    "work scope retirement precondition was blocked",
                )
                .await;
        }
        self.native_retry_record_success(
            command,
            admitted,
            effect,
            "independent native retry WorkScope retirement",
        )
        .await
    }

    // Keep original-plan binding, source fencing, and destructive callbacks in order.
    #[allow(clippy::too_many_lines)]
    #[cfg(test)]
    async fn native_retry_worktree(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        context: &NativeCloseRetryExecutionContext,
        run: &CloseRunRef,
        effect: &CloseSafeRetryEffect,
    ) -> Result<(), NativeRetryExecutionError> {
        if let Err(error) = self.native_retry_no_live_resources(&effect.scope).await {
            return match error {
                NativeRetryNoLiveResourcesError::Blocked => {
                    self.native_retry_failure(
                        command,
                        admitted,
                        context,
                        effect,
                        "native retry scope still has a live runtime resource; target preserved",
                    )
                    .await
                }
                NativeRetryNoLiveResourcesError::Unclassified => {
                    Err(NativeRetryExecutionError::Unclassified)
                }
            };
        }
        let scope = context
            .scopes
            .iter()
            .find(|s| s.scope == effect.scope)
            .ok_or(NativeRetryExecutionError::Unclassified)?;
        let identity = match scope.captured_worktree.as_ref() {
            Some(super::CapturedWorktreeIdentity::Resolved(identity)) => identity.clone(),
            _ => {
                return self
                    .native_retry_failure(
                        command,
                        admitted,
                        context,
                        effect,
                        "captured worktree identity is unresolved",
                    )
                    .await
            }
        };
        let original = context
            .worktree_snapshots
            .iter()
            .find(|(target, _)| target.scope == effect.scope)
            .map(|(_, snapshot)| snapshot.clone())
            .ok_or(NativeRetryExecutionError::Unclassified)?;
        let root = super::worktree_path(&identity);
        let quarantine = super::worktree_quarantine_path(&identity)
            .map_err(|error| unclassified_native_retry_error("worktree_quarantine_path", &error))?;
        // Cleanup metadata is a child of this exact dispatch. Establish its
        // original-generation parent before inserting a fresh plan; neither write
        // retires the worktree or establishes a successful outcome.
        match self
            .db()
            .record_native_close_retry_retirement_dispatch(command, admitted, effect)
            .await
        {
            LocalAuthorityResult::DurableFactUnclassified => {
                return Err(NativeRetryExecutionError::Unclassified)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                return self
                    .native_retry_failure(command, admitted, context, effect, &error.to_string())
                    .await
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(())) => {}
        }
        // A retained plan owns the administrative incarnation and final tombstone.
        // Never rediscover and replace it from a later filesystem generation.
        let plan = if let Some(plan) = self
            .db()
            .close_worktree_cleanup_plan(
                &admitted.attempt_id,
                &effect.scope,
                &context.snapshot,
                &effect.resource,
            )
            .await
            .map_err(|error| unclassified_native_retry_error("worktree_cleanup_plan", &error))?
        {
            plan
        } else {
            let inspection_path = if root
                .try_exists()
                .map_err(|error| unclassified_native_retry_error("worktree_exists", &error))?
            {
                &root
            } else {
                &quarantine
            };
            let common = super::exact_worktree_common_git_dir(inspection_path)?;
            let administrative_dir =
                super::exact_worktree_administrative_dir(inspection_path, &common)?;
            let administrative_dir_incarnation =
                super::observe_administrative_dir_incarnation(&administrative_dir)?;
            Self::native_retry_metadata(
                self.db()
                    .record_native_close_retry_worktree_cleanup_plan(
                        command,
                        admitted,
                        RecordCloseWorktreeCleanupPlanRequest {
                            attempt_id: admitted.attempt_id.clone(),
                            scope: effect.scope.clone(),
                            snapshot: context.snapshot.clone(),
                            resource: effect.resource.clone(),
                            administrative_dir: administrative_dir.clone(),
                            administrative_dir_incarnation: administrative_dir_incarnation.clone(),
                        },
                    )
                    .await,
            )?;
            crate::db::CloseWorktreeCleanupPlan {
                administrative_dir,
                administrative_dir_incarnation,
                final_tombstone: None,
            }
        };
        let runtime = Handle::current();
        let db = self.db().clone();
        let cmd = command.clone();
        let admission = admitted.clone();
        let scope_id = effect.scope.clone();
        let snapshot = context.snapshot.clone();
        let resource = effect.resource.clone();
        let unknown = Arc::new(AtomicBool::new(false));
        let writer_probe = super::CloseWriterProbe {
            persistence_uncertain: Some(unknown.clone()),
            #[cfg(test)]
            clean_writer_fixture: self.clean_close_writer_fixture,
            db: db.clone(),
            runtime: runtime.clone(),
            run: run.clone(),
            scope: effect.scope.clone(),
            snapshot: context.snapshot.clone(),
            resource: effect.resource.clone(),
        };
        let unknown_cb = unknown.clone();
        let bind = move |root: &Path,
                         root_id: (u64, u64),
                         object_id: Option<(u64, u64)>|
              -> Result<(), String> {
            if let Some((device, inode)) = object_id {
                match runtime.block_on(db.bind_native_close_retry_worktree_final_tombstone_object(
                    &cmd,
                    &admission,
                    BindCloseWorktreeFinalTombstoneObjectRequest {
                        attempt_id: admission.attempt_id.clone(),
                        scope: scope_id.clone(),
                        snapshot: snapshot.clone(),
                        resource: resource.clone(),
                        object_device: device,
                        object_inode: inode,
                    },
                )) {
                    LocalAuthorityResult::DurableFactEstablished(Ok(())) => return Ok(()),
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        return Err(error.to_string())
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        unknown_cb.store(true, Ordering::SeqCst);
                        return Err("native tombstone object bind unclassified".into());
                    }
                }
            }
            let tombstone = super::CloseWorktreeFinalTombstone {
                root: root.to_path_buf(),
                device: root_id.0,
                inode: root_id.1,
                object_device: object_id.map(|x| x.0),
                object_inode: object_id.map(|x| x.1),
            };
            match runtime.block_on(db.bind_native_close_retry_worktree_final_tombstone(
                &cmd,
                &admission,
                BindCloseWorktreeFinalTombstoneRequest {
                    attempt_id: admission.attempt_id.clone(),
                    scope: scope_id.clone(),
                    snapshot: snapshot.clone(),
                    resource: resource.clone(),
                    tombstone,
                },
            )) {
                LocalAuthorityResult::DurableFactEstablished(Ok(())) => Ok(()),
                LocalAuthorityResult::DurableFactEstablished(Err(e)) => Err(e.to_string()),
                LocalAuthorityResult::DurableFactUnclassified => {
                    unknown_cb.store(true, Ordering::SeqCst);
                    Err("native tombstone bind unclassified".into())
                }
            }
        };
        // No detached destructive task: the actor's gates and settlement guard
        // remain owned throughout this synchronous phase. Cancellation at the next
        // await leaves settlement armed until a durable terminal fact is known.
        let result = task::block_in_place(|| {
            super::probe_retry_worktree_writers_with(
                &root,
                &quarantine,
                plan.final_tombstone.as_ref(),
                |path| {
                    if !path.try_exists().map_err(|error| error.to_string())? {
                        return Ok(super::RetryTargetObservation::Absent);
                    }
                    writer_probe
                        .observe(path, super::quarantine_has_external_writer)
                        .map(super::RetryTargetObservation::Present)
                },
            )?;
            if !root.try_exists().map_err(|error| error.to_string())?
                && !quarantine.try_exists().map_err(|error| error.to_string())?
            {
                let tombstone = plan.final_tombstone.as_ref().ok_or_else(|| {
                    "absent worktree has no retained final tombstone; target preserved".to_string()
                })?;
                match super::resume_final_worktree_tombstone(
                    tombstone,
                    &identity,
                    &original,
                    writer_probe,
                ) {
                    super::FinalTombstoneRecovery::Completed => {}
                    super::FinalTombstoneRecovery::Residual(detail) => return Err(detail),
                }
                super::complete_persisted_worktree_administrative_cleanup(
                    &identity,
                    &plan.administrative_dir,
                    &plan.administrative_dir_incarnation,
                )?;
                return Ok(super::ExactWorktreeRemoval::Retired);
            }
            super::inspect_and_remove_exact_worktree_with_hook_and_plan(
                &Handle::current(),
                &identity,
                &original,
                &plan.administrative_dir,
                &plan.administrative_dir_incarnation,
                plan.final_tombstone.as_ref(),
                bind,
                |_| {},
                super::CloseWorktreeEffects::production(writer_probe),
            )
        });
        if unknown.load(Ordering::SeqCst) {
            return Err(NativeRetryExecutionError::Unclassified);
        }
        let removed = match result {
            Ok(super::ExactWorktreeRemoval::Retired) => true,
            Ok(
                super::ExactWorktreeRemoval::StopFailed { detail }
                | super::ExactWorktreeRemoval::Residual { detail }
                | super::ExactWorktreeRemoval::ReinspectionRequired { detail },
            )
            | Err(detail) => {
                return self
                    .native_retry_failure(command, admitted, context, effect, &detail)
                    .await
            }
        };
        if removed {
            self.native_retry_record_success(
                command,
                admitted,
                effect,
                "exact original-snapshot native worktree cleanup",
            )
            .await
        } else {
            unreachable!()
        }
    }

    #[cfg(test)]
    fn native_retry_metadata(
        result: LocalAuthorityResult<crate::db::DbResult<()>>,
    ) -> Result<(), NativeRetryExecutionError> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(Ok(())) => Ok(()),
            LocalAuthorityResult::DurableFactEstablished(Err(e)) => Err(e.to_string().into()),
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(NativeRetryExecutionError::Unclassified)
            }
        }
    }

    pub(crate) async fn native_retry_no_live_resources(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> Result<(), NativeRetryNoLiveResourcesError> {
        native_retry_no_live_resources_result(self.native_scope_runtime_drain(scope).await)
    }

    async fn native_scope_runtime_drain(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> NativeScopeRuntimeDrain {
        use phoenix_core::domain::work_scope_inventory::BrowserSessionLiveness;
        let key = ResourceScopeKey::Work(scope.clone());
        let inventory = phoenix_tools::work_scope_inventory::assemble_inventory(
            &key,
            None,
            true,
            self.bash_handles(),
            self.tmux_registry(),
            self.browser_sessions(),
        )
        .await;
        let has_live_other = inventory.bash.iter().any(|handle| handle.state.is_live())
            || inventory
                .browser
                .is_some_and(|browser| browser.state != BrowserSessionLiveness::TornDown)
            || self.terminals.get(&key).is_some();
        classify_native_scope_runtime_drain(has_live_other, inventory.tmux.map(|tmux| tmux.status))
    }

    pub(crate) async fn native_retry_record_success(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        effect: &CloseSafeRetryEffect,
        detail: &str,
    ) -> Result<(), NativeRetryExecutionError> {
        match self
            .db()
            .record_native_close_retry_success(command, admitted, effect, detail)
            .await
        {
            LocalAuthorityResult::DurableFactEstablished(Ok(())) => Ok(()),
            LocalAuthorityResult::DurableFactEstablished(Err(e)) => Err(e.to_string().into()),
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(NativeRetryExecutionError::Unclassified)
            }
        }
    }

    pub(crate) async fn native_retry_failure<T>(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        context: &NativeCloseRetryExecutionContext,
        failed: &CloseSafeRetryEffect,
        detail: &str,
    ) -> Result<T, NativeRetryExecutionError> {
        let run = CloseRunRef {
            attempt_id: admitted.attempt_id.clone(),
            ordinal: admitted.run_ordinal,
        };
        let progress = self
            .db()
            .read_native_close_retry_execution_context(command, admitted)
            .await
            .map_err(|_| NativeRetryExecutionError::Unclassified)?
            .progress;
        if !progress.pending.contains(failed) {
            return Err(NativeRetryExecutionError::Unclassified);
        }
        let remaining_resources = progress
            .pending
            .iter()
            .map(|effect| CloseCleanupFailureResource {
                scope: effect.scope.clone(),
                resource: effect.resource.clone(),
                disposition: if effect == failed {
                    CloseCleanupResourceDisposition::Failed
                } else {
                    CloseCleanupResourceDisposition::Unattempted
                },
            })
            .collect();
        let occurred_at_us = chrono::Utc::now().timestamp_micros();
        let request = TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: run.failure_occurrence_id(),
            attempt_id: admitted.attempt_id.clone(),
            source_product_conversation_id: admitted.target.clone(),
            authority: CloseCleanupFailureAuthority::ExpectedResource {
                scope: failed.scope.clone(),
                snapshot: context.snapshot.clone(),
                resource: failed.resource.clone(),
            },
            remaining_resources,
            reason: RetirementFailureReason::IdentityNotProven,
            detail: detail.to_string(),
            stop_certainty: CloseStopCertainty::ShutdownUncertain,
            occurred_at_us,
        };
        match self
            .db()
            .terminalize_native_close_retry_resource_failure(command, admitted, request)
            .await
        {
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(NativeRetryExecutionError::Unclassified)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(e)) => Err(e.to_string().into()),
            LocalAuthorityResult::DurableFactEstablished(Ok(_)) => Err(
                NativeRetryExecutionError::TerminalizedFailure(detail.to_owned()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprobed_tmux_cannot_establish_initial_scope_drain_or_failure() {
        use phoenix_core::domain::work_scope_inventory::TmuxServerStatus;
        assert_eq!(
            classify_native_scope_runtime_drain(false, Some(TmuxServerStatus::NotProbed)),
            NativeScopeRuntimeDrain::Unclassified,
        );
        assert_eq!(
            classify_native_scope_runtime_drain(false, Some(TmuxServerStatus::Live)),
            NativeScopeRuntimeDrain::Blocked,
        );
        for tmux in [None, Some(TmuxServerStatus::Gone)] {
            assert_eq!(
                classify_native_scope_runtime_drain(false, tmux),
                NativeScopeRuntimeDrain::Drained,
            );
        }
        assert_eq!(
            classify_native_scope_runtime_drain(true, Some(TmuxServerStatus::NotProbed)),
            NativeScopeRuntimeDrain::Blocked,
        );
    }

    #[test]
    fn retry_runtime_unknown_remains_unclassified_while_blocked_is_distinct() {
        assert_eq!(
            native_retry_no_live_resources_result(NativeScopeRuntimeDrain::Drained),
            Ok(()),
        );
        assert_eq!(
            native_retry_no_live_resources_result(NativeScopeRuntimeDrain::Blocked),
            Err(NativeRetryNoLiveResourcesError::Blocked),
        );
        let Err(NativeRetryNoLiveResourcesError::Unclassified) =
            native_retry_no_live_resources_result(NativeScopeRuntimeDrain::Unclassified)
        else {
            panic!("unknown runtime inventory must remain unclassified");
        };
        assert!(matches!(
            classify_native_retry_execution(Err(NativeRetryExecutionError::Unclassified)),
            LocalAuthorityResult::DurableFactUnclassified
        ));
    }

    #[test]
    fn execution_diagnostics_do_not_establish_terminal_authority() {
        for stage in [
            "execution_context",
            "worktree_quarantine_path",
            "worktree_cleanup_plan",
            "worktree_exists",
        ] {
            let error = unclassified_native_retry_error(stage, &"injected execution error");
            assert!(matches!(
                classify_native_retry_execution(Err(error)),
                LocalAuthorityResult::DurableFactUnclassified
            ));
        }
    }

    #[test]
    fn native_retry_only_terminal_facts_release_settlement_authority() {
        assert!(matches!(
            classify_native_retry_execution(Ok(())),
            LocalAuthorityResult::DurableFactEstablished(Ok(()))
        ));
        assert!(matches!(classify_native_retry_execution(Err(
            NativeRetryExecutionError::TerminalizedFailure("persisted failure".into()))),
            LocalAuthorityResult::DurableFactEstablished(Err(detail)) if detail == "persisted failure"));
        for detail in [
            "context read failed",
            "success write failed after removal",
            "writer commit uncertain",
        ] {
            assert!(matches!(
                classify_native_retry_execution(Err(detail.to_string().into())),
                LocalAuthorityResult::DurableFactUnclassified
            ));
        }
    }
}
