//! Source-owned post-process continuation over the captured original inventory.

use super::source_entry::SourceEntryWorker;
#[cfg(unix)]
use crate::runtime::close_retirement::native_initial::worktree::{
    prepare_original_worktree_git_until, NativeWorktreeGit, OriginalWorktreeEffectContext,
    WorktreePhysicalObservation, WorktreePhysicalOutcome,
};
use phoenix_core::domain::close::{
    CloseExpectedRetirementResource, CloseObligation, CloseRetirementSnapshot, CloseRunRef,
    CloseStopCertainty, LossItemIdentity, RetiredResourceKind, RetirementFailureReason,
    RetirementOutcome, WorktreeIdentity,
};
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId, WorkScopeRetirementOutcome};
use phoenix_db::native_close::{NativeCloseCommand, NativeCloseReceipt};
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{
    CloseCleanupFailureAuthority, CloseCleanupResourceDisposition, Database, DbError, DbResult,
    RecordCloseRetirementDispatchRequest, RecordCloseRetirementEvidenceRequest,
    TerminalizeInitialCloseCleanupFailureRequest,
};

enum AuthoritativeCommand<T> {
    Established(T),
    FatalAuthority,
}

async fn run_authoritative_command<T, F, Fut>(
    stage: &mut Stage,
    operation: F,
) -> AuthoritativeCommand<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = LocalAuthorityResult<T>>,
{
    if matches!(stage, Stage::FatalAuthority) {
        return AuthoritativeCommand::FatalAuthority;
    }
    // A canceled actor await cannot prove the writer released. Keep a fatal
    // pending marker until the authoritative reply restores the prior stage.
    let prior = std::mem::replace(stage, Stage::FatalAuthority);
    match operation().await {
        LocalAuthorityResult::DurableFactEstablished(value) => {
            *stage = prior;
            AuthoritativeCommand::Established(value)
        }
        LocalAuthorityResult::DurableFactUnclassified => {
            *stage = Stage::FatalAuthority;
            AuthoritativeCommand::FatalAuthority
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostRetirementStep {
    Retiring,
    Completed(CloseObligation),
    Blocked,
    Unclassified,
    FatalAuthority,
}

#[derive(Debug)]
enum Stage {
    WorktreeDispatch,
    WorktreePreparing,
    WorktreePhysical,
    WorktreeEvidence,
    WorktreeFailure,
    Dispatch,
    Retiring,
    Finalizing,
    Completed(CloseObligation),
    Blocked,
    FatalAuthority,
}

// A captured Worktree remains an exact typed target. A missing path is not
// retirement proof; only the retained physical worker can establish success.
#[derive(Debug)]
struct OriginalWorktreeTarget {
    resource: CloseExpectedRetirementResource,
    identity: WorktreeIdentity,
}

fn select_original_worktrees(
    inventory: &[CloseExpectedRetirementResource],
    scopes: &[WorkScopeId],
) -> Result<Vec<OriginalWorktreeTarget>, String> {
    let mut targets = Vec::new();
    for scope in scopes {
        let worktrees_in_scope = inventory.iter().filter(|item| {
            item.scope == *scope && item.resource.kind() == RetiredResourceKind::Worktree
        });
        for item in worktrees_in_scope {
            let LossItemIdentity::Worktree(identity) = item.resource.identity() else {
                return Err("captured Worktree has no typed worktree identity".into());
            };
            if targets
                .iter()
                .any(|prior: &OriginalWorktreeTarget| prior.resource.scope == *scope)
            {
                return Err("captured scope has multiple Worktree resources".into());
            }
            targets.push(OriginalWorktreeTarget {
                resource: item.clone(),
                identity: identity.clone(),
            });
        }
    }
    if inventory.iter().any(|item| {
        item.resource.kind() == RetiredResourceKind::Worktree && !scopes.contains(&item.scope)
    }) {
        return Err("captured Worktree has no captured WorkScope".into());
    }
    Ok(targets)
}

pub(super) struct PostRetirement {
    original_inventory: Vec<CloseExpectedRetirementResource>,
    worktree_targets: Vec<OriginalWorktreeTarget>,
    scope_order: Vec<WorkScopeId>,
    snapshot: CloseRetirementSnapshot,
    scope_index: usize,
    worktree_index: usize,
    #[cfg(unix)]
    worktree_preparation:
        Option<tokio::task::JoinHandle<Result<Option<NativeWorktreeGit>, String>>>,
    #[cfg(unix)]
    physical: Option<WorktreePhysicalObservation>,
    pending_failure: Option<TerminalizeInitialCloseCleanupFailureRequest>,
    failure: Option<(RetirementFailureReason, String, i64)>,
    stage: Stage,
}

impl PostRetirement {
    pub(super) fn new(
        original_inventory: Vec<CloseExpectedRetirementResource>,
        scope_order: Vec<WorkScopeId>,
        snapshot: CloseRetirementSnapshot,
    ) -> Result<Self, String> {
        for (index, scope) in scope_order.iter().enumerate() {
            if scope_order[..index].contains(scope) {
                return Err("captured WorkScope order contains a duplicate".into());
            }
        }
        if original_inventory.iter().any(|resource| {
            resource.snapshot != snapshot
                || original_inventory
                    .first()
                    .is_some_and(|first| first.attempt_id != resource.attempt_id)
        }) {
            return Err("post-process cursor differs from captured attempt or snapshot".into());
        }
        let scope_resources: Vec<_> = original_inventory
            .iter()
            .filter(|resource| resource.resource.kind() == RetiredResourceKind::WorkScope)
            .collect();
        if scope_resources.len() != scope_order.len()
            || scope_order.iter().any(|scope| {
                scope_resources
                    .iter()
                    .filter(|resource| &resource.scope == scope)
                    .count()
                    != 1
            })
            || scope_resources.iter().any(|resource| {
                !matches!(resource.resource.identity(),
                phoenix_core::domain::close::LossItemIdentity::Opaque(identity)
                    if identity.as_str() == resource.scope.as_str())
            })
        {
            return Err("captured WorkScope resources do not match captured scope order".into());
        }
        let worktree_targets = select_original_worktrees(&original_inventory, &scope_order)?;
        let stage = if worktree_targets.is_empty() {
            Stage::Dispatch
        } else {
            Stage::WorktreeDispatch
        };
        Ok(Self {
            original_inventory,
            worktree_targets,
            scope_order,
            snapshot,
            scope_index: 0,
            worktree_index: 0,
            #[cfg(unix)]
            worktree_preparation: None,
            #[cfg(unix)]
            physical: None,
            pending_failure: None,
            failure: None,
            stage,
        })
    }

    fn current(&self) -> Option<&CloseExpectedRetirementResource> {
        if let Some(target) = self.worktree_targets.get(self.worktree_index) {
            return Some(&target.resource);
        }
        let scope = self.scope_order.get(self.scope_index)?;
        self.original_inventory.iter().find(|resource| {
            resource.scope == *scope && resource.resource.kind() == RetiredResourceKind::WorkScope
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // Orchestrates persisted stages while retaining the original inventory cursor.
    pub(super) async fn step(
        &mut self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        db: &Database,
        backend: &crate::runtime::RuntimeManager,
        live_source: &ConvState,
        source_entry: Option<&SourceEntryWorker>,
        original_deadline: Option<std::time::Instant>,
    ) -> DbResult<PostRetirementStep> {
        if matches!(&self.stage, Stage::Blocked) {
            return Ok(PostRetirementStep::Blocked);
        }
        if matches!(&self.stage, Stage::FatalAuthority) {
            return Ok(PostRetirementStep::FatalAuthority);
        }
        if let Stage::Completed(obligation) = &self.stage {
            return Ok(PostRetirementStep::Completed(obligation.clone()));
        }

        if matches!(&self.stage, Stage::Finalizing) {
            return match run_authoritative_command(&mut self.stage, || {
                db.complete_native_close_initial(command, receipt, live_source)
            })
            .await
            {
                AuthoritativeCommand::Established(Ok(obligation)) => {
                    self.stage = Stage::Completed(obligation.clone());
                    Ok(PostRetirementStep::Completed(obligation))
                }
                AuthoritativeCommand::Established(Err(error)) => Err(error),
                AuthoritativeCommand::FatalAuthority => Ok(PostRetirementStep::FatalAuthority),
            };
        }

        let Some(resource) = self.current().cloned() else {
            self.stage = Stage::Finalizing;
            return Ok(PostRetirementStep::Retiring);
        };
        if resource.attempt_id != receipt.admitted.attempt_id || resource.snapshot != self.snapshot
        {
            return Err(DbError::CloseFoundationPrecondition(
                "post-retirement cursor differs from admitted original inventory".into(),
            ));
        }

        if self.worktree_index < self.worktree_targets.len() {
            return self
                .step_worktree(
                    command,
                    receipt,
                    db,
                    backend,
                    live_source,
                    source_entry,
                    original_deadline,
                    resource,
                )
                .await;
        }

        if matches!(&self.stage, Stage::Dispatch) {
            return match run_authoritative_command(&mut self.stage, || {
                db.record_native_close_initial_retirement_dispatch(
                    command,
                    receipt,
                    live_source,
                    RecordCloseRetirementDispatchRequest {
                        attempt_id: resource.attempt_id.clone(),
                        scope: resource.scope.clone(),
                        snapshot: self.snapshot.clone(),
                        resource: resource.resource.clone(),
                    },
                )
            })
            .await
            {
                AuthoritativeCommand::Established(Ok(())) => {
                    self.stage = Stage::Retiring;
                    Ok(PostRetirementStep::Retiring)
                }
                AuthoritativeCommand::Established(Err(error)) => Err(error),
                AuthoritativeCommand::FatalAuthority => Ok(PostRetirementStep::FatalAuthority),
            };
        }

        // Reconcile the exact original proof before checking live preconditions:
        // a lost successful reply must not turn retirement into a new failure.
        let evidence = match db
            .list_close_retirement_evidence(resource.attempt_id.as_str())
            .await
        {
            Ok(evidence) => evidence,
            Err(error) => {
                tracing::warn!(%error, "native initial scope proof read unclassified");
                return Ok(PostRetirementStep::Unclassified);
            }
        };
        if evidence.iter().any(|proof| {
            proof.attempt_id == resource.attempt_id
                && proof.scope == resource.scope
                && proof.snapshot == resource.snapshot
                && proof.resource == resource.resource
                && matches!(
                    &proof.outcome,
                    phoenix_core::domain::close::RetirementOutcome::Retired
                        | phoenix_core::domain::close::RetirementOutcome::AbsenceAdopted { .. }
                )
        }) {
            self.scope_index += 1;
            self.stage = if self.scope_index == self.scope_order.len() {
                Stage::Finalizing
            } else {
                Stage::Dispatch
            };
            return Ok(PostRetirementStep::Retiring);
        }
        let precondition = match backend
            .native_initial_work_scope_precondition(&ResourceScopeKey::Work(resource.scope.clone()))
            .await
        {
            crate::runtime::close_retirement::NativeInitialWorkScopePrecondition::Ready(value) => {
                value
            }
            crate::runtime::close_retirement::NativeInitialWorkScopePrecondition::Failed(
                detail,
            ) => {
                tracing::warn!(%detail, "native initial WorkScope retirement blocked");
                self.stage = Stage::Blocked;
                return Ok(PostRetirementStep::Blocked);
            }
            crate::runtime::close_retirement::NativeInitialWorkScopePrecondition::Unclassified => {
                return Ok(PostRetirementStep::Unclassified);
            }
        };
        match run_authoritative_command(&mut self.stage, || {
            db.retire_work_scope_for_native_close_initial(
                command,
                receipt,
                live_source,
                resource.scope.clone(),
                self.snapshot.clone(),
                resource.resource.clone(),
                precondition,
                "native initial Close WorkScope retirement",
            )
        })
        .await
        {
            AuthoritativeCommand::Established(Ok(WorkScopeRetirementOutcome::Retired)) => {
                self.scope_index += 1;
                self.stage = if self.scope_index == self.scope_order.len() {
                    Stage::Finalizing
                } else {
                    Stage::Dispatch
                };
                Ok(PostRetirementStep::Retiring)
            }
            AuthoritativeCommand::Established(Ok(other)) => {
                tracing::warn!(scope = %resource.scope, ?other, "native initial Close WorkScope retirement blocked");
                self.stage = Stage::Blocked;
                Ok(PostRetirementStep::Blocked)
            }
            AuthoritativeCommand::Established(Err(error)) => Err(error),
            AuthoritativeCommand::FatalAuthority => Ok(PostRetirementStep::FatalAuthority),
        }
    }
}

impl PostRetirement {
    fn fail_worktree(&mut self, reason: RetirementFailureReason, detail: String) {
        self.failure = Some((reason, detail, chrono::Utc::now().timestamp_micros()));
        self.stage = Stage::WorktreeFailure;
    }

    #[cfg(unix)]
    fn accept_physical_outcome(&mut self, outcome: WorktreePhysicalOutcome) {
        match outcome {
            WorktreePhysicalOutcome::Retired => self.stage = Stage::WorktreeEvidence,
            WorktreePhysicalOutcome::StopFailed { detail } => {
                self.fail_worktree(RetirementFailureReason::RemovalFailed, detail);
            }
            WorktreePhysicalOutcome::ReinspectionRequired { detail } => {
                self.fail_worktree(RetirementFailureReason::IdentityNotProven, detail);
            }
            WorktreePhysicalOutcome::Residual { detail } => {
                self.fail_worktree(RetirementFailureReason::ManualRepairRequired, detail);
            }
        }
    }

    fn advance_worktree(&mut self) {
        self.worktree_index += 1;
        #[cfg(unix)]
        {
            self.physical = None;
            self.worktree_preparation = None;
        }
        self.stage = if self.worktree_index == self.worktree_targets.len() {
            Stage::Dispatch
        } else {
            Stage::WorktreeDispatch
        };
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)] // Exact Worktree proof is intentionally one ordered stage transition.
    async fn step_worktree(
        &mut self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        db: &Database,
        backend: &crate::runtime::RuntimeManager,
        live_source: &ConvState,
        source_entry: Option<&SourceEntryWorker>,
        original_deadline: Option<std::time::Instant>,
        resource: CloseExpectedRetirementResource,
    ) -> DbResult<PostRetirementStep> {
        if matches!(self.stage, Stage::WorktreeDispatch) {
            return match run_authoritative_command(&mut self.stage, || {
                db.record_native_close_initial_retirement_dispatch(
                    command,
                    receipt,
                    live_source,
                    RecordCloseRetirementDispatchRequest {
                        attempt_id: resource.attempt_id.clone(),
                        scope: resource.scope.clone(),
                        snapshot: resource.snapshot.clone(),
                        resource: resource.resource.clone(),
                    },
                )
            })
            .await
            {
                AuthoritativeCommand::Established(Ok(())) => {
                    self.stage = Stage::WorktreePreparing;
                    Ok(PostRetirementStep::Retiring)
                }
                AuthoritativeCommand::Established(Err(error)) => Err(error),
                AuthoritativeCommand::FatalAuthority => Ok(PostRetirementStep::FatalAuthority),
            };
        }

        if matches!(self.stage, Stage::WorktreeFailure) {
            return self
                .persist_worktree_failure(command, receipt, db, live_source, &resource)
                .await;
        }
        if matches!(self.stage, Stage::WorktreeEvidence) {
            return match run_authoritative_command(&mut self.stage, || {
                db.record_native_close_initial_retirement_evidence(
                    command,
                    receipt,
                    live_source,
                    RecordCloseRetirementEvidenceRequest {
                        attempt_id: resource.attempt_id.clone(),
                        scope: resource.scope.clone(),
                        snapshot: resource.snapshot.clone(),
                        resource: resource.resource.clone(),
                        outcome: RetirementOutcome::Retired,
                        detail: None,
                    },
                )
            })
            .await
            {
                AuthoritativeCommand::Established(Ok(())) => {
                    // Only persistence retries are permitted after the worker's
                    // Retired observation; physical execution is never repeated.
                    self.advance_worktree();
                    Ok(PostRetirementStep::Retiring)
                }
                AuthoritativeCommand::Established(Err(error)) => Err(error),
                AuthoritativeCommand::FatalAuthority => Ok(PostRetirementStep::FatalAuthority),
            };
        }

        #[cfg(unix)]
        {
            if matches!(self.stage, Stage::WorktreePreparing) {
                let (Some(source_entry), Some(deadline)) = (source_entry, original_deadline) else {
                    self.stage = Stage::FatalAuthority;
                    return Ok(PostRetirementStep::FatalAuthority);
                };
                if self.worktree_preparation.is_none() {
                    let identity = self.worktree_targets[self.worktree_index].identity.clone();
                    // Preparation binds the captured pointer and acquires the
                    // repository lock without spawning any pre-admission Git.
                    self.worktree_preparation = Some(tokio::task::spawn_blocking(move || {
                        prepare_original_worktree_git_until(&identity, deadline)
                    }));
                    return Ok(PostRetirementStep::Retiring);
                }
                let preparation = self
                    .worktree_preparation
                    .as_mut()
                    .expect("retained preparation");
                if !preparation.is_finished() {
                    // Keep the same preparation and stage while the actor services
                    // Stop/fatal events; never reprepare the original target.
                    return Ok(PostRetirementStep::Retiring);
                }
                let prepared = preparation.await;
                self.worktree_preparation = None;
                let prepared = match prepared {
                    Ok(Ok(Some(prepared))) => prepared,
                    Ok(Ok(None)) => {
                        self.fail_worktree(RetirementFailureReason::IdentityNotProven,
                            "captured original Worktree path is absent without physical retirement proof".into());
                        return Ok(PostRetirementStep::Retiring);
                    }
                    Ok(Err(detail)) => {
                        self.fail_worktree(RetirementFailureReason::IdentityNotProven, detail);
                        return Ok(PostRetirementStep::Retiring);
                    }
                    Err(error) => {
                        tracing::error!(%error, "native original Worktree preparation worker lost");
                        self.stage = Stage::FatalAuthority;
                        return Ok(PostRetirementStep::FatalAuthority);
                    }
                };
                let original_effect = OriginalWorktreeEffectContext::new(
                    db.clone(),
                    command.clone(),
                    receipt.clone(),
                    live_source.clone(),
                    resource.scope.clone(),
                    resource.snapshot.clone(),
                    resource.resource.clone(),
                    source_entry.clone(),
                    deadline,
                    std::sync::Arc::clone(&backend.fatal_local_authority_fence),
                );
                let continuation = match prepared.into_continuation(original_effect) {
                    Ok(continuation) => continuation,
                    Err(detail) => {
                        self.fail_worktree(RetirementFailureReason::IdentityNotProven, detail);
                        return Ok(PostRetirementStep::Retiring);
                    }
                };
                let probe = backend.native_initial_close_writer_probe(
                    receipt,
                    resource.scope.clone(),
                    resource.snapshot.clone(),
                    resource.resource.clone(),
                );
                // Install and retain observation synchronously before the next
                // actor await. Cancellation cannot discard or restart the worker.
                self.physical =
                    Some(continuation.spawn_physical(tokio::runtime::Handle::current(), probe));
                self.stage = Stage::WorktreePhysical;
                return Ok(PostRetirementStep::Retiring);
            }
            let observation = self
                .physical
                .as_ref()
                .expect("physical stage retains observer");
            if !observation.is_ready() {
                // Observation must not park the actor behind retained physical
                // work, including an unclassified worker with fatal custody.
                return Ok(PostRetirementStep::Retiring);
            }
            let observed = observation.observe().await;
            match observed {
                Ok(outcome) => self.accept_physical_outcome(outcome),
                Err(error) => {
                    tracing::error!(%error, "native original Worktree observation unclassified");
                    self.stage = Stage::FatalAuthority;
                    return Ok(PostRetirementStep::FatalAuthority);
                }
            }
            Ok(PostRetirementStep::Retiring)
        }
        #[cfg(not(unix))]
        {
            let _ = (backend, source_entry, original_deadline);
            self.fail_worktree(
                RetirementFailureReason::ManualRepairRequired,
                "native physical Worktree retirement is unavailable on this platform".into(),
            );
            Ok(PostRetirementStep::Retiring)
        }
    }

    async fn persist_worktree_failure(
        &mut self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        db: &Database,
        live_source: &ConvState,
        resource: &CloseExpectedRetirementResource,
    ) -> DbResult<PostRetirementStep> {
        if self.pending_failure.is_none() {
            let mut remaining = db
                .expected_close_cleanup_failure_resources(
                    resource.attempt_id.as_str(),
                    &resource.scope,
                    &resource.resource,
                )
                .await?;
            for item in &mut remaining {
                if item.disposition != CloseCleanupResourceDisposition::Failed {
                    item.disposition = CloseCleanupResourceDisposition::Unattempted;
                }
            }
            let (reason, detail, occurred_at_us) =
                self.failure.as_ref().expect("retained typed failure");
            self.pending_failure = Some(TerminalizeInitialCloseCleanupFailureRequest {
                failure_occurrence_id: CloseRunRef::initial(resource.attempt_id.clone())
                    .failure_occurrence_id(),
                attempt_id: resource.attempt_id.clone(),
                source_product_conversation_id: receipt.admitted.target.clone(),
                authority: CloseCleanupFailureAuthority::ExpectedResource {
                    scope: resource.scope.clone(),
                    snapshot: resource.snapshot.clone(),
                    resource: resource.resource.clone(),
                },
                remaining_resources: remaining,
                reason: *reason,
                detail: detail.clone(),
                // Post-retirement is reached only after source settlement and
                // all captured process resources have durable success proofs.
                stop_certainty: CloseStopCertainty::ConversationAndProcessesStopped {
                    confirmed_at_us: *occurred_at_us,
                },
                occurred_at_us: *occurred_at_us,
            });
        }
        let request = self
            .pending_failure
            .as_ref()
            .expect("retained failure request")
            .clone();
        match run_authoritative_command(&mut self.stage, || {
            db.terminalize_native_close_initial_resource_failure(
                command,
                receipt,
                live_source,
                request,
            )
        })
        .await
        {
            AuthoritativeCommand::Established(Ok(obligation)) => {
                self.stage = Stage::Completed(obligation.clone());
                Ok(PostRetirementStep::Completed(obligation))
            }
            AuthoritativeCommand::Established(Err(error)) => Err(error),
            AuthoritativeCommand::FatalAuthority => Ok(PostRetirementStep::FatalAuthority),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::close::{
        CloseAttemptId, GitPathIdentity, LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity,
        WorktreeFingerprint, WorktreeId, WorktreeIdentity,
    };

    fn resource(kind: RetiredResourceKind, identity: &str) -> CloseExpectedRetirementResource {
        let resource = if kind == RetiredResourceKind::Worktree {
            RetiredResourceIdentity::parse(
                kind,
                LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                    WorktreeId::parse(identity).unwrap(),
                    WorktreeFingerprint::parse("fingerprint").unwrap(),
                    GitPathIdentity::from_bytes(b"worktree".to_vec()),
                )),
            )
            .unwrap()
        } else {
            RetiredResourceIdentity::parse(
                kind,
                LossItemIdentity::Opaque(OpaqueIdentity::parse(identity).unwrap()),
            )
            .unwrap()
        };
        CloseExpectedRetirementResource {
            attempt_id: CloseAttemptId::parse("attempt").unwrap(),
            scope: WorkScopeId::parse("scope").unwrap(),
            snapshot: CloseRetirementSnapshot::parse("original", "fingerprint").unwrap(),
            resource,
        }
    }

    #[tokio::test]
    async fn unclassified_command_is_latched_and_later_operation_is_not_invoked() {
        use phoenix_db::workflow::LocalAuthorityResult;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut stage = Stage::Dispatch;
        let calls = AtomicUsize::new(0);
        let first = run_authoritative_command(&mut stage, || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(LocalAuthorityResult::<()>::DurableFactUnclassified)
        })
        .await;
        assert!(matches!(first, AuthoritativeCommand::FatalAuthority));
        assert!(matches!(stage, Stage::FatalAuthority));
        let second = run_authoritative_command(&mut stage, || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(LocalAuthorityResult::DurableFactEstablished(()))
        })
        .await;
        assert!(matches!(second, AuthoritativeCommand::FatalAuthority));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // This proves classifier/dispatch behavior only, not real SQLite uncertainty.
    }

    #[test]
    fn captured_worktree_keeps_full_inventory_and_is_not_treated_as_absent() {
        let inventory = vec![
            resource(RetiredResourceKind::WorkScope, "scope"),
            resource(RetiredResourceKind::Worktree, "worktree-id"),
        ];
        let scope = WorkScopeId::parse("scope").unwrap();
        let snapshot = inventory[0].snapshot.clone();
        let cursor = PostRetirement::new(inventory.clone(), vec![scope], snapshot).unwrap();
        assert_eq!(cursor.original_inventory, inventory);
        assert!(cursor
            .original_inventory
            .iter()
            .any(|item| { item.resource.kind() == RetiredResourceKind::Worktree }));
    }

    #[test]
    fn original_worktree_targets_follow_scope_order_and_preserve_full_identity() {
        let first = resource(RetiredResourceKind::Worktree, "first");
        let mut second = resource(RetiredResourceKind::Worktree, "second");
        second.scope = WorkScopeId::parse("other").unwrap();
        let targets = select_original_worktrees(
            &[second.clone(), first.clone()],
            &[first.scope.clone(), second.scope.clone()],
        )
        .unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].resource, first);
        assert_eq!(targets[1].resource, second);
        assert!(matches!(targets[0].resource.resource.identity(),
            LossItemIdentity::Worktree(identity) if identity == &targets[0].identity));
    }

    #[test]
    fn rejects_duplicate_or_foreign_original_worktree_targets() {
        let item = resource(RetiredResourceKind::Worktree, "first");
        let scope = item.scope.clone();
        assert!(
            select_original_worktrees(
                &[item.clone(), item.clone()],
                std::slice::from_ref(&scope),
            )
            .is_err()
        );
        assert!(select_original_worktrees(&[item], &[]).is_err());
    }

    #[tokio::test]
    async fn canceled_authoritative_await_cannot_admit_the_next_stage() {
        let mut stage = Stage::WorktreeDispatch;
        tokio::select! {
            biased;
            _ = run_authoritative_command(&mut stage, || {
                std::future::pending::<LocalAuthorityResult<()>>()
            }) => panic!("unclassified writer cannot return"),
            () = std::future::ready(()) => {}
        }
        assert!(matches!(stage, Stage::FatalAuthority));
        let later = run_authoritative_command(&mut stage, || {
            panic!("canceled writer must prevent later effects");
            #[allow(unreachable_code)]
            std::future::ready(LocalAuthorityResult::DurableFactEstablished(()))
        })
        .await;
        assert!(matches!(later, AuthoritativeCommand::FatalAuthority));
    }

    #[cfg(unix)]
    fn two_scope_cursor() -> (PostRetirement, Vec<CloseExpectedRetirementResource>) {
        let first_scope = resource(RetiredResourceKind::WorkScope, "scope");
        let first_tree = resource(RetiredResourceKind::Worktree, "first");
        let mut second_scope = resource(RetiredResourceKind::WorkScope, "later");
        second_scope.scope = WorkScopeId::parse("later").unwrap();
        let mut second_tree = resource(RetiredResourceKind::Worktree, "second");
        second_tree.scope = second_scope.scope.clone();
        // Deliberately scramble inventory order: execution must use captured
        // scope order, without reducing or reconstructing resource identities.
        let inventory = vec![
            second_tree,
            first_scope.clone(),
            first_tree,
            second_scope.clone(),
        ];
        let cursor = PostRetirement::new(
            inventory.clone(),
            vec![first_scope.scope, second_scope.scope],
            first_scope.snapshot,
        )
        .unwrap();
        (cursor, inventory)
    }

    #[cfg(unix)]
    #[test]
    fn all_original_worktrees_precede_independent_scopes_and_success_requires_evidence() {
        let (mut cursor, inventory) = two_scope_cursor();
        assert!(matches!(cursor.stage, Stage::WorktreeDispatch));
        assert_eq!(cursor.current(), Some(&inventory[2]));
        cursor.stage = Stage::WorktreePhysical;
        cursor.accept_physical_outcome(WorktreePhysicalOutcome::Retired);
        assert!(matches!(cursor.stage, Stage::WorktreeEvidence));
        assert_eq!(
            cursor.current(),
            Some(&inventory[2]),
            "observation alone cannot advance"
        );
        assert_eq!(cursor.scope_index, 0);
        // Simulate only the confirmed evidence writer's success branch.
        cursor.advance_worktree();
        assert!(matches!(cursor.stage, Stage::WorktreeDispatch));
        assert_eq!(cursor.current(), Some(&inventory[0]));
        assert_eq!(
            cursor.scope_index, 0,
            "no WorkScope before the later Worktree"
        );
        cursor.accept_physical_outcome(WorktreePhysicalOutcome::Retired);
        cursor.advance_worktree();
        assert!(matches!(cursor.stage, Stage::Dispatch));
        assert_eq!(cursor.current(), Some(&inventory[1]));
        assert_eq!(cursor.original_inventory, inventory);
    }

    #[cfg(unix)]
    #[test]
    fn every_physical_failure_retains_failed_identity_and_stops_before_later_scopes() {
        let failures = [
            (
                WorktreePhysicalOutcome::StopFailed {
                    detail: "stop refused".into(),
                },
                RetirementFailureReason::RemovalFailed,
            ),
            (
                WorktreePhysicalOutcome::ReinspectionRequired {
                    detail: "drift".into(),
                },
                RetirementFailureReason::IdentityNotProven,
            ),
            (
                WorktreePhysicalOutcome::Residual {
                    detail: "retained".into(),
                },
                RetirementFailureReason::ManualRepairRequired,
            ),
        ];
        for (outcome, reason) in failures {
            let (mut cursor, inventory) = two_scope_cursor();
            cursor.stage = Stage::WorktreePhysical;
            cursor.accept_physical_outcome(outcome);
            assert!(matches!(cursor.stage, Stage::WorktreeFailure));
            assert_eq!(cursor.failure.as_ref().unwrap().0, reason);
            assert_eq!(cursor.current(), Some(&inventory[2]));
            assert_eq!((cursor.worktree_index, cursor.scope_index), (0, 0));
            assert!(
                cursor.pending_failure.is_none(),
                "failure is retained before any writer await"
            );
        }
    }

    #[test]
    fn rejects_scope_order_that_drops_a_captured_scope_resource() {
        let item = resource(RetiredResourceKind::WorkScope, "scope");
        assert!(PostRetirement::new(vec![item.clone()], vec![], item.snapshot).is_err());
    }
}
