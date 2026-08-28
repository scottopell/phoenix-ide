//! Exact-instance resource fences and receipts for Close retirement.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use phoenix_core::domain::close::{
    AbsenceBasis, CapturedWorktreeIdentity, CloseAttemptId, CloseExpectedRetirementResource,
    CloseLossItem, CloseOwnedResourceInventory, ClosePhase, CloseRetirementSnapshot,
    GitOidIdentity, GitPathIdentity, LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity,
    RetiredResourceKind, RetirementFailureReason, RetirementOutcome, WorktreeIdentity,
};
use phoenix_core::work_scope::{
    ResourceScopeKey, WorkScopeId, WorkScopeRetirementOutcome, WorkScopeRetirementPrecondition,
};
use phoenix_terminal::session::{TerminalRetirementOutcome, TerminalRetirementPermit};
use phoenix_tools::{
    bash::registry::{BashRetirementOutcome, BashRetirementPermit},
    browser::session::{BrowserRetirementOutcome, BrowserRetirementPermit},
    tmux::registry::{
        PersistentTmuxDiscovery, TmuxRetirementOutcome, TmuxRetirementPermit,
        TmuxRetirementRehydration, TmuxServerInstanceIdentity,
    },
};

use super::creation_worker::RepositoryMutationLock;
use super::RuntimeManager;
use crate::db::{
    CaptureCloseRetirementInventoryRequest, CaptureCloseRetirementInventoryScopeRequest,
    RecordCloseRetirementDispatchRequest, RecordCloseRetirementEvidenceRequest,
    ReplaceCloseInspectionRequest, ReplaceCloseInspectionScopeRequest,
};

/// Process-local capability retained from inventory sealing through per-resource
/// teardown. Durable inventory stores only the permit's stable instance identity.
pub(crate) struct CloseResourceLease {
    bash: BashRetirementPermit,
    tmux: TmuxRetirementPermit,
    terminal: TerminalRetirementPermit,
    browser: BrowserRetirementPermit,
    resources: Vec<RetiredResourceIdentity>,
}

#[derive(Debug, PartialEq, Eq)]
enum CloseLeaseFailure {
    ProcessEpoch {
        kind: RetiredResourceKind,
        reason: String,
    },
    Tmux {
        reason: RetirementFailureReason,
        detail: String,
    },
}

impl std::fmt::Display for CloseLeaseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProcessEpoch { kind, reason } => {
                write!(
                    formatter,
                    "{} process-epoch teardown failed: {reason}",
                    kind.as_str()
                )
            }
            Self::Tmux { detail, .. } => write!(formatter, "tmux teardown failed: {detail}"),
        }
    }
}

