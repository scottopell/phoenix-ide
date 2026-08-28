//! Exact-instance resource fences and receipts for Close retirement.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use phoenix_core::domain::close::{
    AbsenceBasis, CapturedWorktreeIdentity, CloseAttemptId, CloseLossItem,
    CloseOwnedResourceInventory, ClosePhase, CloseRetirementSnapshot, GitOidIdentity,
    GitPathIdentity, LossItemIdentity, OpaqueIdentity, RetiredResourceIdentity,
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
                    self.db()
                        .route_close_attempt_to_repair(&attempt_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Err(format!(
                        "{} process-epoch Close teardown requires repair for sealed scope {scope}: {reason}",
                        kind.as_str()
                    ));
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
                    let fresh_snapshot = match final_removal {
                        Ok(ExactWorktreeRemoval::Removed) => Some(confirmed.snapshot.clone()),
                        Ok(ExactWorktreeRemoval::Missing { reason }) => {
                            let absence_basis = if self
                                .db()
                                .close_retirement_resource_was_dispatched(
                                    attempt_id,
                                    &scope,
                                    snapshot,
                                    &target.resource,
                                )
                                .await
                                .map_err(|error| error.to_string())?
                            {
                                AbsenceBasis::SameAttemptPriorRetirement
                            } else {
                                AbsenceBasis::PreexistingExactIdentityEvidence
                            };
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
                        Ok(ExactWorktreeRemoval::Changed) => {
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
    let output = tokio::task::spawn_blocking(move || {
        phoenix_core::git::command()
            .args([
                "status",
                "--porcelain=v1",
                "-z",
                "--ignored",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ])
            .current_dir(status_path)
            .output()
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
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
        observe_initialized_submodules(&path, &[], &mut result)?;
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
            "--glob=refs/stash",
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
            if oid == head_oid && is_other {
                return Ok(b"OTHER_WORKTREE_HEAD\n".to_vec());
            }
        }
    }
    Ok(Vec::new())
}

#[allow(clippy::too_many_lines)]
fn observe_initialized_submodules(
    repository: &Path,
    relative_prefix: &[u8],
    observation: &mut WorktreeObservation,
) -> Result<(), String> {
    let modules_file = repository.join(".gitmodules");
    if !modules_file.exists() {
        return Ok(());
    }
    let declarations = phoenix_core::git::command()
        .args([
            "config",
            "-z",
            "--file",
            ".gitmodules",
            "--get-regexp",
            "^submodule\\..*\\.path$",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    let no_declared_submodules = declarations.status.code() == Some(1)
        && declarations.stdout.is_empty()
        && declarations.stderr.is_empty();
    if !declarations.status.success() && !no_declared_submodules {
        return Err(format!(
            "cannot read declared submodule paths: {}",
            String::from_utf8_lossy(&declarations.stderr).trim()
        ));
    }
    observation.push((
        [b"SUBMODULE_DECLARATIONS\0".as_slice(), relative_prefix].concat(),
        declarations.stdout.clone(),
    ));
    for declaration in declarations.stdout.split(|byte| *byte == 0) {
        if declaration.is_empty() {
            continue;
        }
        let separator = declaration
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| {
                "malformed declared submodule path record: missing key/value separator".to_string()
            })?;
        let path_bytes = &declaration[separator + 1..];
        let path_identity = git_path_from_observation(path_bytes)?;
        let relative_path = join_git_paths(relative_prefix, path_identity.as_bytes())?;
        let submodule_path = repository.join(path_buf_from_git_bytes(path_identity.as_bytes()));
        if !submodule_path.join(".git").exists() {
            continue;
        }

        let status = phoenix_core::git::command()
            .args([
                "status",
                "--porcelain=v1",
                "-z",
                "--ignored",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ])
            .current_dir(&submodule_path)
            .output()
            .map_err(|error| error.to_string())?;
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

        let gitlink = phoenix_core::git::command()
            .args(["submodule", "status", "--"])
            .arg(path_buf_from_git_bytes(path_identity.as_bytes()))
            .current_dir(repository)
            .output()
            .map_err(|error| error.to_string())?;
        if !gitlink.status.success() {
            return Err(format!(
                "cannot inspect initialized submodule gitlink {}: {}",
                String::from_utf8_lossy(&relative_path),
                String::from_utf8_lossy(&gitlink.stderr).trim()
            ));
        }
        observation.push((
            [b"SUBMODULE_GITLINK\0".as_slice(), relative_path.as_slice()].concat(),
            gitlink.stdout.clone(),
        ));
        let detached_loss =
            detached_head_is_unreachable(&submodule_path, observation, &relative_path)?;
        let gitlink_changed = gitlink
            .stdout
            .first()
            .is_some_and(|prefix| matches!(prefix, b'+' | b'U'));
        if gitlink_changed || detached_loss {
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
        observe_initialized_submodules(&submodule_path, &relative_path, observation)?;
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

enum ExactWorktreeRemoval {
    Removed,
    Missing { reason: String },
    Changed,
}

fn inspect_and_remove_exact_worktree(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
) -> Result<ExactWorktreeRemoval, String> {
    let path = worktree_path(identity);
    if !path
        .try_exists()
        .map_err(|error| format!("cannot observe captured worktree path: {error}"))?
    {
        return Ok(ExactWorktreeRemoval::Missing {
            reason: "captured worktree path is absent".to_string(),
        });
    }
    let _repository_lock =
        RepositoryMutationLock::acquire(&path).map_err(|(message, _)| message)?;
    let (fresh_snapshot, _) = runtime.block_on(inspect_worktree(identity))?;
    if &fresh_snapshot != confirmed_snapshot {
        return Ok(ExactWorktreeRemoval::Changed);
    }
    runtime.block_on(remove_exact_worktree(identity))?;
    Ok(ExactWorktreeRemoval::Removed)
}

async fn remove_exact_worktree(identity: &WorktreeIdentity) -> Result<(), String> {
    let path = worktree_path(identity);
    if !path.exists() {
        return Err("captured worktree path is absent without an exact prior receipt".to_string());
    }
    let expected = identity.fingerprint().as_str().to_string();
    let path_for_check = path.clone();
    let (repo, registered, fingerprint_matches) = tokio::task::spawn_blocking(move || {
        let common = phoenix_core::git::command()
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .current_dir(&path_for_check)
            .output()?;
        if !common.status.success() {
            return Ok::<_, std::io::Error>((None, false, false));
        }
        let common = path_buf_from_git_bytes(common.stdout.trim_ascii());
        let repo = if common.join("HEAD").is_file() && !common.join(".git").exists() {
            common.clone()
        } else {
            let Some(repo) = common.parent().map(Path::to_path_buf) else {
                return Ok((None, false, false));
            };
            repo
        };
        let target = std::fs::canonicalize(&path_for_check)?;
        let listed = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain", "-z"])
            .current_dir(&repo)
            .output()?;
        let registered = listed.status.success()
            && listed.stdout.split(|byte| *byte == 0).any(|field| {
                field
                    .strip_prefix(b"worktree ")
                    .map(path_buf_from_git_bytes)
                    .and_then(|candidate| std::fs::canonicalize(candidate).ok())
                    .as_ref()
                    == Some(&target)
            });
        let fingerprint_matches =
            observe_worktree_fingerprint(&path_for_check).as_deref() == Some(expected.as_str());
        Ok((Some(repo), registered, fingerprint_matches))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let Some(repo) = repo else {
        return Err("captured worktree is not server-owned Git worktree".to_string());
    };
    if !registered {
        return Err("captured worktree is no longer Git registered".to_string());
    }
    if !fingerprint_matches {
        return Err("captured worktree administrative incarnation changed".to_string());
    }
    let output = tokio::task::spawn_blocking(move || {
        let status = phoenix_core::git::command()
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .current_dir(&path)
            .output()?;
        if !status.status.success() {
            return Ok::<_, std::io::Error>(Err(String::from_utf8_lossy(&status.stderr)
                .trim()
                .to_string()));
        }
        if !status.stdout.is_empty() {
            return Ok(Err("worktree changed after confirmation".to_string()));
        }
        let removal = phoenix_core::git::command()
            .args(["worktree", "remove"])
            .arg(path)
            .current_dir(repo)
            .output()?;
        Ok(if removal.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&removal.stderr).trim().to_string())
        })
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string());
    output?
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
        canonical_status_observation, git_path_from_observation, inspect_worktree,
        parse_status_losses, remove_exact_worktree, snapshot_for, CloseLeaseFailure,
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
            WorktreeFingerprint::parse("inspection-test-fingerprint").unwrap(),
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

        remove_exact_worktree(&identity).await.unwrap();

        assert!(!linked.exists());
        let listing = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&bare)
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert!(!String::from_utf8_lossy(&listing.stdout).contains(linked.to_str().unwrap()));
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
    async fn detached_commit_held_by_another_worktree_is_not_reported_as_loss() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["checkout", "--quiet", "--detach"]);
        std::fs::write(temp.path().join("tracked"), "detached\n").unwrap();
        run_git(temp.path(), &["commit", "--quiet", "-am", "detached"]);
        let sibling = temp.path().with_file_name("close-inspection-sibling");
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
        assert!(!losses
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
    }

    #[tokio::test]
    async fn initialized_submodule_clean_detached_commit_is_reported_as_loss() {
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

        let (_, losses) = inspect_worktree(&inspection_identity(&closing))
            .await
            .unwrap();
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::InitializedSubmoduleState(path)
                if path.as_bytes() == b"deps/child"
        )));
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
