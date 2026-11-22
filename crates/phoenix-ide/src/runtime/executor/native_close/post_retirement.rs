//! Source-owned post-process continuation over the captured original inventory.

use phoenix_core::domain::close::{
    CloseExpectedRetirementResource, CloseObligation, CloseRetirementSnapshot, LossItemIdentity,
    RetiredResourceKind, WorktreeIdentity,
};
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId, WorkScopeRetirementOutcome};
use phoenix_db::native_close::{NativeCloseCommand, NativeCloseReceipt};
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{Database, DbError, DbResult, RecordCloseRetirementDispatchRequest};

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
    match operation().await {
        LocalAuthorityResult::DurableFactEstablished(value) => {
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
    Dispatch,
    Retiring,
    Finalizing,
    Completed(CloseObligation),
    Blocked,
    FatalAuthority,
}

// A captured Worktree remains an exact typed target; it is never inferred absent
// from the lack of physical evidence. This is preparation, not physical authority.
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
        let scoped = inventory.iter().filter(|item| {
            item.scope == *scope && item.resource.kind() == RetiredResourceKind::Worktree
        });
        for item in scoped {
            let LossItemIdentity::Worktree(identity) = item.resource.identity() else {
                return Err("captured Worktree has no typed worktree identity".into());
            };
            if targets.iter().any(|prior: &OriginalWorktreeTarget| prior.resource.scope == *scope) {
                return Err("captured scope has multiple Worktree resources".into());
            }
            targets.push(OriginalWorktreeTarget {
                resource: item.clone(),
                identity: identity.clone(),
            });
        }
    }
    if inventory.iter().any(|item| {
        item.resource.kind() == RetiredResourceKind::Worktree
            && !scopes.contains(&item.scope)
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
        Ok(Self {
            original_inventory,
            worktree_targets,
            scope_order,
            snapshot,
            scope_index: 0,
            stage: Stage::Dispatch,
        })
    }

    fn current(&self) -> Option<&CloseExpectedRetirementResource> {
        let scope = self.scope_order.get(self.scope_index)?;
        self.original_inventory.iter().find(|resource| {
            resource.scope == *scope && resource.resource.kind() == RetiredResourceKind::WorkScope
        })
    }

    pub(super) async fn step(
        &mut self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        db: &Database,
        backend: &crate::runtime::RuntimeManager,
        live_source: &ConvState,
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
        // No physical sink is installed yet. The target is selected from the
        // complete captured inventory, but no WorkScope may retire past it.
        if let Some(target) = self.worktree_targets.first() {
            self.stage = Stage::Blocked;
            tracing::warn!(
                attempt_id = %receipt.admitted.attempt_id,
                scope = %target.resource.scope,
                worktree_id = %target.identity.id().as_str(),
                "native initial Close blocked: captured Worktree resources require the native adapter"
            );
            return Ok(PostRetirementStep::Blocked);
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
        assert!(select_original_worktrees(&[item.clone(), item.clone()], &[scope.clone()]).is_err());
        assert!(select_original_worktrees(&[item], &[]).is_err());
    }

    #[test]
    fn rejects_scope_order_that_drops_a_captured_scope_resource() {
        let item = resource(RetiredResourceKind::WorkScope, "scope");
        assert!(PostRetirement::new(vec![item.clone()], vec![], item.snapshot).is_err());
    }
}