impl RuntimeManager {
    /// Inspects exact server-owned captured worktrees and persists normalized loss evidence.
    pub(crate) async fn inspect_close_retirement(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<CloseRetirementSnapshot, String> {
        self.inspect_close_retirement_with_continuation(attempt_id, true)
            .await
    }

    pub(crate) async fn inspect_close_retirement_only(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<CloseRetirementSnapshot, String> {
        self.inspect_close_retirement_with_continuation(attempt_id, false)
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn inspect_close_retirement_with_continuation(
        &self,
        attempt_id: CloseAttemptId,
        continue_clean_retirement: bool,
    ) -> Result<CloseRetirementSnapshot, String> {
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let mut requests = Vec::with_capacity(scopes.len());
        for scope in scopes {
            let (snapshot, losses) = match scope.captured_worktree {
                None => continue,
                Some(CapturedWorktreeIdentity::Resolved(identity)) => {
                    let path = worktree_path(&identity);
                    match path.try_exists() {
                        Ok(true) => inspect_worktree(&identity).await?,
                        Ok(false) => {
                            let obligation = self
                                .db()
                                .get_close_obligation(attempt_id.as_str())
                                .await
                                .map_err(|error| error.to_string())?;
                            if let Some(prior_snapshot) = obligation.snapshot().cloned() {
                                let worktree = RetiredResourceIdentity::parse(
                                    RetiredResourceKind::Worktree,
                                    LossItemIdentity::Worktree(identity.clone()),
                                )
                                .map_err(|error| error.to_string())?;
                                if self
                                    .db()
                                    .close_retirement_resource_was_dispatched(
                                        &attempt_id,
                                        &scope.scope,
                                        &prior_snapshot,
                                        &worktree,
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?
                                {
                                    if continue_clean_retirement
                                        && obligation.phase() == ClosePhase::RetirementRequested
                                    {
                                        self.retire_close_runtime_resources(attempt_id).await?;
                                    }
                                    return Ok(prior_snapshot);
                                }
                            }
                            self.db()
                                .route_close_attempt_to_repair(&attempt_id)
                                .await
                                .map_err(|error| error.to_string())?;
                            return Err(format!(
                                "scope {} captured worktree is absent before inspection",
                                scope.scope
                            ));
                        }
                        Err(error) => {
                            self.db()
                                .route_close_attempt_to_repair(&attempt_id)
                                .await
                                .map_err(|db_error| db_error.to_string())?;
                            return Err(format!(
                                "scope {} captured worktree is inaccessible before inspection: {error}",
                                scope.scope
                            ));
                        }
                    }
                }
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                    self.db()
                        .route_close_attempt_to_repair(&attempt_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Err(format!(
                        "scope {} has unresolved captured worktree identity",
                        scope.scope
                    ));
                }
            };
            requests.push(ReplaceCloseInspectionScopeRequest {
                scope: scope.scope,
                snapshot,
                losses,
            });
        }
        self.db()
            .replace_close_inspection(ReplaceCloseInspectionRequest {
                attempt_id: attempt_id.clone(),
                scopes: requests,
            })
            .await
            .map_err(|error| error.to_string())?;
        let obligation = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let snapshot = obligation
            .snapshot()
            .cloned()
            .ok_or_else(|| "server inspection did not persist aggregate snapshot".to_string())?;
        if continue_clean_retirement && obligation.phase() == ClosePhase::RetirementRequested {
            self.retire_close_runtime_resources(attempt_id).await?;
        }
        Ok(snapshot)
    }

    /// Acquires every registry admission fence before Close seals inventory.
    ///
    /// The map lock spans check, fencing, and insertion so concurrent callers for
    /// one exact `(attempt, scope)` cannot mint competing generations.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn acquire_close_resource_lease(
        &self,
        attempt_id: &CloseAttemptId,
        scope: WorkScopeId,
    ) -> Result<Vec<RetiredResourceIdentity>, String> {
        let mut leases = self.close_retirement_leases.lock().await;
        if let Some(existing) = leases
            .get(&(attempt_id.as_str().to_string(), scope.clone()))
            .map(|lease| lease.resources.clone())
        {
            return Ok(existing);
        }
        let captured = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|captured| captured.scope == scope)
            .ok_or_else(|| format!("Close attempt {attempt_id} did not capture scope {scope}"))?;
        let legacy_worktree_path = match &captured.captured_worktree {
            Some(CapturedWorktreeIdentity::Resolved(identity)) => {
                Some(path_buf_from_git_bytes(identity.locator().as_bytes()))
            }
            Some(CapturedWorktreeIdentity::Unresolved { .. }) | None => None,
        };
        let key = ResourceScopeKey::Work(scope.clone());
        let bash = self.bash_handles().begin_retirement(&key).await;
        let tmux_discovery = match self
            .tmux_registry()
            .discover_persistent_identity(&key, legacy_worktree_path.as_deref(), None)
            .await
        {
            Ok(discovery) => discovery,
            Err(error) => {
                self.bash_handles().cancel_retirement(bash).await;
                return Err(format!("tmux identity discovery failed: {error}"));
            }
        };
        let tmux = match tmux_discovery {
            PersistentTmuxDiscovery::Absent => {
                self.tmux_registry()
                    .begin_retirement(&key, legacy_worktree_path.as_deref(), None)
                    .await
            }
            PersistentTmuxDiscovery::Exact(identity) => {
                match self
                    .tmux_registry()
                    .rehydrate_retirement(&key, &identity)
                    .await
                {
                    Ok(TmuxRetirementRehydration::Permit(permit)) => permit,
                    Ok(TmuxRetirementRehydration::AbsenceVerified) => {
                        self.tmux_registry()
                            .begin_retirement(&key, legacy_worktree_path.as_deref(), None)
                            .await
                    }
                    Ok(TmuxRetirementRehydration::Residual { reason }) => {
                        self.bash_handles().cancel_retirement(bash).await;
                        return Err(format!("tmux identity is ambiguous: {reason}"));
                    }
                    Err(error) => {
                        self.bash_handles().cancel_retirement(bash).await;
                        return Err(format!("tmux rehydration failed: {error}"));
                    }
                }
            }
            PersistentTmuxDiscovery::Ambiguous { reason } => {
                self.bash_handles().cancel_retirement(bash).await;
                return Err(format!("tmux identity is ambiguous: {reason}"));
            }
        };
        let terminal = self.terminals.begin_retirement(&key);
        let browser = self.browser_sessions().begin_retirement(&key).await;
        let mut resources = Vec::new();
        for target in &bash.exact_process_groups {
            resources.push(opaque_resource(
                RetiredResourceKind::BashProcessGroup,
                target.stable_resource_identity(),
            ));
        }
        if tmux.had_entry() {
            resources.push(opaque_resource(
                RetiredResourceKind::TmuxServer,
                tmux.instance.stable_identity(),
            ));
        }
        if let Some(instance) = &terminal.instance {
            resources.push(opaque_resource(
                RetiredResourceKind::PtySession,
                instance.stable_identity(),
            ));
        }
        for instance in &browser.instances {
            resources.push(opaque_resource(
                RetiredResourceKind::BrowserSession,
                instance.stable_identity(),
            ));
        }
        leases.insert(
            (attempt_id.as_str().to_string(), scope),
            CloseResourceLease {
                bash,
                tmux,
                terminal,
                browser,
                resources: resources.clone(),
            },
        );
        Ok(resources)
    }

    async fn cancel_close_resource_lease(&self, attempt_id: &CloseAttemptId, scope: &WorkScopeId) {
        let lease = self
            .close_retirement_leases
            .lock()
            .await
            .remove(&(attempt_id.as_str().to_string(), scope.clone()));
        let Some(lease) = lease else {
            return;
        };
        let key = ResourceScopeKey::Work(scope.clone());
        self.bash_handles().cancel_retirement(lease.bash).await;
        self.tmux_registry().reopen_after_repair(&key).await;
        self.terminals.reopen_after_repair(&key);
        self.browser_sessions().reopen_after_repair(&key).await;
    }

    pub(crate) async fn cancel_close_resource_leases(&self, attempt_id: &CloseAttemptId) {
        let scopes = self
            .close_retirement_leases
            .lock()
            .await
            .keys()
            .filter(|(candidate, _)| candidate == attempt_id.as_str())
            .map(|(_, scope)| scope.clone())
            .collect::<Vec<_>>();
        for scope in scopes {
            self.cancel_close_resource_lease(attempt_id, &scope).await;
        }
    }

    pub(crate) async fn cancel_close_before_retirement(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<(), String> {
        let _execution = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
        self.db()
            .cancel_close_before_retirement(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        self.cancel_close_resource_leases(attempt_id).await;
        Ok(())
    }

    /// Acquires all scope fences, then seals the exact server-owned inventory.
    /// The fence stays held in `close_retirement_leases` through completion or repair.
    pub(crate) async fn capture_close_retirement_inventory(
        &self,
        attempt_id: CloseAttemptId,
        snapshot: CloseRetirementSnapshot,
    ) -> Result<(), String> {
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let mut requests = Vec::with_capacity(scopes.len());
        let mut acquired_scopes = Vec::with_capacity(scopes.len());
        for captured in scopes {
            let resources = match self
                .acquire_close_resource_lease(&attempt_id, captured.scope.clone())
                .await
            {
                Ok(resources) => resources,
                Err(reason) => {
                    for scope in &acquired_scopes {
                        self.cancel_close_resource_lease(&attempt_id, scope).await;
                    }
                    self.db()
                        .route_close_attempt_to_repair(&attempt_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Err(reason);
                }
            };
            acquired_scopes.push(captured.scope.clone());
            let worktree = match captured.captured_worktree {
                None => None,
                Some(CapturedWorktreeIdentity::Resolved(identity)) => Some(identity),
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                    for scope in &acquired_scopes {
                        self.cancel_close_resource_lease(&attempt_id, scope).await;
                    }
                    self.db()
                        .route_close_attempt_to_repair(&attempt_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Err(format!(
                        "scope {} has unresolved captured worktree identity",
                        captured.scope
                    ));
                }
            };
            let mut inventory = CloseOwnedResourceInventory {
                worktree,
                work_scopes: BTreeSet::default(),
                bash_process_groups: BTreeSet::default(),
                tmux_servers: BTreeSet::default(),
                pty_sessions: BTreeSet::default(),
                browser_sessions: BTreeSet::default(),
                equivalent_live_resources: BTreeSet::default(),
            };
            for resource in resources {
                if resource.kind() != RetiredResourceKind::TmuxServer {
                    continue;
                }
                let LossItemIdentity::Opaque(identity) = resource.identity() else {
                    return Err("registry resource identity was not opaque".to_string());
                };
                match resource.kind() {
                    RetiredResourceKind::TmuxServer => {
                        inventory.tmux_servers.insert(identity.clone());
                    }
                    kind => {
                        return Err(format!("unexpected durable permit resource kind {kind:?}"))
                    }
                }
            }
            requests.push(CaptureCloseRetirementInventoryScopeRequest {
                scope: captured.scope,
                inventory,
            });
        }
        match self
            .db()
            .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
                attempt_id: attempt_id.clone(),
                snapshot,
                scopes: requests,
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(error) => {
                for scope in &acquired_scopes {
                    self.cancel_close_resource_lease(&attempt_id, scope).await;
                }
                Err(error.to_string())
            }
        }
    }

    /// Retires exactly the unresolved inventory targets. The coordinator never
    /// advances Close to `Completed`; it only creates per-resource evidence.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn retire_close_runtime_resources(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<(), String> {
        let _execution = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
        let obligation = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        if !matches!(
            obligation.phase(),
            ClosePhase::RetirementRequested | ClosePhase::NeedsRepair
        ) {
            return Ok(());
        }
        let snapshot = obligation
            .snapshot()
            .cloned()
            .ok_or_else(|| "retirement requested without an inspection snapshot".to_string())?;
        let mut targets = self
            .db()
            .list_close_expected_retirement_resources(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        if targets.is_empty() {
            self.capture_close_retirement_inventory(attempt_id.clone(), snapshot.clone())
                .await?;
            targets = self
                .db()
                .list_close_expected_retirement_resources(attempt_id.as_str())
                .await
                .map_err(|error| error.to_string())?;
        }
        let evidence = self
            .db()
            .list_close_retirement_evidence(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let retired = evidence
            .into_iter()
            .filter(|evidence| {
                matches!(
                    evidence.outcome,
                    RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. }
                )
            })
            .map(|evidence| (evidence.scope, resource_key(&evidence.resource)))
            .collect::<std::collections::BTreeSet<_>>();
        self.validate_close_worktrees_before_runtime_retirement(
            &attempt_id,
            &snapshot,
            &targets,
            &retired,
        )
        .await?;
        let runtime_targets = targets
            .into_iter()
            .filter(|target| is_runtime_resource(target.resource.kind()))
            .filter(|target| {
                !retired.contains(&(target.scope.clone(), resource_key(&target.resource)))
            })
            .collect::<Vec<_>>();
        let mut scopes = runtime_targets
            .iter()
            .map(|target| target.scope.clone())
            .collect::<std::collections::BTreeSet<_>>();
        scopes.extend(
            self.close_retirement_leases
                .lock()
                .await
                .keys()
                .filter(|(lease_attempt, _)| lease_attempt == attempt_id.as_str())
                .map(|(_, scope)| scope.clone()),
        );
        for scope in scopes {
            let expected = runtime_targets
                .iter()
                .filter(|target| target.scope == scope)
                .map(|target| target.resource.clone())
                .collect::<Vec<_>>();
            let has_lease = self
                .close_retirement_leases
                .lock()
                .await
                .contains_key(&(attempt_id.as_str().to_string(), scope.clone()));
            if !has_lease {
                for resource in &expected {
                    let LossItemIdentity::Opaque(identity) = resource.identity() else {
                        return self
                            .record_close_residual(
                                &attempt_id,
                                &snapshot,
                                &scope,
                                resource.clone(),
                                RetirementFailureReason::IdentityNotProven,
                                "sealed durable tmux identity was not opaque",
                            )
                            .await;
                    };
                    let Some(instance) =
                        TmuxServerInstanceIdentity::parse_stable_identity(identity.as_str())
                    else {
                        return self
                            .record_close_residual(
                                &attempt_id,
                                &snapshot,
                                &scope,
                                resource.clone(),
                                RetirementFailureReason::IdentityNotProven,
                                "sealed durable tmux identity was malformed",
                            )
                            .await;
                    };
                    self.db()
                        .record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
                            attempt_id: attempt_id.clone(),
                            scope: scope.clone(),
                            snapshot: snapshot.clone(),
                            resource: resource.clone(),
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                    match self
                        .tmux_registry()
                        .rehydrate_retirement(&ResourceScopeKey::Work(scope.clone()), &instance)
                        .await
                    {
                        Ok(TmuxRetirementRehydration::Permit(permit)) => {
                            let outcome = self
                                .tmux_registry()
                                .complete_retirement(&permit)
                                .await
                                .map_err(|error| error.to_string())?;
                            if let Err((reason, detail)) = require_tmux_absent(outcome) {
                                return self
                                    .record_close_residual(
                                        &attempt_id,
                                        &snapshot,
                                        &scope,
                                        resource.clone(),
                                        reason,
                                        &detail,
                                    )
                                    .await;
                            }
                            self.record_close_retired(
                                &attempt_id,
                                &snapshot,
                                &scope,
                                resource.clone(),
                                "exact durable tmux rehydration",
                            )
                            .await?;
                        }
                        Ok(TmuxRetirementRehydration::AbsenceVerified) => {
                            self.record_close_retired(
                                &attempt_id,
                                &snapshot,
                                &scope,
                                resource.clone(),
                                "exact durable tmux absence",
                            )
                            .await?;
                        }
                        Ok(TmuxRetirementRehydration::Residual { reason }) => {
                            return self
                                .record_close_residual(
                                    &attempt_id,
                                    &snapshot,
                                    &scope,
                                    resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    &reason,
                                )
                                .await;
                        }
                        Err(error) => {
                            return self
                                .record_close_residual(
                                    &attempt_id,
                                    &snapshot,
                                    &scope,
                                    resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    &format!("durable tmux rehydration failed: {error}"),
                                )
                                .await;
                        }
                    }
                }
                continue;
            }
            for resource in &expected {
                self.db()
                    .record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
                        attempt_id: attempt_id.clone(),
                        scope: scope.clone(),
                        snapshot: snapshot.clone(),
                        resource: resource.clone(),
                    })
                    .await
                    .map_err(|error| error.to_string())?;
            }
            let resources = match self
                .complete_close_resource_lease(&attempt_id, &scope)
                .await
            {
                Ok(resources) => resources,
                Err(CloseLeaseFailure::ProcessEpoch { kind, reason }) => {
                    let scope_resource =
                        opaque_resource(RetiredResourceKind::WorkScope, scope.as_str().to_string());
                    return self
                        .record_close_residual(
                            &attempt_id,
                            &snapshot,
                            &scope,
                            scope_resource,
                            RetirementFailureReason::IdentityNotProven,
                            &format!(
                                "{} process-epoch Close teardown requires repair: {reason}",
                                kind.as_str()
                            ),
                        )
                        .await;
                }
                Err(CloseLeaseFailure::Tmux { reason, detail }) => {
                    let Some(resource) = expected
                        .iter()
                        .find(|resource| resource.kind() == RetiredResourceKind::TmuxServer)
                        .cloned()
                    else {
                        self.db()
                            .route_close_attempt_to_repair(&attempt_id)
                            .await
                            .map_err(|error| error.to_string())?;
                        return Err(format!(
                            "tmux Close teardown failed without a sealed tmux target for scope {scope}: {detail}"
                        ));
                    };
                    return self
                        .record_close_residual(
                            &attempt_id,
                            &snapshot,
                            &scope,
                            resource,
                            reason,
                            &detail,
                        )
                        .await;
                }
            };
            if expected.is_empty() {
                continue;
            }
            let expected_keys = expected
                .iter()
                .map(resource_key)
                .collect::<std::collections::BTreeSet<_>>();
            let resources = resources
                .into_iter()
                .filter(|resource| expected_keys.contains(&resource_key(resource)))
                .collect::<Vec<_>>();
            let retired_keys = resources
                .iter()
                .map(resource_key)
                .collect::<std::collections::BTreeSet<_>>();
            if expected_keys != retired_keys {
                return self
                    .record_close_residual(
                        &attempt_id,
                        &snapshot,
                        &scope,
                        expected[0].clone(),
                        RetirementFailureReason::IdentityNotProven,
                        "live Close lease differs from sealed unresolved inventory",
                    )
                    .await;
            }
            for resource in resources {
                self.record_close_retired(
                    &attempt_id,
                    &snapshot,
                    &scope,
                    resource,
                    "exact registry permit retirement",
                )
                .await?;
            }
        }
        self.retire_close_worktrees_and_scopes(&attempt_id, &snapshot)
            .await?;
        self.db()
            .complete_close_retirement(&attempt_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn validate_close_worktrees_before_runtime_retirement(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
        targets: &[CloseExpectedRetirementResource],
        retired: &std::collections::BTreeSet<(WorkScopeId, (String, String))>,
    ) -> Result<(), String> {
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let inspections = self
            .db()
            .list_close_retirement_inspections(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        for captured in scopes {
            let Some(target) = targets.iter().find(|target| {
                target.scope == captured.scope
                    && target.resource.kind() == RetiredResourceKind::Worktree
                    && !retired.contains(&(captured.scope.clone(), resource_key(&target.resource)))
            }) else {
                continue;
            };
            let Some(CapturedWorktreeIdentity::Resolved(identity)) = &captured.captured_worktree
            else {
                return self
                    .record_close_residual(
                        attempt_id,
                        snapshot,
                        &captured.scope,
                        target.resource.clone(),
                        RetirementFailureReason::IdentityNotProven,
                        "worktree cannot be validated before live resource retirement",
                    )
                    .await;
            };
            match worktree_path(identity).try_exists() {
                Ok(false) => continue,
                Ok(true) => {}
                Err(error) => {
                    return self
                        .record_close_residual(
                            attempt_id,
                            snapshot,
                            &captured.scope,
                            target.resource.clone(),
                            RetirementFailureReason::IdentityNotProven,
                            &format!(
                                "captured worktree is inaccessible before live resource retirement: {error}"
                            ),
                        )
                        .await;
                }
            }
            let Some(confirmed) = inspections
                .iter()
                .find(|inspection| inspection.target.scope == captured.scope)
            else {
                return self
                    .record_close_residual(
                        attempt_id,
                        snapshot,
                        &captured.scope,
                        target.resource.clone(),
                        RetirementFailureReason::IdentityNotProven,
                        "worktree has no confirmed inspection before live resource retirement",
                    )
                    .await;
            };
            let (fresh_snapshot, _) = inspect_worktree(identity).await.map_err(|reason| {
                format!("worktree cannot be reinspected before live resource retirement: {reason}")
            })?;
            if fresh_snapshot != confirmed.snapshot {
                self.db()
                    .return_close_attempt_to_reinspection(attempt_id)
                    .await
                    .map_err(|error| error.to_string())?;
                Box::pin(self.inspect_close_retirement_only(attempt_id.clone())).await?;
                return Err(
                    "worktree changed after Close inspection confirmation; fresh confirmation is required"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn retire_close_worktrees_and_scopes(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
    ) -> Result<(), String> {
        let targets = self
            .db()
            .list_close_expected_retirement_resources(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let evidence = self
            .db()
            .list_close_retirement_evidence(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let retired = evidence
            .into_iter()
            .filter(|evidence| {
                matches!(
                    evidence.outcome,
                    RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. }
                )
            })
            .map(|evidence| (evidence.scope, resource_key(&evidence.resource)))
            .collect::<std::collections::BTreeSet<_>>();
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        for captured in scopes {
            let scope = captured.scope.clone();
            let worktree_target = targets.iter().find(|target| {
                target.scope == scope && target.resource.kind() == RetiredResourceKind::Worktree
            });
            if self
                .db()
                .work_scope_has_unresolved_product_ownership(&scope)
                .await
                .map_err(|error| error.to_string())?
            {
                let resource = worktree_target
                    .map_or_else(
                        || {
                            RetiredResourceIdentity::parse(
                                RetiredResourceKind::WorkScope,
                                LossItemIdentity::Opaque(
                                    OpaqueIdentity::parse(scope.as_str())
                                        .expect("WorkScopeId is non-empty"),
                                ),
                            )
                        },
                        |target| Ok(target.resource.clone()),
                    )
                    .map_err(|error| error.to_string())?;
                return self
                    .record_close_residual(
                        attempt_id,
                        snapshot,
                        &scope,
                        resource,
                        RetirementFailureReason::IdentityNotProven,
                        "work scope has unresolved ProductConversation ownership",
                    )
                    .await;
            }
            if let Some(target) = worktree_target {
                if !retired.contains(&(scope.clone(), resource_key(&target.resource))) {
                    let identity = match &captured.captured_worktree {
                        Some(CapturedWorktreeIdentity::Resolved(identity)) => identity,
                        Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    "captured worktree identity is unresolved",
                                )
                                .await;
                        }
                        None => {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    "worktree target is not backed by a captured worktree identity",
                                )
                                .await;
                        }
                    };
                    let inspections = self
                        .db()
                        .list_close_retirement_inspections(attempt_id.as_str())
                        .await
                        .map_err(|error| error.to_string())?;
                    let Some(confirmed) = inspections
                        .iter()
                        .find(|inspection| inspection.target.scope == scope)
                    else {
                        return self
                            .record_close_residual(
                                attempt_id,
                                snapshot,
                                &scope,
                                target.resource.clone(),
                                RetirementFailureReason::IdentityNotProven,
                                "worktree removal has no confirmed inspection",
                            )
                            .await;
                    };
                    match worktree_path(identity).try_exists() {
                        Ok(true) => {
                            self.db()
                                .record_close_retirement_dispatch(
                                    RecordCloseRetirementDispatchRequest {
                                        attempt_id: attempt_id.clone(),
                                        scope: scope.clone(),
                                        snapshot: snapshot.clone(),
                                        resource: target.resource.clone(),
                                    },
                                )
                                .await
                                .map_err(|error| error.to_string())?;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    &format!(
                                        "captured worktree is inaccessible before retirement dispatch: {error}"
                                    ),
                                )
                                .await;
                        }
                    }
                    let identity = identity.clone();
                    let confirmed_snapshot = confirmed.snapshot.clone();
                    let runtime = tokio::runtime::Handle::current();
                    let final_removal = tokio::task::spawn_blocking(move || {
                        inspect_and_remove_exact_worktree(&runtime, &identity, &confirmed_snapshot)
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                    let fresh_snapshot: Option<CloseRetirementSnapshot> = match final_removal {
                        Ok(ExactWorktreeRemoval::Missing { reason }) => {
                            let absence_basis = AbsenceBasis::PreexistingExactIdentityEvidence;
                            let adopted = self
                                .db()
                                .record_close_retirement_evidence(
                                    RecordCloseRetirementEvidenceRequest {
                                        attempt_id: attempt_id.clone(),
                                        snapshot: snapshot.clone(),
                                        scope: scope.clone(),
                                        resource: target.resource.clone(),
                                        outcome: RetirementOutcome::AbsenceAdopted {
                                            absence_basis,
                                        },
                                        detail: Some(
                                            "exact prior close evidence adopted missing worktree"
                                                .to_string(),
                                        ),
                                    },
                                )
                                .await;
                            match adopted {
                                Ok(()) => None,
                                Err(_) => {
                                    return self
                                        .record_close_residual(
                                            attempt_id,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            &format!(
                                                "worktree is absent without adoptable exact identity evidence: {reason}"
                                            ),
                                        )
                                        .await;
                                }
                            }
                        }
                        Ok(ExactWorktreeRemoval::ReinspectionRequired { detail }) => {
                            self.db()
                                .return_close_attempt_to_reinspection(attempt_id)
                                .await
                                .map_err(|error| error.to_string())?;
                            Box::pin(self.inspect_close_retirement_only(attempt_id.clone()))
                                .await?;
                            return Err(detail);
                        }
                        Ok(ExactWorktreeRemoval::Residual { detail }) => {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    &detail,
                                )
                                .await;
                        }
                        Err(reason) => {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    &format!(
                                        "worktree cannot be reinspected before removal: {reason}"
                                    ),
                                )
                                .await;
                        }
                    };
                    if let Some(fresh_snapshot) = fresh_snapshot {
                        if fresh_snapshot != confirmed.snapshot {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    "worktree changed after Close inspection confirmation",
                                )
                                .await;
                        }
                        self.record_close_retired(
                            attempt_id,
                            snapshot,
                            &scope,
                            target.resource.clone(),
                            "exact captured Git worktree removal",
                        )
                        .await?;
                    }
                }
            }
            let work_scope_target = targets
                .iter()
                .find(|target| {
                    target.scope == scope
                        && target.resource.kind() == RetiredResourceKind::WorkScope
                })
                .ok_or_else(|| format!("Close scope {scope} lacks mandatory WorkScope target"))?;
            if retired.contains(&(scope.clone(), resource_key(&work_scope_target.resource))) {
                continue;
            }
            match self
                .db()
                .retire_work_scope_for_close_attempt(
                    attempt_id,
                    WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(
                        scope.clone(),
                    ),
                    "close retirement",
                )
                .await
                .map_err(|error| error.to_string())?
            {
                WorkScopeRetirementOutcome::Retired
                | WorkScopeRetirementOutcome::AlreadyRetired => {
                    self.record_close_retired(
                        attempt_id,
                        snapshot,
                        &scope,
                        work_scope_target.resource.clone(),
                        "exact Close WorkScope retirement",
                    )
                    .await?;
                }
                WorkScopeRetirementOutcome::Blocked(blocker) => {
                    return self
                        .record_close_residual(
                            attempt_id,
                            snapshot,
                            &scope,
                            work_scope_target.resource.clone(),
                            RetirementFailureReason::StillSharedByLiveOwner,
                            &format!("Close WorkScope retirement remains blocked: {blocker:?}"),
                        )
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn record_close_retired(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        detail: &str,
    ) -> Result<(), String> {
        self.db()
            .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: attempt_id.clone(),
                snapshot: snapshot.clone(),
                scope: scope.clone(),
                resource,
                outcome: RetirementOutcome::Retired,
                detail: Some(detail.to_string()),
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn record_close_residual<T>(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        reason: RetirementFailureReason,
        detail: &str,
    ) -> Result<T, String> {
        self.db()
            .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: attempt_id.clone(),
                snapshot: snapshot.clone(),
                scope: scope.clone(),
                resource,
                outcome: RetirementOutcome::Residual {
                    residual_reason: reason,
                },
                detail: Some(detail.to_string()),
            })
            .await
            .map_err(|error| error.to_string())?;
        Err(detail.to_string())
    }

    /// Completes one live lease. Callers must persist exactly one receipt per
    /// returned identity; any residual outcome keeps the registry fence closed.
    async fn complete_close_resource_lease(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
    ) -> Result<Vec<RetiredResourceIdentity>, CloseLeaseFailure> {
        let key = (attempt_id.as_str().to_string(), scope.clone());
        let lease = self.close_retirement_leases.lock().await.remove(&key);
        let Some(lease) = lease else {
            return Err(CloseLeaseFailure::ProcessEpoch {
                kind: RetiredResourceKind::EquivalentLiveResource,
                reason: "Close resource lease is unavailable after restart; process-epoch identities cannot be rehydrated".to_string(),
            });
        };
        let result = async {
            require_absent(self.bash_handles().complete_retirement(&lease.bash).await).map_err(
                |reason| CloseLeaseFailure::ProcessEpoch {
                    kind: RetiredResourceKind::BashProcessGroup,
                    reason,
                },
            )?;
            let tmux_outcome = self
                .tmux_registry()
                .complete_retirement(&lease.tmux)
                .await
                .map_err(|error| CloseLeaseFailure::Tmux {
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail: error.to_string(),
                })?;
            require_tmux_absent(tmux_outcome)
                .map_err(|(reason, detail)| CloseLeaseFailure::Tmux { reason, detail })?;
            require_terminal_absent(self.terminals.complete_retirement(&lease.terminal).await)
                .map_err(|reason| CloseLeaseFailure::ProcessEpoch {
                    kind: RetiredResourceKind::PtySession,
                    reason,
                })?;
            require_browser_absent(
                self.browser_sessions()
                    .complete_retirement(&lease.browser)
                    .await,
            )
            .map_err(|reason| CloseLeaseFailure::ProcessEpoch {
                kind: RetiredResourceKind::BrowserSession,
                reason,
            })?;
            Ok(lease.resources.clone())
        }
        .await;
        if result.is_err() {
            self.close_retirement_leases.lock().await.insert(key, lease);
        }
        result
    }
}

async fn inspect_worktree(
    identity: &WorktreeIdentity,
) -> Result<(CloseRetirementSnapshot, Vec<CloseLossItem>), String> {
    let path = worktree_path(identity);
    let status_path = path.clone();
    let output = tokio::task::spawn_blocking(move || run_bounded_git_status(&status_path))
        .await
        .map_err(|error| error.to_string())??;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut observation = canonical_status_observation(&output.stdout);
    let mut losses = parse_status_losses(&output.stdout);
    observe_detached_head_and_submodules(&path, &mut observation, &mut losses).await?;
    Ok((snapshot_for(&observation), losses))
}

type WorktreeObservation = Vec<(Vec<u8>, Vec<u8>)>;

async fn observe_detached_head_and_submodules(
    path: &Path,
    observation: &mut Vec<u8>,
    losses: &mut Vec<CloseLossItem>,
) -> Result<(), String> {
    let path = path.to_path_buf();
    let loss_path = path.clone();
    let observed = tokio::task::spawn_blocking(move || -> Result<WorktreeObservation, String> {
        let mut visited = std::collections::HashSet::new();
        visited.insert(path.canonicalize().map_err(|error| error.to_string())?);
        let head = phoenix_core::git::command()
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(&path)
            .output()
            .map_err(|error| error.to_string())?;
        if !head.status.success() {
            return Err(String::from_utf8_lossy(&head.stderr).trim().to_string());
        }
        let head_oid = head.stdout.trim_ascii().to_vec();
        let detached = !phoenix_core::git::command()
            .args(["symbolic-ref", "-q", "HEAD"])
            .current_dir(&path)
            .status()
            .map_err(|error| error.to_string())?
            .success();
        let mut result = Vec::new();
        result.push((b"HEAD".to_vec(), head_oid.clone()));
        if detached {
            result.push((
                b"DETACHED".to_vec(),
                detached_reachability_evidence(&path, &head_oid)?,
            ));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        observe_initialized_submodules(&path, &[], &mut result, deadline, &mut visited)?;
        Ok(result)
    })
    .await
    .map_err(|error| error.to_string())??;
    let head_oid = observed
        .iter()
        .find_map(|(tag, value)| (tag == b"HEAD").then_some(value.clone()));
    for (tag, value) in observed {
        observation.extend_from_slice(&tag);
        observation.push(0);
        observation.extend_from_slice(&value);
        observation.push(0);
        if tag == b"DETACHED" && value.is_empty() {
            let head = head_oid
                .as_ref()
                .ok_or_else(|| "detached worktree has no resolved HEAD".to_string())?;
            for oid in detached_unreachable_commits(&loss_path, head)? {
                losses.push(CloseLossItem::DetachedUnreachableCommit(
                    GitOidIdentity::parse_hex(String::from_utf8_lossy(&oid).trim())
                        .map_err(|error| error.to_string())?,
                ));
            }
        }
        if let Some(record) = tag.strip_prefix(b"SUBMODULE_LOSS\0") {
            let separator = record
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| "malformed submodule loss record".to_string())?;
            let category = &record[..separator];
            let path = git_path_from_observation(&record[separator + 1..])?;
            losses.push(match category {
                b"staged" => CloseLossItem::StagedTrackedPath(path),
                b"unstaged" => CloseLossItem::UnstagedTrackedPath(path),
                b"untracked" => CloseLossItem::UntrackedNonIgnoredPath(path),
                b"submodule" => CloseLossItem::InitializedSubmoduleState(path),
                b"detached" => CloseLossItem::DetachedUnreachableCommit(
                    GitOidIdentity::parse_hex(String::from_utf8_lossy(path.as_bytes()).trim())
                        .map_err(|error| error.to_string())?,
                ),
                _ => return Err("unknown submodule loss category".to_string()),
            });
        }
    }
    Ok(())
}

fn detached_head_is_unreachable(
    repository: &Path,
    observation: &mut WorktreeObservation,
    relative_path: &[u8],
) -> Result<bool, String> {
    let detached = !phoenix_core::git::command()
        .args(["symbolic-ref", "-q", "HEAD"])
        .current_dir(repository)
        .status()
        .map_err(|error| error.to_string())?
        .success();
    if !detached {
        return Ok(false);
    }
    let head = phoenix_core::git::command()
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !head.status.success() {
        return Err(String::from_utf8_lossy(&head.stderr).trim().to_string());
    }
    let head_oid = head.stdout.trim_ascii().to_vec();
    let evidence = detached_reachability_evidence(repository, &head_oid)?;
    observation.push((
        [b"SUBMODULE_DETACHED\0".as_slice(), relative_path].concat(),
        evidence.clone(),
    ));
    Ok(evidence.is_empty())
}

fn detached_unreachable_commits(
    repository: &Path,
    head_oid: &[u8],
) -> Result<Vec<Vec<u8>>, String> {
    let head_text = std::str::from_utf8(head_oid).map_err(|error| error.to_string())?;
    let output = phoenix_core::git::command()
        .args([
            "rev-list",
            head_text,
            "--not",
            "--branches",
            "--remotes",
            "--tags",
            "--glob=refs/stash*",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|oid| !oid.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn detached_reachability_evidence(repository: &Path, head_oid: &[u8]) -> Result<Vec<u8>, String> {
    let head_text = std::str::from_utf8(head_oid).map_err(|error| error.to_string())?;
    let reachable = phoenix_core::git::command()
        .args([
            "for-each-ref",
            "--contains",
            head_text,
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
            "refs/stash",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !reachable.status.success() {
        return Err(String::from_utf8_lossy(&reachable.stderr)
            .trim()
            .to_string());
    }
    if !reachable.stdout.is_empty() {
        return Ok(reachable.stdout);
    }
    let _ = head_oid;
    let listing = phoenix_core::git::command()
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !listing.status.success() {
        return Err(String::from_utf8_lossy(&listing.stderr).trim().to_string());
    }
    let repository = repository
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize inspected worktree: {error}"))?;
    let git_dir = phoenix_core::git::command()
        .args(["rev-parse", "--absolute-git-dir"])
        .current_dir(&repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !git_dir.status.success() {
        return Err(String::from_utf8_lossy(&git_dir.stderr).trim().to_string());
    }
    let git_dir = path_buf_from_git_bytes(git_dir.stdout.trim_ascii())
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize inspected Git directory: {error}"))?;
    let mut worktree_path: Option<PathBuf> = None;
    for field in listing.stdout.split(|byte| *byte == 0) {
        if field.is_empty() {
            worktree_path = None;
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            worktree_path = Some(path_buf_from_git_bytes(path));
        } else if let Some(oid) = field.strip_prefix(b"HEAD ") {
            let is_other = worktree_path
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .is_some_and(|path| path != repository && path != git_dir);
            let _ = (oid, is_other);
        }
    }
    Ok(Vec::new())
}

fn run_bounded_git_status(repository: &Path) -> Result<std::process::Output, String> {
    run_bounded_git_status_until(
        repository,
        std::time::Instant::now() + std::time::Duration::from_secs(5),
    )
}

fn run_bounded_git_status_until(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<std::process::Output, String> {
    let mut command = phoenix_core::git::command();
    command
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ])
        .current_dir(repository)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return child.wait_with_output().map_err(|error| error.to_string());
        }
        if std::time::Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                let process_group = i32::try_from(child.id())
                    .map_err(|error| format!("Git status process id overflow: {error}"))?;
                libc::kill(-process_group, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = child.kill();
            child.wait().map_err(|error| error.to_string())?;
            return Err("Git status inspection exceeded its deadline".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

type IndexedGitlinks = (Vec<u8>, Vec<(GitPathIdentity, Vec<u8>)>);

fn index_gitlinks(repository: &Path) -> Result<IndexedGitlinks, String> {
    let output = phoenix_core::git::command()
        .args(["ls-files", "--stage", "-z", "--"])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect index gitlinks: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut gitlinks = Vec::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let separator = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| "malformed index entry: missing path separator".to_string())?;
        let mut fields = record[..separator].split(|byte| *byte == b' ');
        let mode = fields
            .next()
            .ok_or_else(|| "malformed index entry: missing mode".to_string())?;
        let oid = fields
            .next()
            .ok_or_else(|| "malformed index entry: missing object id".to_string())?;
        let stage = fields
            .next()
            .ok_or_else(|| "malformed index entry: missing stage".to_string())?;
        if mode == b"160000" && stage == b"0" {
            gitlinks.push((
                git_path_from_observation(&record[separator + 1..])?,
                oid.to_vec(),
            ));
        }
    }
    Ok((output.stdout, gitlinks))
}

#[allow(clippy::too_many_lines)]
fn observe_initialized_submodules(
    repository: &Path,
    relative_prefix: &[u8],
    observation: &mut WorktreeObservation,
    deadline: std::time::Instant,
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<(), String> {
    if std::time::Instant::now() >= deadline {
        return Err("Git submodule inspection exceeded its aggregate deadline".to_string());
    }
    let (index_observation, gitlinks) = index_gitlinks(repository)?;
    observation.push((
        [b"SUBMODULE_GITLINK_INDEX\0".as_slice(), relative_prefix].concat(),
        index_observation,
    ));
    for (path_identity, index_oid) in gitlinks {
        if std::time::Instant::now() >= deadline {
            return Err("Git submodule inspection exceeded its aggregate deadline".to_string());
        }
        let relative_path = join_git_paths(relative_prefix, path_identity.as_bytes())?;
        let submodule_path = repository.join(path_buf_from_git_bytes(path_identity.as_bytes()));
        if !submodule_path.join(".git").exists() {
            continue;
        }
        let canonical = submodule_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !visited.insert(canonical) {
            return Err("initialized submodule graph contains a cycle".to_string());
        }

        let status = run_bounded_git_status(&submodule_path)?;
        if !status.status.success() {
            return Err(format!(
                "cannot inspect initialized submodule {}: {}",
                String::from_utf8_lossy(&relative_path),
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
        observation.push((
            [b"SUBMODULE_STATUS\0".as_slice(), relative_path.as_slice()].concat(),
            canonical_status_observation(&status.stdout),
        ));

        let submodule_head = phoenix_core::git::command()
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(&submodule_path)
            .output()
            .map_err(|error| error.to_string())?;
        if !submodule_head.status.success() {
            return Err(format!(
                "cannot inspect initialized submodule gitlink {}: {}",
                String::from_utf8_lossy(&relative_path),
                String::from_utf8_lossy(&submodule_head.stderr).trim()
            ));
        }
        let submodule_head_oid = submodule_head.stdout.trim_ascii();
        observation.push((
            [b"SUBMODULE_GITLINK\0".as_slice(), relative_path.as_slice()].concat(),
            [index_oid.as_slice(), b"\0", submodule_head_oid].concat(),
        ));
        let detached_loss =
            detached_head_is_unreachable(&submodule_path, observation, &relative_path)?;
        let gitlink_changed = submodule_head_oid != index_oid;
        if gitlink_changed {
            observation.push((
                [
                    b"SUBMODULE_LOSS\0submodule\0".as_slice(),
                    relative_path.as_slice(),
                ]
                .concat(),
                Vec::new(),
            ));
        }
        if detached_loss {
            for oid in detached_unreachable_commits(&submodule_path, submodule_head_oid)? {
                observation.push((
                    [b"SUBMODULE_LOSS\0detached\0".as_slice(), oid.as_slice()].concat(),
                    Vec::new(),
                ));
            }
        }
        if !parse_status_losses(&status.stdout).is_empty() {
            observation.push((
                [
                    b"SUBMODULE_LOSS\0submodule\0".as_slice(),
                    relative_path.as_slice(),
                ]
                .concat(),
                Vec::new(),
            ));
        }
        for loss in parse_status_losses(&status.stdout) {
            let (category, nested_path) = match loss {
                CloseLossItem::StagedTrackedPath(path) => (b"staged".as_slice(), path),
                CloseLossItem::UnstagedTrackedPath(path) => (b"unstaged".as_slice(), path),
                CloseLossItem::UntrackedNonIgnoredPath(path) => (b"untracked".as_slice(), path),
                _ => continue,
            };
            let full_path = join_git_paths(&relative_path, nested_path.as_bytes())?;
            observation.push((
                [
                    b"SUBMODULE_LOSS\0".as_slice(),
                    category,
                    b"\0".as_slice(),
                    full_path.as_slice(),
                ]
                .concat(),
                Vec::new(),
            ));
        }
        observe_initialized_submodules(
            &submodule_path,
            &relative_path,
            observation,
            deadline,
            visited,
        )?;
    }
    Ok(())
}

fn git_path_from_observation(bytes: &[u8]) -> Result<GitPathIdentity, String> {
    if bytes.is_empty() {
        return Err("observed Git path is empty".to_string());
    }
    if bytes.contains(&0) {
        return Err("observed Git path contains NUL".to_string());
    }
    #[cfg(not(unix))]
    std::str::from_utf8(bytes)
        .map_err(|_| "observed Git path is not valid platform text".to_string())?;
    let path = path_buf_from_git_bytes(bytes);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("observed Git path escapes its repository".to_string());
    }
    Ok(GitPathIdentity::from_bytes(bytes.to_vec()))
}

fn join_git_paths(prefix: &[u8], path: &[u8]) -> Result<Vec<u8>, String> {
    let mut joined =
        Vec::with_capacity(prefix.len() + usize::from(!prefix.is_empty()) + path.len());
    joined.extend_from_slice(prefix);
    if !prefix.is_empty() {
        joined.push(b'/');
    }
    joined.extend_from_slice(path);
    git_path_from_observation(&joined)?;
    Ok(joined)
}

fn path_buf_from_git_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(
            std::str::from_utf8(bytes)
                .expect("Git path bytes are validated before platform conversion"),
        )
    }
}

fn worktree_quarantine_path(identity: &WorktreeIdentity) -> Result<PathBuf, String> {
    let path = worktree_path(identity);
    let parent = path
        .parent()
        .ok_or_else(|| "captured worktree has no parent directory".to_string())?;
    let mut name = path
        .file_name()
        .ok_or_else(|| "captured worktree has no final path component".to_string())?
        .to_os_string();
    let digest = Sha256::digest(
        [
            identity.id().as_str().as_bytes(),
            b"\0",
            identity.fingerprint().as_str().as_bytes(),
        ]
        .concat(),
    );
    let mut suffix = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut suffix, "{byte:02x}").expect("writing to String cannot fail");
    }
    name.push(format!(".phoenix-close-{suffix}"));
    Ok(parent.join(name))
}

enum ExactWorktreeRemoval {
    Missing { reason: String },
    ReinspectionRequired { detail: String },
    Residual { detail: String },
}

fn inspect_and_remove_exact_worktree(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
) -> Result<ExactWorktreeRemoval, String> {
    inspect_and_remove_exact_worktree_with_hook(runtime, identity, confirmed_snapshot, |_| {})
}

fn inspect_and_remove_exact_worktree_with_hook<F>(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    after_quarantine: F,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
{
    let path = worktree_path(identity);
    let quarantine = worktree_quarantine_path(identity)?;
    if !path
        .try_exists()
        .map_err(|error| format!("cannot observe captured worktree path: {error}"))?
        && !quarantine
            .try_exists()
            .map_err(|error| format!("cannot observe quarantined worktree path: {error}"))?
    {
        return Ok(ExactWorktreeRemoval::Missing {
            reason: "captured worktree path is absent".to_string(),
        });
    }
    let _repository_lock =
        RepositoryMutationLock::acquire(if path.exists() { &path } else { &quarantine })
            .map_err(|(message, _)| message)?;
    if path.exists() {
        let (fresh_snapshot, _) = runtime.block_on(inspect_worktree(identity))?;
        if &fresh_snapshot != confirmed_snapshot {
            return Ok(ExactWorktreeRemoval::ReinspectionRequired {
                detail: "worktree changed after Close inspection confirmation; fresh confirmation is required"
                    .to_string(),
            });
        }
    }
    runtime.block_on(quarantine_and_remove_exact_worktree(
        identity,
        after_quarantine,
    ))
}

fn path_is_within(candidate: &Path, directory: &Path) -> bool {
    candidate == directory || candidate.starts_with(directory)
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors(path: &Path) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantined worktree before descriptor inspection: {error}")
    })?;
    let processes = std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate process descriptors: {error}"))?;
    for process in processes.flatten() {
        if !process
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let Ok(descriptors) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            if std::fs::read_link(descriptor.path())
                .is_ok_and(|candidate| path_is_within(&candidate, &canonical))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn quarantine_has_open_descriptors(path: &Path) -> Result<bool, String> {
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};
    use std::os::unix::ffi::OsStrExt as _;

    #[repr(C)]
    struct ProcFileInfo {
        open_flags: u32,
        status: u32,
        offset: i64,
        file_type: i32,
        guard_flags: u32,
    }
    #[repr(C)]
    struct VnodeFdInfoWithPath {
        file: ProcFileInfo,
        vnode: libc::vnode_info_path,
    }

    const PROC_ALL_PIDS: u32 = 1;
    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantined worktree before descriptor inspection: {error}")
    })?;
    let mut pids = vec![0_i32; 4096];
    // SAFETY: the vector provides writable storage for exactly the byte count passed.
    let pid_bytes = unsafe {
        libc::proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr().cast(),
            i32::try_from(pids.len() * size_of::<i32>()).expect("PID buffer fits i32"),
        )
    };
    if pid_bytes < 0 {
        return Err("cannot enumerate process descriptors".to_string());
    }
    pids.truncate(
        usize::try_from(pid_bytes).expect("nonnegative PID byte count") / size_of::<i32>(),
    );
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        let mut descriptors = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0
            };
            256
        ];
        // SAFETY: the vector provides writable storage for exactly the byte count passed.
        let descriptor_bytes = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                descriptors.as_mut_ptr().cast(),
                i32::try_from(descriptors.len() * size_of::<libc::proc_fdinfo>())
                    .expect("descriptor buffer fits i32"),
            )
        };
        if descriptor_bytes <= 0 {
            continue;
        }
        descriptors.truncate(
            usize::try_from(descriptor_bytes).expect("positive descriptor byte count")
                / size_of::<libc::proc_fdinfo>(),
        );
        for descriptor in descriptors
            .into_iter()
            .filter(|descriptor| descriptor.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32)
        {
            let mut info = MaybeUninit::<VnodeFdInfoWithPath>::uninit();
            // SAFETY: proc_pidfdinfo initializes the declared C-compatible structure on success.
            let bytes = unsafe {
                libc::proc_pidfdinfo(
                    pid,
                    descriptor.proc_fd,
                    PROC_PIDFDVNODEPATHINFO,
                    info.as_mut_ptr().cast(),
                    i32::try_from(size_of::<VnodeFdInfoWithPath>()).expect("vnode info fits i32"),
                )
            };
            if bytes
                != i32::try_from(size_of::<VnodeFdInfoWithPath>())
                    .expect("vnode info size fits i32")
            {
                continue;
            }
            // SAFETY: the exact structure size was reported as initialized above.
            let info = unsafe { info.assume_init() };
            let path_bytes = info.vnode.vip_path.as_flattened();
            // SAFETY: the kernel returns a NUL-terminated MAXPATHLEN path buffer.
            let candidate = unsafe { CStr::from_ptr(path_bytes.as_ptr()) };
            if path_is_within(
                Path::new(std::ffi::OsStr::from_bytes(candidate.to_bytes())),
                &canonical,
            ) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_open_descriptors(_path: &Path) -> Result<bool, String> {
    Err("open-descriptor inspection is unsupported on this platform".to_string())
}

#[allow(clippy::too_many_lines)]
async fn quarantine_and_remove_exact_worktree<F>(
    identity: &WorktreeIdentity,
    after_quarantine: F,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
{
    let path = worktree_path(identity);
    let quarantine = worktree_quarantine_path(identity)?;
    let resuming_quarantine = !path.exists() && quarantine.exists();
    if !path.exists() && !resuming_quarantine {
        return Err("captured worktree path is absent without an exact prior receipt".to_string());
    }
    let expected = identity.fingerprint().as_str().to_string();
    tokio::task::spawn_blocking(move || {
        let inspection_path = if resuming_quarantine { &quarantine } else { &path };
        let common = phoenix_core::git::command()
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .current_dir(inspection_path)
            .output()
            .map_err(|error| error.to_string())?;
        if !common.status.success() {
            return Err("captured worktree is not server-owned Git worktree".to_string());
        }
        let common = path_buf_from_git_bytes(common.stdout.trim_ascii());
        let repo = if common.join("HEAD").is_file() && !common.join(".git").exists() {
            common.clone()
        } else {
            common
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| "captured worktree has no common repository root".to_string())?
        };
        let target = std::fs::canonicalize(inspection_path).map_err(|error| error.to_string())?;
        let listed = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain", "-z"])
            .current_dir(&repo)
            .output()
            .map_err(|error| error.to_string())?;
        let registered = listed.status.success()
            && listed.stdout.split(|byte| *byte == 0).any(|field| {
                field
                    .strip_prefix(b"worktree ")
                    .map(path_buf_from_git_bytes)
                    .and_then(|candidate| std::fs::canonicalize(candidate).ok())
                    .as_ref()
                    == Some(&target)
            });
        if !registered && !resuming_quarantine {
            return Err("captured worktree is no longer Git registered".to_string());
        }
        if observe_worktree_fingerprint(inspection_path).as_deref() != Some(expected.as_str()) {
            return Err("captured worktree administrative incarnation changed".to_string());
        }

        if !resuming_quarantine {
            if quarantine
                .try_exists()
                .map_err(|error| format!("cannot observe quarantined worktree path: {error}"))?
            {
                return Ok(ExactWorktreeRemoval::Residual {
                    detail: format!(
                        "confirmed worktree remains quarantined at {}",
                        quarantine.display()
                    ),
                });
            }
            std::fs::rename(&path, &quarantine)
                .map_err(|error| format!("cannot quarantine captured worktree: {error}"))?;
        }
        if observe_worktree_fingerprint(&quarantine).as_deref() != Some(expected.as_str()) {
            let _ = std::fs::rename(&quarantine, &path);
            return Err("quarantined worktree administrative incarnation changed".to_string());
        }

        after_quarantine(&path);
        if quarantine_has_open_descriptors(&quarantine)? {
            return Ok(ExactWorktreeRemoval::Residual {
                detail: format!(
                    "open descriptors can still modify the confirmed worktree; retained at {}",
                    quarantine.display()
                ),
            });
        }
        if path
            .try_exists()
            .map_err(|error| format!("cannot inspect original worktree path: {error}"))?
        {
            return Ok(ExactWorktreeRemoval::Residual {
                detail: format!(
                    "post-inspection write survived at {}; confirmed worktree is retained at {}",
                    path.display(),
                    quarantine.display()
                ),
            });
        }
        Ok(ExactWorktreeRemoval::Residual {
            detail: format!(
                "writer exclusion cannot be held through recursive deletion; confirmed worktree retained at {}",
                quarantine.display()
            ),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn canonical_status_observation(status: &[u8]) -> Vec<u8> {
    let mut observation = Vec::with_capacity(status.len());
    let mut rows = status.split(|byte| *byte == 0);
    while let Some(row) = rows.next() {
        if row.len() < 4 || &row[..2] == b"!!" {
            continue;
        }
        observation.extend_from_slice(row);
        observation.push(0);
        if matches!(row[0], b'R' | b'C') || matches!(row[1], b'R' | b'C') {
            if let Some(source) = rows.next() {
                observation.extend_from_slice(source);
                observation.push(0);
            }
        }
    }
    observation
}

fn parse_status_losses(status: &[u8]) -> Vec<CloseLossItem> {
    let mut losses = Vec::new();
    let mut rows = status.split(|byte| *byte == 0);
    while let Some(row) = rows.next() {
        if row.len() < 4 {
            continue;
        }
        let path = GitPathIdentity::from_bytes(row[3..].to_vec());
        match &row[..2] {
            b"??" => losses.push(CloseLossItem::UntrackedNonIgnoredPath(path)),
            b"!!" => {}
            xy => {
                let unmerged = matches!(xy, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU");
                if unmerged {
                    losses.push(CloseLossItem::UnstagedTrackedPath(path));
                } else {
                    if xy[0] != b' ' {
                        losses.push(CloseLossItem::StagedTrackedPath(path.clone()));
                    }
                    if xy[1] != b' ' {
                        losses.push(CloseLossItem::UnstagedTrackedPath(path));
                    }
                }
                if matches!(xy[0], b'R' | b'C') || matches!(xy[1], b'R' | b'C') {
                    let _source_path = rows.next();
                }
            }
        }
    }
    losses.sort_by_key(|loss| (loss.category().as_str(), loss.identity().value()));
    losses.dedup();
    losses
}

fn snapshot_for(bytes: &[u8]) -> CloseRetirementSnapshot {
    let digest = Sha256::digest(bytes);
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut fingerprint, "{byte:02x}").expect("writing into String cannot fail");
    }
    CloseRetirementSnapshot::parse("server_git_status_v1", fingerprint)
        .expect("constant generation and SHA-256 fingerprint are valid")
}

fn observe_worktree_fingerprint(path: &Path) -> Option<String> {
    phoenix_core::git::observe_worktree_fingerprint(path)
}

fn worktree_path(identity: &WorktreeIdentity) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(
            identity.locator().as_bytes().to_vec(),
        ))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(identity.locator().as_bytes()).into_owned())
    }
}

fn is_runtime_resource(kind: RetiredResourceKind) -> bool {
    matches!(
        kind,
        RetiredResourceKind::BashProcessGroup
            | RetiredResourceKind::TmuxServer
            | RetiredResourceKind::PtySession
            | RetiredResourceKind::BrowserSession
    )
}

fn resource_key(resource: &RetiredResourceIdentity) -> (String, String) {
    (
        resource.kind().as_str().to_string(),
        resource.identity().value(),
    )
}

fn opaque_resource(kind: RetiredResourceKind, value: String) -> RetiredResourceIdentity {
    RetiredResourceIdentity::parse(
        kind,
        phoenix_core::domain::close::LossItemIdentity::Opaque(
            OpaqueIdentity::parse(value).expect("registry stable instance identity is non-empty"),
        ),
    )
    .expect("registry resource kind accepts opaque stable identity")
}

fn require_absent(outcome: BashRetirementOutcome) -> Result<(), String> {
    match outcome {
        BashRetirementOutcome::Retired(report) | BashRetirementOutcome::AbsenceVerified(report)
            if report.kill_failures.is_empty() =>
        {
            Ok(())
        }
        BashRetirementOutcome::Retired(report) | BashRetirementOutcome::AbsenceVerified(report) => {
            Err(format!(
                "bash retirement left {} kill failure(s)",
                report.kill_failures.len()
            ))
        }
    }
}

fn require_tmux_absent(
    outcome: TmuxRetirementOutcome,
) -> Result<(), (RetirementFailureReason, String)> {
    match outcome {
        TmuxRetirementOutcome::Retired | TmuxRetirementOutcome::AbsenceVerified => Ok(()),
        TmuxRetirementOutcome::IdentityNotProven { reason } => {
            Err((RetirementFailureReason::IdentityNotProven, reason))
        }
        TmuxRetirementOutcome::RemovalFailed { reason } => {
            Err((RetirementFailureReason::RemovalFailed, reason))
        }
    }
}

fn require_terminal_absent(outcome: TerminalRetirementOutcome) -> Result<(), String> {
    match outcome {
        TerminalRetirementOutcome::Retired | TerminalRetirementOutcome::AbsenceVerified => Ok(()),
        TerminalRetirementOutcome::Residual { reason } => Err(reason),
    }
}

fn require_browser_absent(outcome: BrowserRetirementOutcome) -> Result<(), String> {
    match outcome {
        BrowserRetirementOutcome::Retired | BrowserRetirementOutcome::AbsenceVerified => Ok(()),
        BrowserRetirementOutcome::Residual { reason } => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_status_observation, git_path_from_observation,
        inspect_and_remove_exact_worktree_with_hook, inspect_worktree, parse_status_losses,
        quarantine_and_remove_exact_worktree, run_bounded_git_status_until, snapshot_for,
        worktree_quarantine_path, CloseLeaseFailure, ExactWorktreeRemoval,
    };
    use phoenix_core::domain::close::{
        CloseLossItem, GitPathIdentity, WorktreeFingerprint, WorktreeId, WorktreeIdentity,
    };
    use std::path::Path;

    fn run_git(repository: &Path, arguments: &[&str]) {
        let output = phoenix_core::git::command()
            .args(arguments)
            .current_dir(repository)
            .env("GIT_AUTHOR_NAME", "Close Test")
            .env("GIT_AUTHOR_EMAIL", "close@example.invalid")
            .env("GIT_COMMITTER_NAME", "Close Test")
            .env("GIT_COMMITTER_EMAIL", "close@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn inspection_identity(path: &Path) -> WorktreeIdentity {
        #[cfg(unix)]
        let locator = {
            use std::os::unix::ffi::OsStrExt as _;
            GitPathIdentity::from_bytes(path.as_os_str().as_bytes().to_vec())
        };
        #[cfg(not(unix))]
        let locator = GitPathIdentity::from_bytes(path.to_string_lossy().as_bytes().to_vec());
        WorktreeIdentity::from_parts(
            WorktreeId::parse("inspection-test-worktree").unwrap(),
            WorktreeFingerprint::parse(
                phoenix_core::git::observe_worktree_fingerprint(path).unwrap(),
            )
            .unwrap(),
            locator,
        )
    }

    fn initialize_repository(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        run_git(path, &["init", "--quiet"]);
        std::fs::write(path.join("tracked"), "initial\n").unwrap();
        run_git(path, &["add", "tracked"]);
        run_git(path, &["commit", "--quiet", "-m", "initial"]);
    }

    #[tokio::test]
    async fn remove_exact_worktree_uses_bare_common_repository_root() {
        let temp = tempfile::tempdir().unwrap();
        let seed = temp.path().join("seed");
        let bare = temp.path().join("origin.git");
        let linked = temp.path().join("linked");
        initialize_repository(&seed);
        run_git(
            temp.path(),
            &[
                "clone",
                "--quiet",
                "--bare",
                seed.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        run_git(
            &bare,
            &[
                "worktree",
                "add",
                "--quiet",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let fingerprint = phoenix_core::git::observe_worktree_fingerprint(&linked).unwrap();
        #[cfg(unix)]
        let locator = {
            use std::os::unix::ffi::OsStrExt as _;
            GitPathIdentity::from_bytes(linked.as_os_str().as_bytes().to_vec())
        };
        #[cfg(not(unix))]
        let locator = GitPathIdentity::from_bytes(linked.to_string_lossy().as_bytes().to_vec());
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("bare-linked-worktree").unwrap(),
            WorktreeFingerprint::parse(fingerprint).unwrap(),
            locator,
        );

        let outcome = quarantine_and_remove_exact_worktree(&identity, |_| {})
            .await
            .unwrap();
        let ExactWorktreeRemoval::Residual { detail } = outcome else {
            panic!("worktree must remain quarantined without writer exclusion");
        };
        assert!(detail.contains("writer exclusion"));
        assert!(!linked.exists());
        assert!(worktree_quarantine_path(&identity).unwrap().exists());
        let listing = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&bare)
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert!(String::from_utf8_lossy(&listing.stdout).contains(linked.to_str().unwrap()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn post_inspection_write_survives_inode_bound_quarantine() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("repository");
        let linked = temp.path().join("linked");
        initialize_repository(&repository);
        run_git(
            &repository,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        std::fs::write(linked.join("confirmed-loss"), "confirmed\n").unwrap();
        let fingerprint = phoenix_core::git::observe_worktree_fingerprint(&linked).unwrap();
        #[cfg(unix)]
        let locator = {
            use std::os::unix::ffi::OsStrExt as _;
            GitPathIdentity::from_bytes(linked.as_os_str().as_bytes().to_vec())
        };
        #[cfg(not(unix))]
        let locator = GitPathIdentity::from_bytes(linked.to_string_lossy().as_bytes().to_vec());
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("race-linked-worktree").unwrap(),
            WorktreeFingerprint::parse(fingerprint).unwrap(),
            locator,
        );
        let (snapshot, _) = inspect_worktree(&identity).await.unwrap();
        let runtime = tokio::runtime::Handle::current();
        let retry_runtime = runtime.clone();
        let retry_identity = identity.clone();
        let retry_snapshot = snapshot.clone();
        let late_path = linked.join("late-write");
        let result = tokio::task::spawn_blocking(move || {
            inspect_and_remove_exact_worktree_with_hook(
                &runtime,
                &identity,
                &snapshot,
                move |original| {
                    std::fs::create_dir(original).unwrap();
                    std::fs::write(original.join("late-write"), "must survive\n").unwrap();
                },
            )
        })
        .await
        .unwrap()
        .unwrap();

        let ExactWorktreeRemoval::Residual { detail } = result else {
            panic!("late write must refuse retirement");
        };
        assert!(detail.contains("confirmed worktree is retained at"));
        assert_eq!(
            std::fs::read_to_string(&late_path).unwrap(),
            "must survive\n"
        );
        let listing = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&repository)
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert!(String::from_utf8_lossy(&listing.stdout).contains(linked.to_str().unwrap()));

        std::fs::remove_dir_all(&linked).unwrap();
        let retry_quarantine = worktree_quarantine_path(&retry_identity).unwrap();
        let retry = tokio::task::spawn_blocking(move || {
            inspect_and_remove_exact_worktree_with_hook(
                &retry_runtime,
                &retry_identity,
                &retry_snapshot,
                |_| {},
            )
        })
        .await
        .unwrap()
        .unwrap();
        let ExactWorktreeRemoval::Residual { detail } = retry else {
            panic!("retry must retain quarantine without writer exclusion");
        };
        assert!(detail.contains("writer exclusion"));
        assert!(retry_quarantine.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pre_quarantine_snapshot_change_requests_reinspection_without_renaming() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("repository");
        let linked = temp.path().join("linked");
        initialize_repository(&repository);
        run_git(
            &repository,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let identity = inspection_identity(&linked);
        let (confirmed, _) = inspect_worktree(&identity).await.unwrap();
        std::fs::write(linked.join("changed-after-confirmation"), "preserve\n").unwrap();
        let runtime = tokio::runtime::Handle::current();

        let outcome = tokio::task::spawn_blocking(move || {
            inspect_and_remove_exact_worktree_with_hook(&runtime, &identity, &confirmed, |_| {})
        })
        .await
        .unwrap()
        .unwrap();

        assert!(matches!(
            outcome,
            ExactWorktreeRemoval::ReinspectionRequired { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(linked.join("changed-after-confirmation")).unwrap(),
            "preserve\n"
        );
    }

    #[tokio::test]
    async fn detached_commit_reachability_ignores_nondurable_custom_refs() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["checkout", "--quiet", "--detach"]);
        std::fs::write(temp.path().join("tracked"), "detached\n").unwrap();
        run_git(temp.path(), &["commit", "--quiet", "-am", "detached"]);
        run_git(temp.path(), &["update-ref", "refs/custom/keep", "HEAD"]);

        let identity = inspection_identity(temp.path());
        let (_, custom_ref_losses) = inspect_worktree(&identity).await.unwrap();
        assert!(custom_ref_losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));

        run_git(temp.path(), &["update-ref", "refs/tags/keep", "HEAD"]);
        let (_, durable_ref_losses) = inspect_worktree(&identity).await.unwrap();
        assert!(!durable_ref_losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));
    }

    #[tokio::test]
    async fn detached_commit_held_only_by_another_worktree_is_reported_as_loss() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["checkout", "--quiet", "--detach"]);
        std::fs::write(temp.path().join("tracked"), "detached\n").unwrap();
        run_git(temp.path(), &["commit", "--quiet", "-am", "detached"]);
        let sibling = temp
            .path()
            .with_file_name(format!("close-inspection-sibling-{}", uuid::Uuid::new_v4()));
        run_git(
            temp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                sibling.to_str().unwrap(),
                "HEAD",
            ],
        );

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));
        run_git(
            temp.path(),
            &["worktree", "remove", "--force", sibling.to_str().unwrap()],
        );
    }

    #[tokio::test]
    async fn initialized_submodule_dirty_state_changes_snapshot_and_emits_exact_path() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        let parent = temp.path().join("parent");
        initialize_repository(&child);
        initialize_repository(&parent);
        let child_text = child.to_string_lossy().into_owned();
        let output = phoenix_core::git::command()
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                &child_text,
                "deps/child",
            ])
            .current_dir(&parent)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "submodule add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        run_git(&parent, &["commit", "--quiet", "-am", "add submodule"]);

        let identity = inspection_identity(&parent);
        let (clean_snapshot, clean_losses) = inspect_worktree(&identity).await.unwrap();
        assert!(!clean_losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::InitializedSubmoduleState(_))));

        std::fs::write(parent.join("deps/child/untracked"), "nested loss\n").unwrap();
        let (dirty_snapshot, dirty_losses) = inspect_worktree(&identity).await.unwrap();
        assert_ne!(clean_snapshot, dirty_snapshot);
        assert!(dirty_losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::UntrackedNonIgnoredPath(path)
                if path.as_bytes() == b"deps/child/untracked"
        )));

