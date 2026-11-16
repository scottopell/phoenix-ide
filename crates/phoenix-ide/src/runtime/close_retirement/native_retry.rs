use phoenix_core::domain::close::{
    CloseRunRef, CloseStopCertainty, LossItemIdentity, RetiredResourceKind,
    RetirementFailureReason, RetirementOutcome,
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
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::{runtime::Handle, task};

use crate::db::{
    BindCloseWorktreeFinalTombstoneObjectRequest, BindCloseWorktreeFinalTombstoneRequest,
    RecordCloseWorktreeCleanupPlanRequest,
};

#[derive(Debug)]
pub(super) enum NativeRetryExecutionError {
    TerminalizedFailure(String),
    Unclassified,
}

impl From<String> for NativeRetryExecutionError {
    fn from(_: String) -> Self {
        Self::Unclassified
    }
}

fn classify_native_retry_execution(
    result: Result<(), NativeRetryExecutionError>,
) -> LocalAuthorityResult<Result<(), String>> {
    match result {
        Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(())),
        Err(NativeRetryExecutionError::TerminalizedFailure(detail)) => {
            LocalAuthorityResult::DurableFactEstablished(Err(detail))
        }
        Err(NativeRetryExecutionError::Unclassified) => {
            LocalAuthorityResult::DurableFactUnclassified
        }
    }
}

impl super::RuntimeManager {
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

    async fn execute_native_close_retry_inner(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
    ) -> Result<(), NativeRetryExecutionError> {
        let context = self
            .db()
            .read_native_close_retry_execution_context(command, admitted)
            .await
            .map_err(|_| NativeRetryExecutionError::Unclassified)?;
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
                        .await?
                }
                RetiredResourceKind::Worktree => {
                    self.native_retry_worktree(command, admitted, &context, &run, effect)
                        .await?
                }
                RetiredResourceKind::WorkScope => {
                    self.native_retry_scope(command, admitted, &context, effect)
                        .await?
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

    async fn native_retry_tmux(
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
                    .complete_retirement(&permit)
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
        if let Err(detail) = self.native_retry_no_live_resources(&effect.scope).await {
            return self
                .native_retry_failure(command, admitted, context, effect, &detail)
                .await;
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
    async fn native_retry_worktree(
        &self,
        command: &NativeCloseCommand,
        admitted: &NativeCloseRunRef,
        context: &NativeCloseRetryExecutionContext,
        run: &CloseRunRef,
        effect: &CloseSafeRetryEffect,
    ) -> Result<(), NativeRetryExecutionError> {
        if let Err(detail) = self.native_retry_no_live_resources(&effect.scope).await {
            return self
                .native_retry_failure(command, admitted, context, effect, &detail)
                .await;
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
            .map_err(|_| NativeRetryExecutionError::Unclassified)?;
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
            .map_err(|_| NativeRetryExecutionError::Unclassified)?
        {
            plan
        } else {
            let inspection_path = if root
                .try_exists()
                .map_err(|_| NativeRetryExecutionError::Unclassified)?
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
            self.native_retry_metadata(
                command,
                admitted,
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
                            administrative_dir_incarnation: administrative_dir_incarnation
                                .clone(),
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

    fn native_retry_metadata(
        &self,
        _command: &NativeCloseCommand,
        _admitted: &NativeCloseRunRef,
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

    async fn native_retry_no_live_resources(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> Result<(), String> {
        use phoenix_core::domain::work_scope_inventory::{
            BrowserSessionLiveness, TmuxServerStatus,
        };
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
        if inventory.bash.iter().any(|handle| handle.state.is_live())
            || inventory
                .tmux
                .is_some_and(|tmux| tmux.status != TmuxServerStatus::Gone)
            || inventory
                .browser
                .is_some_and(|browser| browser.state != BrowserSessionLiveness::TornDown)
            || self.terminals.get(&key).is_some()
        {
            return Err(
                "native retry scope still has a live runtime resource; target preserved".into(),
            );
        }
        Ok(())
    }

    async fn native_retry_record_success(
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

    async fn native_retry_failure<T>(
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