        std::fs::remove_file(parent.join(".gitmodules")).unwrap();
        let (_, missing_declaration_losses) = inspect_worktree(&identity).await.unwrap();
        assert!(missing_declaration_losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::UntrackedNonIgnoredPath(path)
                if path.as_bytes() == b"deps/child/untracked"
        )));
    }

    #[tokio::test]
    async fn initialized_submodule_detached_commit_is_reported_as_exact_oid_loss() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child-detached");
        let parent = temp.path().join("parent-detached");
        initialize_repository(&child);
        initialize_repository(&parent);
        let child_text = child.to_string_lossy().into_owned();
        let output = phoenix_core::git::command()
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                &child_text,
                "deps/child",
            ])
            .current_dir(&parent)
            .output()
            .unwrap();
        assert!(output.status.success());
        run_git(&parent, &["commit", "--quiet", "-am", "add submodule"]);

        let closing = temp.path().join("closing-worktree");
        run_git(
            &parent,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "closing-test",
                closing.to_str().unwrap(),
            ],
        );
        let update = phoenix_core::git::command()
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "--quiet",
            ])
            .current_dir(&closing)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(update.status.success());
        let submodule = closing.join("deps/child");
        run_git(&submodule, &["checkout", "--quiet", "--detach"]);
        std::fs::write(submodule.join("tracked"), "detached submodule\n").unwrap();
        run_git(
            &submodule,
            &["commit", "--quiet", "-am", "detached submodule"],
        );
        run_git(&closing, &["add", "deps/child"]);
        run_git(
            &closing,
            &["commit", "--quiet", "-m", "record detached gitlink"],
        );

        let identity = inspection_identity(&closing);
        let (_, losses) = inspect_worktree(&identity).await.unwrap();
        assert!(losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));
        let outcome = quarantine_and_remove_exact_worktree(&identity, |_| {})
            .await
            .unwrap();
        let ExactWorktreeRemoval::Residual { detail } = outcome else {
            panic!("submodule worktree must remain quarantined without writer exclusion");
        };
        assert!(detail.contains("writer exclusion"));
        assert!(!closing.exists());
        assert!(worktree_quarantine_path(&identity).unwrap().exists());
    }

    #[test]
    fn descriptor_scan_has_no_external_executable_dependency() {
        let source = include_str!("close_retirement.rs");
        assert!(!source.contains("Command::new(\"lsof\")"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quarantine_preserves_worktree_with_open_file_descriptor() {
        use std::io::Write as _;

        let temp = tempfile::tempdir().unwrap();
        let closing = temp.path().join("closing");
        initialize_repository(&closing);
        let tracked = closing.join("tracked");
        std::fs::write(&tracked, "before\n").unwrap();
        run_git(&closing, &["add", "tracked"]);
        run_git(&closing, &["commit", "--quiet", "-m", "tracked"]);
        let identity = inspection_identity(&closing);
        let mut descriptor = std::fs::OpenOptions::new()
            .append(true)
            .open(&tracked)
            .unwrap();

        let outcome = quarantine_and_remove_exact_worktree(&identity, move |_| {
            descriptor.write_all(b"after\n").unwrap();
            descriptor.flush().unwrap();
            std::mem::forget(descriptor);
        })
        .await
        .unwrap();

        let ExactWorktreeRemoval::Residual { detail } = outcome else {
            panic!("open descriptor must preserve quarantine");
        };
        assert!(detail.contains("open descriptors"));
        assert!(!closing.exists());
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        assert_eq!(
            std::fs::read_to_string(quarantine.join("tracked")).unwrap(),
            "before\nafter\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn top_level_status_probe_kills_git_at_its_deadline() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let hook = temp.path().join("blocking-fsmonitor.sh");
        let marker = temp.path().join("fsmonitor-started");
        let gate = temp.path().join("fsmonitor-gate");
        std::fs::write(&gate, "block\n").unwrap();
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nwhile test -e '{}'; do sleep 0.05; done\n",
                marker.display(),
                gate.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
        run_git(
            temp.path(),
            &["config", "core.fsmonitor", hook.to_str().unwrap()],
        );

        let error = run_bounded_git_status_until(
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )
        .unwrap_err();
        let marker_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !marker.exists() && std::time::Instant::now() < marker_deadline {
            // test-timing-allow: polling for the observable fsmonitor marker is the bounded process-start handshake
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::remove_file(&gate).unwrap();

        assert_eq!(error, "Git status inspection exceeded its deadline");
        assert!(marker.exists(), "fsmonitor hook was not exercised");
        let hook_pid: i32 = std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let reaped_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while unsafe { libc::kill(hook_pid, 0) } == 0 && std::time::Instant::now() < reaped_deadline
        {
            // test-timing-allow: process absence is the observable deadline behavior under test
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(hook_pid, 0) },
            0,
            "fsmonitor hook survived the Git status deadline"
        );
    }

    #[tokio::test]
    async fn empty_gitmodules_is_a_valid_empty_declaration_set() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        std::fs::write(temp.path().join(".gitmodules"), "# no submodules\n").unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(!losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::InitializedSubmoduleState(_))));
    }

    #[test]
    fn porcelain_rename_source_is_not_parsed_as_another_status_record() {
        let losses = parse_status_losses(b"R  new-name.txt\0old-long-name.txt\0");
        assert_eq!(
            losses,
            vec![CloseLossItem::StagedTrackedPath(
                GitPathIdentity::from_bytes(b"new-name.txt".to_vec())
            )]
        );
    }

    #[test]
    fn self_referential_git_path_is_rejected() {
        assert!(git_path_from_observation(b".").is_err());
        assert!(git_path_from_observation(b"./child").is_err());
    }

    #[test]
    fn malformed_observed_git_paths_return_errors_without_panicking() {
        assert!(git_path_from_observation(b"").is_err());
        assert!(git_path_from_observation(b"../outside").is_err());
        assert!(git_path_from_observation(b"inside\0outside").is_err());
    }

    #[test]
    fn close_lease_failure_origin_distinguishes_tmux_from_process_epoch() {
        let process_epoch = CloseLeaseFailure::ProcessEpoch {
            kind: phoenix_core::domain::close::RetiredResourceKind::BrowserSession,
            reason: "profile identity changed".to_string(),
        };
        let tmux = CloseLeaseFailure::Tmux {
            reason: phoenix_core::domain::close::RetirementFailureReason::IdentityNotProven,
            detail: "server token changed".to_string(),
        };

        assert!(matches!(
            process_epoch,
            CloseLeaseFailure::ProcessEpoch {
                kind: phoenix_core::domain::close::RetiredResourceKind::BrowserSession,
                ..
            }
        ));
        assert!(matches!(tmux, CloseLeaseFailure::Tmux { .. }));
    }

    #[test]
    fn porcelain_loss_parser_distinguishes_loss_categories_and_ignores_ignored_paths() {
        let losses =
            parse_status_losses(b"M  staged\0 M unstaged\0MM both\0?? untracked\0!! ignored\0");
        assert_eq!(losses.len(), 5);
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::StagedTrackedPath(path) if path.as_bytes() == b"staged")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"unstaged")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::StagedTrackedPath(path) if path.as_bytes() == b"both")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"both")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::UntrackedNonIgnoredPath(path) if path.as_bytes() == b"untracked")));
    }

    #[test]
    fn ignored_paths_do_not_change_canonical_snapshot() {
        let clean = snapshot_for(&canonical_status_observation(b""));
        let ignored = snapshot_for(&canonical_status_observation(b"!! target/log.txt\0"));
        assert_eq!(clean, ignored);
    }

    #[test]
    fn canonical_status_preserves_rename_source_record() {
        assert_eq!(
            canonical_status_observation(b"R  new-name.txt\0old-name.txt\0!! ignored\0"),
            b"R  new-name.txt\0old-name.txt\0"
        );
    }

    #[test]
    fn porcelain_snapshot_digest_changes_with_server_observation() {
        let clean = snapshot_for(b"");
        let dirty = snapshot_for(b"?? server-observed\0");
        assert_eq!(clean.generation(), "server_git_status_v1");
        assert_ne!(clean.fingerprint(), dirty.fingerprint());
    }
}
