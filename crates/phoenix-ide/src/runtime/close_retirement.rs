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
    RecordCloseWorktreeCleanupPlanRequest, ReplaceCloseInspectionRequest,
    ReplaceCloseInspectionScopeRequest,
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

struct CompletedCloseResource {
    identity: RetiredResourceIdentity,
    outcome: RetirementOutcome,
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
        let prior_obligation = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let reinspection_generation = (prior_obligation.phase()
            == ClosePhase::AwaitingRetirementInspection
            && prior_obligation.snapshot().is_some())
        .then(|| format!("server_git_status_v2_retry_{}", uuid::Uuid::new_v4()));
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
                    let quarantine = worktree_quarantine_path(&identity)?;
                    match path.try_exists() {
                        Ok(true) => {
                            if observe_worktree_fingerprint(&path).as_deref()
                                != Some(identity.fingerprint().as_str())
                            {
                                self.db()
                                    .route_close_attempt_to_repair(&attempt_id)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                return Err("captured worktree administrative incarnation changed"
                                    .to_string());
                            }
                            let (snapshot, losses) = match inspect_worktree(&identity).await {
                                Ok(inspection) => inspection,
                                Err(error) => {
                                    self.db()
                                        .route_close_attempt_to_repair(&attempt_id)
                                        .await
                                        .map_err(|db_error| db_error.to_string())?;
                                    return Err(error);
                                }
                            };
                            (
                                rotate_inspection_generation(
                                    snapshot,
                                    reinspection_generation.as_deref(),
                                )?,
                                losses,
                            )
                        }
                        Ok(false)
                            if quarantine.try_exists().map_err(|error| {
                                format!("cannot observe quarantined worktree path: {error}")
                            })? =>
                        {
                            if observe_worktree_fingerprint(&quarantine).as_deref()
                                != Some(identity.fingerprint().as_str())
                            {
                                self.db()
                                    .route_close_attempt_to_repair(&attempt_id)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                return Err("captured worktree administrative incarnation changed"
                                    .to_string());
                            }
                            let (snapshot, losses) =
                                match inspect_worktree_at(&identity, quarantine).await {
                                    Ok(inspection) => inspection,
                                    Err(error) => {
                                        self.db()
                                            .route_close_attempt_to_repair(&attempt_id)
                                            .await
                                            .map_err(|db_error| db_error.to_string())?;
                                        return Err(error);
                                    }
                                };
                            (
                                rotate_inspection_generation(
                                    snapshot,
                                    reinspection_generation.as_deref(),
                                )?,
                                losses,
                            )
                        }
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
                                    if continue_clean_retirement {
                                        let active_snapshot = if obligation.phase()
                                            == ClosePhase::AwaitingRetirementInspection
                                        {
                                            self.db()
                                                .resume_close_retirement_after_dispatched_absence(
                                                    &attempt_id,
                                                    &prior_snapshot,
                                                    reinspection_generation.as_deref().ok_or_else(
                                                        || {
                                                            "dispatched absence retry lacks replacement generation"
                                                                .to_string()
                                                        },
                                                    )?,
                                                )
                                                .await
                                                .map_err(|error| error.to_string())?
                                        } else {
                                            prior_snapshot
                                        };
                                        self.retire_close_runtime_resources(attempt_id).await?;
                                        return Ok(active_snapshot);
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
            .replace_close_inspection_with_empty_generation(
                ReplaceCloseInspectionRequest {
                    attempt_id: attempt_id.clone(),
                    scopes: requests,
                },
                reinspection_generation.as_deref(),
            )
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
                    .begin_retirement(&key, None, None)
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
                            .begin_retirement(&key, None, None)
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

    async fn discard_close_resource_leases(&self, attempt_id: &CloseAttemptId) {
        self.close_retirement_leases
            .lock()
            .await
            .retain(|(candidate, _), _| candidate != attempt_id.as_str());
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
    /// The fence stays held in `close_retirement_leases` until completion or entry into repair.
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
        let inventory_is_complete = self
            .db()
            .close_retirement_inventory_is_complete(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        if !inventory_is_complete {
            self.capture_close_retirement_inventory(attempt_id.clone(), snapshot.clone())
                .await?;
        }
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
                            match tmux_retirement_outcome(outcome) {
                                Ok(RetirementOutcome::Retired) => {
                                    self.record_close_retired(
                                        &attempt_id,
                                        &snapshot,
                                        &scope,
                                        resource.clone(),
                                        "exact durable tmux rehydration",
                                    )
                                    .await?;
                                }
                                Ok(RetirementOutcome::AbsenceAdopted { .. }) => {
                                    self.record_close_absence_adopted(
                                        &attempt_id,
                                        &snapshot,
                                        &scope,
                                        resource.clone(),
                                        "exact durable tmux absence after dispatch",
                                    )
                                    .await?;
                                }
                                Ok(RetirementOutcome::Residual { .. }) => unreachable!(),
                                Err((reason, detail)) => {
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
                            }
                        }
                        Ok(TmuxRetirementRehydration::AbsenceVerified) => {
                            self.record_close_absence_adopted(
                                &attempt_id,
                                &snapshot,
                                &scope,
                                resource.clone(),
                                "exact durable tmux absence after dispatch",
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
                .filter(|resource| expected_keys.contains(&resource_key(&resource.identity)))
                .collect::<Vec<_>>();
            let retired_keys = resources
                .iter()
                .map(|resource| resource_key(&resource.identity))
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
                match resource.outcome {
                    RetirementOutcome::Retired => {
                        self.record_close_retired(
                            &attempt_id,
                            &snapshot,
                            &scope,
                            resource.identity,
                            "exact registry permit retirement",
                        )
                        .await?;
                    }
                    RetirementOutcome::AbsenceAdopted { .. } => {
                        self.record_close_absence_adopted(
                            &attempt_id,
                            &snapshot,
                            &scope,
                            resource.identity,
                            "exact registry permit absence after dispatch",
                        )
                        .await?;
                    }
                    RetirementOutcome::Residual { .. } => unreachable!(),
                }
            }
        }
        self.retire_close_worktrees_and_scopes(&attempt_id, &snapshot)
            .await?;
        self.db()
            .complete_close_retirement(&attempt_id)
            .await
            .map_err(|error| error.to_string())?;
        self.discard_close_resource_leases(&attempt_id).await;
        for captured in self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
        {
            self.broadcast_work_scope_update(&ResourceScopeKey::Work(captured.scope))
                .await;
        }
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
            let (fresh_snapshot, _) = match inspect_worktree(identity).await {
                Ok(inspection) => inspection,
                Err(reason) => {
                    return self
                        .record_close_residual(
                            attempt_id,
                            snapshot,
                            &captured.scope,
                            target.resource.clone(),
                            RetirementFailureReason::IdentityNotProven,
                            &format!(
                                "worktree cannot be reinspected before live resource retirement: {reason}"
                            ),
                        )
                        .await;
                }
            };
            if fresh_snapshot.fingerprint() != confirmed.snapshot.fingerprint() {
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
                    let captured_path = worktree_path(identity);
                    let quarantine_path = worktree_quarantine_path(identity)?;
                    let worktree_absent =
                        match both_worktree_paths_absent(&captured_path, &quarantine_path) {
                            Ok(absent) => absent,
                            Err(detail) => {
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
                        };
                    if worktree_absent {
                        let dispatched = self
                            .db()
                            .close_retirement_resource_was_dispatched(
                                attempt_id,
                                &scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        let planned_administrative_dir = self
                            .db()
                            .close_worktree_cleanup_plan(
                                attempt_id,
                                &scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        let Some(cleanup_plan) = planned_administrative_dir else {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    "absent worktree lacks an exact durable cleanup plan",
                                )
                                .await;
                        };
                        if !dispatched {
                            return self
                                .record_close_residual(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    "absent worktree lacks an exact durable retirement dispatch",
                                )
                                .await;
                        }
                        let identity = identity.clone();
                        let recovery = tokio::task::spawn_blocking(move || {
                            complete_persisted_worktree_administrative_cleanup(
                                &identity,
                                &cleanup_plan.administrative_dir,
                                &cleanup_plan.administrative_dir_incarnation,
                            )
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                        if let Err(detail) = recovery {
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
                        self.record_close_absence_adopted(
                            attempt_id,
                            snapshot,
                            &scope,
                            target.resource.clone(),
                            "validated exact persisted worktree cleanup plan; completed only its administrative-directory deletion",
                        )
                        .await?;
                    } else {
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
                        let cleanup_plan = if let Some(plan) = self
                            .db()
                            .close_worktree_cleanup_plan(
                                attempt_id,
                                &scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?
                        {
                            plan
                        } else {
                            let identity = identity.clone();
                            let discovered = tokio::task::spawn_blocking(move || {
                                let path = worktree_path(&identity);
                                let quarantine = worktree_quarantine_path(&identity)?;
                                let inspection_path =
                                    if path.exists() { &path } else { &quarantine };
                                let common = exact_worktree_common_git_dir(inspection_path)?;
                                let administrative_dir =
                                    exact_worktree_administrative_dir(inspection_path, &common)?;
                                let administrative_dir_incarnation =
                                    observe_administrative_dir_incarnation(&administrative_dir)?;
                                Ok::<_, String>((
                                    administrative_dir,
                                    administrative_dir_incarnation,
                                ))
                            })
                            .await
                            .map_err(|error| error.to_string())??;
                            self.db()
                                .record_close_worktree_cleanup_plan(
                                    RecordCloseWorktreeCleanupPlanRequest {
                                        attempt_id: attempt_id.clone(),
                                        scope: scope.clone(),
                                        snapshot: snapshot.clone(),
                                        resource: target.resource.clone(),
                                        administrative_dir: discovered.0.clone(),
                                        administrative_dir_incarnation: discovered.1.clone(),
                                    },
                                )
                                .await
                                .map_err(|error| error.to_string())?;
                            crate::db::CloseWorktreeCleanupPlan {
                                administrative_dir: discovered.0,
                                administrative_dir_incarnation: discovered.1,
                            }
                        };
                        let identity = identity.clone();
                        let confirmed_snapshot = confirmed.snapshot.clone();
                        let runtime = tokio::runtime::Handle::current();
                        let final_removal = tokio::task::spawn_blocking(move || {
                            inspect_and_remove_exact_worktree(
                                &runtime,
                                &identity,
                                &confirmed_snapshot,
                                &cleanup_plan.administrative_dir,
                                &cleanup_plan.administrative_dir_incarnation,
                            )
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                        let fresh_snapshot: Option<CloseRetirementSnapshot> = match final_removal {
                            Ok(ExactWorktreeRemoval::Retired) => {
                                self.record_close_retired(
                                    attempt_id,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    "exact captured Git worktree removal",
                                )
                                .await?;
                                None
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
                            if fresh_snapshot.fingerprint() != confirmed.snapshot.fingerprint() {
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

    async fn record_close_absence_adopted(
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
                outcome: RetirementOutcome::AbsenceAdopted {
                    absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
                },
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
        self.cancel_close_resource_leases(attempt_id).await;
        Err(detail.to_string())
    }

    /// Completes one live lease. Callers must persist exactly one receipt per
    /// returned identity; the repair transition reopens admission after a residual.
    async fn complete_close_resource_lease(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
    ) -> Result<Vec<CompletedCloseResource>, CloseLeaseFailure> {
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
            let tmux_outcome = tmux_retirement_outcome(tmux_outcome)
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
            Ok(lease
                .resources
                .iter()
                .cloned()
                .map(|identity| CompletedCloseResource {
                    outcome: if identity.kind() == RetiredResourceKind::TmuxServer {
                        tmux_outcome.clone()
                    } else {
                        RetirementOutcome::Retired
                    },
                    identity,
                })
                .collect())
        }
        .await;
        self.close_retirement_leases.lock().await.insert(key, lease);
        result
    }
}

async fn inspect_worktree(
    identity: &WorktreeIdentity,
) -> Result<(CloseRetirementSnapshot, Vec<CloseLossItem>), String> {
    inspect_worktree_at(identity, worktree_path(identity)).await
}

async fn inspect_worktree_at(
    identity: &WorktreeIdentity,
    path: PathBuf,
) -> Result<(CloseRetirementSnapshot, Vec<CloseLossItem>), String> {
    if observe_worktree_fingerprint(&path).as_deref() != Some(identity.fingerprint().as_str()) {
        return Err("captured worktree administrative incarnation changed".to_string());
    }
    let status_path = path.clone();
    let output = tokio::task::spawn_blocking(move || run_bounded_git_status(&status_path))
        .await
        .map_err(|error| error.to_string())??;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut observation = canonical_status_observation(&output.stdout);
    let mut losses = parse_status_losses(&output.stdout);
    let content_path = path.clone();
    let content_losses = losses.clone();
    let content_observation =
        tokio::task::spawn_blocking(move || observe_dirty_content(&content_path, &content_losses))
            .await
            .map_err(|error| error.to_string())??;
    observation.extend_from_slice(&content_observation);
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
    let stdout = child
        .stdout
        .take()
        .ok_or("Git status stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Git status stderr was not piped")?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::BufReader::new(stdout), &mut bytes).map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::BufReader::new(stderr), &mut bytes).map(|_| bytes)
    });
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
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
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("Git status inspection exceeded its deadline".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "Git status stdout reader panicked".to_string())?
        .map_err(|error| error.to_string())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "Git status stderr reader panicked".to_string())?
        .map_err(|error| error.to_string())?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
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
        let submodule_losses = parse_status_losses(&status.stdout);
        let mut submodule_status = canonical_status_observation(&status.stdout);
        submodule_status
            .extend_from_slice(&observe_dirty_content(&submodule_path, &submodule_losses)?);
        observation.push((
            [b"SUBMODULE_STATUS\0".as_slice(), relative_path.as_slice()].concat(),
            submodule_status,
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
    Retired,
    ReinspectionRequired { detail: String },
    Residual { detail: String },
}

fn inspect_and_remove_exact_worktree(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
) -> Result<ExactWorktreeRemoval, String> {
    inspect_and_remove_exact_worktree_with_hook_and_plan(
        runtime,
        identity,
        confirmed_snapshot,
        administrative_dir,
        administrative_dir_incarnation,
        |_| {},
    )
}

#[cfg(test)]
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
    let inspection_path = if path.exists() { &path } else { &quarantine };
    let common = exact_worktree_common_git_dir(inspection_path)?;
    let administrative_dir = exact_worktree_administrative_dir(inspection_path, &common)?;
    let administrative_dir_incarnation =
        observe_administrative_dir_incarnation(&administrative_dir)?;
    inspect_and_remove_exact_worktree_with_hook_and_plan(
        runtime,
        identity,
        confirmed_snapshot,
        &administrative_dir,
        &administrative_dir_incarnation,
        after_quarantine,
    )
}

fn inspect_and_remove_exact_worktree_with_hook_and_plan<F>(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
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
        return Ok(ExactWorktreeRemoval::Residual {
            detail: format!(
                "captured worktree and quarantine are absent; refusing to delete unverified Git registration {}",
                administrative_dir.display()
            ),
        });
    }
    let _repository_lock =
        RepositoryMutationLock::acquire(if path.exists() { &path } else { &quarantine })
            .map_err(|(message, _)| message)?;
    let inspection_path = if path.exists() { &path } else { &quarantine };
    let (fresh_snapshot, _) =
        runtime.block_on(inspect_worktree_at(identity, inspection_path.clone()))?;
    if fresh_snapshot.fingerprint() != confirmed_snapshot.fingerprint() {
        return Ok(ExactWorktreeRemoval::ReinspectionRequired {
            detail: "worktree changed after Close inspection confirmation; fresh confirmation is required"
                .to_string(),
        });
    }
    runtime.block_on(quarantine_and_remove_exact_worktree(
        identity,
        administrative_dir.to_path_buf(),
        administrative_dir_incarnation.to_string(),
        after_quarantine,
    ))
}

fn path_is_within(candidate: &Path, directory: &Path) -> bool {
    candidate == directory || candidate.starts_with(directory)
}

fn exact_worktree_common_git_dir(worktree: &Path) -> Result<PathBuf, String> {
    let output = phoenix_core::git::command()
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(worktree)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("captured worktree is not server-owned Git worktree".to_string());
    }
    Ok(path_buf_from_git_bytes(output.stdout.trim_ascii()))
}

fn observe_administrative_dir_incarnation(administrative_dir: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(administrative_dir).map_err(|error| {
        format!(
            "cannot observe worktree administrative-directory incarnation {}: {error}",
            administrative_dir.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err("worktree administrative registration is not a directory".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(format!(
            "git_admin_dir_v1:{}:{}",
            metadata.dev(),
            metadata.ino()
        ))
    }
    #[cfg(not(unix))]
    {
        let canonical =
            std::fs::canonicalize(administrative_dir).map_err(|error| error.to_string())?;
        Ok(format!("git_admin_dir_v1:portable:{}", canonical.display()))
    }
}

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: caller treats the platform libc errno pointer according to libc's contract.
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: caller treats the platform libc errno pointer according to libc's contract.
    unsafe { libc::__error() }
}

#[cfg(unix)]
fn remove_directory_contents_at(directory: &std::os::fd::OwnedFd) -> Result<(), String> {
    use std::ffi::CStr;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

    // SAFETY: dup returns a new owned descriptor or -1. fdopendir consumes only
    // that duplicate, while the caller retains the descriptor used by openat.
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(format!(
            "cannot duplicate deletion descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: duplicate is a valid owned directory descriptor on success above.
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        // SAFETY: fdopendir did not consume the descriptor when it returned null.
        unsafe { libc::close(duplicate) };
        return Err(format!(
            "cannot enumerate deletion descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }

    let result = (|| {
        loop {
            // SAFETY: stream remains valid until closed below. errno is reset so
            // a null result can distinguish end-of-directory from an error.
            unsafe { *errno_location() = 0 };
            // SAFETY: stream is a valid DIR pointer owned by this function.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                return if error.raw_os_error() == Some(0) {
                    Ok(())
                } else {
                    Err(format!("cannot read deletion descriptor: {error}"))
                };
            }
            // SAFETY: d_name is NUL-terminated for the lifetime of this entry.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: descriptors and C string are valid; metadata points to writable storage.
            if unsafe {
                libc::fstatat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } < 0
            {
                return Err(format!(
                    "cannot inspect deletion entry: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: fstatat initialized metadata on success.
            let metadata = unsafe { metadata.assume_init() };
            if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
                // SAFETY: openat does not follow the entry because O_NOFOLLOW is set.
                let child = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if child < 0 {
                    return Err(format!(
                        "cannot open deletion subdirectory without following replacements: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                // SAFETY: child is a newly owned descriptor on success above.
                let child = unsafe { OwnedFd::from_raw_fd(child) };
                let mut opened = std::mem::MaybeUninit::<libc::stat>::uninit();
                // SAFETY: child is valid and opened points to writable storage.
                if unsafe { libc::fstat(child.as_raw_fd(), opened.as_mut_ptr()) } < 0 {
                    return Err(format!(
                        "cannot identify opened deletion subdirectory: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                // SAFETY: fstat initialized opened on success.
                let opened = unsafe { opened.assume_init() };
                if opened.st_dev != metadata.st_dev || opened.st_ino != metadata.st_ino {
                    return Err(
                        "deletion subdirectory was replaced before descriptor binding".to_string(),
                    );
                }
                remove_directory_contents_at(&child)?;
                // SAFETY: unlinkat is descriptor-relative and name is valid.
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } < 0
                {
                    return Err(format!(
                        "cannot remove deletion subdirectory: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            } else {
                // SAFETY: unlinkat is descriptor-relative and never follows the entry.
                if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                    return Err(format!(
                        "cannot remove deletion entry: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
        }
    })();
    // SAFETY: stream is the valid DIR pointer returned by fdopendir.
    unsafe { libc::closedir(stream) };
    result
}

fn remove_identity_bound_directory<F, O>(
    deletion_target: &Path,
    expected_identity: &str,
    observe_identity: O,
    before_final_move: F,
    description: &str,
) -> Result<(), String>
where
    F: FnOnce(&Path),
    O: Fn(&Path) -> Result<String, String>,
{
    before_final_move(deletion_target);
    let parent = deletion_target
        .parent()
        .ok_or_else(|| format!("{description} has no parent directory"))?;
    let tombstone_root = parent.join(format!(".phoenix-delete-{}", uuid::Uuid::new_v4().simple()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder
            .mode(0o700)
            .create(&tombstone_root)
            .map_err(|error| format!("cannot reserve private {description} tombstone: {error}"))?;
    }
    #[cfg(not(unix))]
    {
        return Err(format!(
            "identity-bound {description} deletion is unsupported on this platform"
        ));
    }
    let tombstone = tombstone_root.join("object");
    if let Err(error) = std::fs::rename(deletion_target, &tombstone) {
        let _ = std::fs::remove_dir(&tombstone_root);
        return Err(format!(
            "cannot move {description} into private final tombstone: {error}"
        ));
    }
    if observe_identity(&tombstone)? != expected_identity {
        return Err(format!(
            "{description} identity changed before final deletion; replacement preserved at {}",
            tombstone.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::FromRawFd as _;
        use std::os::unix::ffi::OsStrExt as _;

        let tombstone_name = CString::new("object").expect("static name contains no NUL");
        let root = CString::new(tombstone_root.as_os_str().as_bytes())
            .map_err(|_| format!("private {description} tombstone path contains NUL"))?;
        // SAFETY: root is a valid C path; O_NOFOLLOW rejects a replaced symlink.
        let root_descriptor = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if root_descriptor < 0 {
            return Err(format!(
                "cannot open private {description} tombstone without following replacements: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: root_descriptor is newly owned on success above.
        let root_descriptor = unsafe { std::os::fd::OwnedFd::from_raw_fd(root_descriptor) };
        // SAFETY: openat is rooted in the private descriptor and O_NOFOLLOW rejects replacement links.
        let object_descriptor = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&root_descriptor),
                tombstone_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if object_descriptor < 0 {
            return Err(format!(
                "cannot open identity-bound {description}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: object_descriptor is newly owned on success above.
        let object_descriptor = unsafe { std::os::fd::OwnedFd::from_raw_fd(object_descriptor) };
        remove_directory_contents_at(&object_descriptor)?;
        // SAFETY: unlinkat is rooted at the still-open private directory and does not follow names.
        if unsafe {
            libc::unlinkat(
                std::os::fd::AsRawFd::as_raw_fd(&root_descriptor),
                tombstone_name.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        } < 0
        {
            return Err(format!(
                "cannot unlink identity-bound {description}: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    std::fs::remove_dir(&tombstone_root)
        .map_err(|error| format!("cannot remove empty {description} tombstone: {error}"))
}

fn administrative_dir_quarantine_path(
    administrative_dir: &Path,
    incarnation: &str,
) -> Result<PathBuf, String> {
    let parent = administrative_dir
        .parent()
        .ok_or_else(|| "worktree administrative directory has no parent".to_string())?;
    let digest = Sha256::digest(incarnation.as_bytes());
    let mut suffix = String::with_capacity(16);
    for byte in &digest[..8] {
        write!(&mut suffix, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(parent.join(format!(".phoenix-close-admin-{suffix}")))
}

fn remove_exact_worktree_administrative_dir(
    administrative_dir: &Path,
    expected_incarnation: &str,
) -> Result<(), String> {
    remove_exact_worktree_administrative_dir_with_hook(
        administrative_dir,
        expected_incarnation,
        |_| {},
    )
}

fn remove_exact_worktree_administrative_dir_with_hook<F>(
    administrative_dir: &Path,
    expected_incarnation: &str,
    before_final_move: F,
) -> Result<(), String>
where
    F: FnOnce(&Path),
{
    let quarantine = administrative_dir_quarantine_path(administrative_dir, expected_incarnation)?;
    let source_exists = administrative_dir
        .try_exists()
        .map_err(|error| error.to_string())?;
    let quarantine_exists = quarantine.try_exists().map_err(|error| error.to_string())?;
    let deletion_target = match (source_exists, quarantine_exists) {
        (false, false) => return Ok(()),
        (true, true) => {
            return Err(
                "worktree administrative cleanup has both live and quarantined registrations"
                    .to_string(),
            );
        }
        (false, true) => quarantine,
        (true, false) => {
            if observe_administrative_dir_incarnation(administrative_dir)? != expected_incarnation {
                return Err("worktree administrative-directory incarnation changed".to_string());
            }
            std::fs::rename(administrative_dir, &quarantine).map_err(|error| {
                format!("cannot quarantine exact worktree administrative directory: {error}")
            })?;
            quarantine
        }
    };
    if observe_administrative_dir_incarnation(&deletion_target)? != expected_incarnation {
        return Err(
            "quarantined worktree administrative-directory incarnation changed".to_string(),
        );
    }
    remove_identity_bound_directory(
        &deletion_target,
        expected_incarnation,
        observe_administrative_dir_incarnation,
        before_final_move,
        "retired worktree administrative directory",
    )
}

fn exact_worktree_administrative_dir(
    worktree: &Path,
    common_git_dir: &Path,
) -> Result<PathBuf, String> {
    let git_file = std::fs::read(worktree.join(".git"))
        .map_err(|error| format!("cannot read exact worktree administrative link: {error}"))?;
    let git_dir = git_file
        .strip_prefix(b"gitdir: ")
        .and_then(|value| value.strip_suffix(b"\n").or(Some(value)))
        .map(path_buf_from_git_bytes)
        .ok_or_else(|| "exact worktree administrative link is malformed".to_string())?;
    let git_dir = std::fs::canonicalize(git_dir)
        .map_err(|error| format!("cannot resolve exact worktree administrative link: {error}"))?;
    let worktrees_dir = std::fs::canonicalize(common_git_dir.join("worktrees"))
        .map_err(|error| format!("cannot resolve repository worktree registrations: {error}"))?;
    if git_dir.parent() != Some(worktrees_dir.as_path()) {
        return Err("exact worktree administrative link escapes repository worktrees".to_string());
    }
    Ok(git_dir)
}

fn quarantine_has_external_writer(path: &Path) -> Result<bool, String> {
    Ok(quarantine_has_open_descriptors(path)?
        || quarantine_has_process_cwd(path)?
        || quarantine_has_writable_mappings(path)?)
}

#[cfg(target_os = "linux")]
fn quarantine_has_writable_mappings(path: &Path) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantine before mapping inspection: {error}")
    })?;
    for process in std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate process mappings: {error}"))?
        .flatten()
    {
        if !process
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let mappings = match std::fs::read_to_string(process.path().join("maps")) {
            Ok(mappings) => mappings,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue
            }
            Err(error) => return Err(format!("cannot inspect process mappings: {error}")),
        };
        for mapping in mappings.lines() {
            let mut fields = mapping
                .splitn(6, char::is_whitespace)
                .filter(|field| !field.is_empty());
            let _address = fields.next();
            let permissions = fields.next().unwrap_or_default();
            let _offset = fields.next();
            let _device = fields.next();
            let _inode = fields.next();
            let mapped_path = fields.next().unwrap_or_default().trim_start();
            if permissions.as_bytes().get(1) == Some(&b'w')
                && permissions.as_bytes().get(3) == Some(&b's')
                && path_is_within(Path::new(mapped_path), &canonical)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn quarantine_has_writable_mappings(path: &Path) -> Result<bool, String> {
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};
    use std::os::unix::ffi::OsStrExt as _;

    #[repr(C)]
    struct ProcRegionInfo {
        protection: u32,
        max_protection: u32,
        inheritance: u32,
        flags: u32,
        offset: u64,
        behavior: u32,
        user_wired_count: u32,
        user_tag: u32,
        pages_resident: u32,
        pages_shared_now_private: u32,
        pages_swapped_out: u32,
        pages_dirtied: u32,
        ref_count: u32,
        shadow_depth: u32,
        share_mode: u32,
        private_pages_resident: u32,
        shared_pages_resident: u32,
        object_id: u32,
        depth: u32,
        address: u64,
        size: u64,
    }

    #[repr(C)]
    struct ProcRegionWithPathInfo {
        region: ProcRegionInfo,
        vnode: libc::vnode_info_path,
    }

    const PROC_PIDREGIONPATHINFO: i32 = 8;
    const SM_SHARED: u32 = 4;
    const SM_TRUESHARED: u32 = 5;
    const SM_SHARED_ALIASED: u32 = 7;
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantine before mapping inspection: {error}")
    })?;
    for pid in macos_all_pids()?.into_iter().filter(|pid| *pid > 0) {
        let mut address = 0_u64;
        loop {
            let mut info = MaybeUninit::<ProcRegionWithPathInfo>::zeroed();
            let bytes = unsafe {
                libc::proc_pidinfo(
                    pid,
                    PROC_PIDREGIONPATHINFO,
                    address,
                    info.as_mut_ptr().cast(),
                    i32::try_from(size_of::<ProcRegionWithPathInfo>())
                        .expect("region path info fits i32"),
                )
            };
            if bytes == 0 {
                break;
            }
            if bytes
                != i32::try_from(size_of::<ProcRegionWithPathInfo>())
                    .expect("region path info size fits i32")
            {
                return Err(format!("cannot inspect process {pid} memory mappings"));
            }
            let info = unsafe { info.assume_init() };
            let next = info
                .region
                .address
                .checked_add(info.region.size)
                .ok_or_else(|| format!("process {pid} mapping address overflowed"))?;
            if next <= address {
                return Err(format!("process {pid} mapping inventory did not advance"));
            }
            address = next;
            let path_bytes = info.vnode.vip_path.as_flattened();
            let mapped_path = unsafe { CStr::from_ptr(path_bytes.as_ptr()) };
            if info.region.protection & u32::try_from(libc::VM_PROT_WRITE).unwrap() != 0
                && matches!(
                    info.region.share_mode,
                    SM_SHARED | SM_TRUESHARED | SM_SHARED_ALIASED
                )
                && path_is_within(
                    Path::new(std::ffi::OsStr::from_bytes(mapped_path.to_bytes())),
                    &canonical,
                )
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_writable_mappings(_path: &Path) -> Result<bool, String> {
    Err("writable memory-mapping inspection is unsupported on this platform".to_string())
}

#[cfg(target_os = "linux")]
fn quarantine_has_process_cwd(path: &Path) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantine before cwd inspection: {error}")
    })?;
    for process in std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate process working directories: {error}"))?
    {
        let process = process.map_err(|error| format!("cannot inspect process entry: {error}"))?;
        if !process
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        match std::fs::read_link(process.path().join("cwd")) {
            Ok(cwd) if path_is_within(&cwd, &canonical) => return Ok(true),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(error) => return Err(format!("cannot inspect process working directory: {error}")),
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn macos_all_pids() -> Result<Vec<i32>, String> {
    use std::mem::size_of;

    let mut capacity = 4096_usize;
    loop {
        let mut pids = vec![0_i32; capacity];
        let capacity_bytes = pids
            .len()
            .checked_mul(size_of::<i32>())
            .ok_or_else(|| "process inventory size overflowed".to_string())?;
        let pid_bytes = unsafe {
            libc::proc_listpids(
                1,
                0,
                pids.as_mut_ptr().cast(),
                i32::try_from(capacity_bytes)
                    .map_err(|_| "process inventory exceeds macOS API limit")?,
            )
        };
        if pid_bytes < 0 {
            return Err("cannot enumerate processes".to_string());
        }
        let pid_bytes = usize::try_from(pid_bytes).expect("nonnegative PID bytes");
        if pid_bytes < capacity_bytes {
            pids.truncate(pid_bytes / size_of::<i32>());
            return Ok(pids);
        }
        capacity = capacity
            .checked_mul(2)
            .ok_or_else(|| "process inventory size overflowed".to_string())?;
    }
}

#[cfg(target_os = "macos")]
fn quarantine_has_process_cwd(path: &Path) -> Result<bool, String> {
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};
    use std::os::unix::ffi::OsStrExt as _;

    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantine before cwd inspection: {error}")
    })?;
    let pids = macos_all_pids()?;
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
        let bytes = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr().cast(),
                i32::try_from(size_of::<libc::proc_vnodepathinfo>())
                    .expect("vnode path info fits i32"),
            )
        };
        if bytes
            != i32::try_from(size_of::<libc::proc_vnodepathinfo>())
                .expect("vnode path info size fits i32")
        {
            continue;
        }
        let info = unsafe { info.assume_init() };
        let cwd_bytes = info.pvi_cdir.vip_path.as_flattened();
        let cwd = unsafe { CStr::from_ptr(cwd_bytes.as_ptr()) };
        if path_is_within(
            Path::new(std::ffi::OsStr::from_bytes(cwd.to_bytes())),
            &canonical,
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_process_cwd(_path: &Path) -> Result<bool, String> {
    Err("process working-directory inspection is unsupported on this platform".to_string())
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors(path: &Path) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantined worktree before descriptor inspection: {error}")
    })?;
    let processes = std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate process descriptors: {error}"))?;
    for process in processes {
        let process = process.map_err(|error| format!("cannot inspect process entry: {error}"))?;
        if !process
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let descriptors = match std::fs::read_dir(process.path().join("fd")) {
            Ok(descriptors) => descriptors,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot inspect process {} descriptor table: {error}",
                    process.file_name().to_string_lossy()
                ));
            }
        };
        for descriptor in descriptors {
            let descriptor = descriptor
                .map_err(|error| format!("cannot inspect process descriptor entry: {error}"))?;
            match std::fs::read_link(descriptor.path()) {
                Ok(candidate) if path_is_within(&candidate, &canonical) => return Ok(true),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("cannot inspect process descriptor target: {error}"));
                }
            }
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn descriptor_inventory_may_be_truncated(returned_bytes: usize, capacity_bytes: usize) -> bool {
    returned_bytes >= capacity_bytes
}

#[allow(clippy::too_many_lines)]
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

    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantined worktree before descriptor inspection: {error}")
    })?;
    let pids = macos_all_pids()?;
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        let mut descriptor_capacity = 256_usize;
        let descriptors = loop {
            let mut descriptors = vec![
                libc::proc_fdinfo {
                    proc_fd: 0,
                    proc_fdtype: 0
                };
                descriptor_capacity
            ];
            let capacity_bytes = descriptors.len() * size_of::<libc::proc_fdinfo>();
            // SAFETY: the vector provides writable storage for exactly the byte count passed.
            let descriptor_bytes = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    descriptors.as_mut_ptr().cast(),
                    i32::try_from(capacity_bytes)
                        .map_err(|_| "process descriptor inventory exceeds macOS API limit")?,
                )
            };
            if descriptor_bytes <= 0 {
                break Vec::new();
            }
            let descriptor_bytes =
                usize::try_from(descriptor_bytes).expect("positive descriptor byte count");
            if !descriptor_inventory_may_be_truncated(descriptor_bytes, capacity_bytes) {
                descriptors.truncate(descriptor_bytes / size_of::<libc::proc_fdinfo>());
                break descriptors;
            }
            descriptor_capacity = descriptor_capacity
                .checked_mul(2)
                .ok_or_else(|| "process descriptor inventory size overflowed".to_string())?;
        };
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

fn both_worktree_paths_absent(path: &Path, quarantine: &Path) -> Result<bool, String> {
    let path_exists = path.try_exists().map_err(|error| {
        format!("cannot observe captured worktree path before retirement: {error}")
    })?;
    let quarantine_exists = quarantine.try_exists().map_err(|error| {
        format!("cannot observe quarantined worktree path before retirement: {error}")
    })?;
    Ok(!path_exists && !quarantine_exists)
}

fn planned_administrative_dir_is_absent(path: &Path) -> Result<bool, String> {
    path.try_exists().map(|exists| !exists).map_err(|error| {
        format!("cannot observe planned worktree administrative directory: {error}")
    })
}

fn complete_persisted_worktree_administrative_cleanup(
    identity: &WorktreeIdentity,
    planned_administrative_dir: &Path,
    planned_administrative_dir_incarnation: &str,
) -> Result<(), String> {
    validate_cleanup_plan_registration_incarnation(identity, planned_administrative_dir)?;
    let quarantine = administrative_dir_quarantine_path(
        planned_administrative_dir,
        planned_administrative_dir_incarnation,
    )?;
    if !planned_administrative_dir_is_absent(planned_administrative_dir)? {
        if observe_administrative_dir_incarnation(planned_administrative_dir)?
            != planned_administrative_dir_incarnation
        {
            return Err(
                "persisted cleanup plan administrative-directory incarnation changed".to_string(),
            );
        }
        validate_live_persisted_worktree_registration(identity, planned_administrative_dir)?;
    } else if planned_administrative_dir_is_absent(&quarantine)? {
        return Ok(());
    }
    remove_exact_worktree_administrative_dir(
        planned_administrative_dir,
        planned_administrative_dir_incarnation,
    )
}

fn validate_cleanup_plan_registration_incarnation(
    identity: &WorktreeIdentity,
    planned_administrative_dir: &Path,
) -> Result<(), String> {
    let encoded_pointer = identity
        .fingerprint()
        .as_str()
        .rsplit_once(':')
        .filter(|(prefix, _)| prefix.starts_with("git_admin_incarnation_v2:"))
        .map(|(_, encoded)| encoded)
        .ok_or_else(|| "captured worktree registration incarnation is not decodable".to_string())?;
    let pointer = decode_hex_bytes(encoded_pointer)
        .ok_or_else(|| "captured worktree registration incarnation is malformed".to_string())?;
    if path_buf_from_git_bytes(&pointer) != planned_administrative_dir {
        return Err(
            "persisted cleanup plan does not match the captured worktree registration incarnation"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_live_persisted_worktree_registration(
    identity: &WorktreeIdentity,
    planned_administrative_dir: &Path,
) -> Result<(), String> {
    let backlink = std::fs::read(planned_administrative_dir.join("gitdir"))
        .map_err(|error| format!("cannot validate persisted worktree registration: {error}"))?;
    let backlink = backlink
        .strip_suffix(b"\r\n")
        .or_else(|| backlink.strip_suffix(b"\n"))
        .unwrap_or(&backlink);
    let captured_path = worktree_path(identity);
    let captured_parent = captured_path
        .parent()
        .ok_or_else(|| "captured worktree has no parent directory".to_string())?;
    let expected_git_file = std::fs::canonicalize(captured_parent)
        .map_err(|error| format!("cannot validate captured worktree parent: {error}"))?
        .join(
            captured_path
                .file_name()
                .ok_or_else(|| "captured worktree has no final path component".to_string())?,
        )
        .join(".git");
    if path_buf_from_git_bytes(backlink) != expected_git_file {
        return Err(
            "persisted cleanup plan registration does not point to the captured worktree"
                .to_string(),
        );
    }
    Ok(())
}

fn decode_hex_bytes(encoded: &str) -> Option<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from((high << 4) | low).ok()
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
async fn quarantine_and_remove_exact_worktree<F>(
    identity: &WorktreeIdentity,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
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
        let inspection_path = if resuming_quarantine {
            &quarantine
        } else {
            &path
        };
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
        if quarantine_has_external_writer(&quarantine)? {
            return Ok(ExactWorktreeRemoval::Residual {
                detail: format!(
                    "an external process can still write the confirmed worktree; retained at {}",
                    quarantine.display()
                ),
            });
        }
        let administrative_dir = exact_worktree_administrative_dir(&quarantine, &common)?;
        if administrative_dir != planned_administrative_dir {
            return Err(
                "exact worktree administrative directory differs from durable cleanup plan"
                    .to_string(),
            );
        }
        remove_quarantine_then_administrative_dir(
            &quarantine,
            &administrative_dir,
            &planned_administrative_dir_incarnation,
            || {},
        )?;
        Ok(ExactWorktreeRemoval::Retired)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn remove_quarantine_then_administrative_dir<F>(
    quarantine: &Path,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    after_quarantine_removal: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    remove_quarantine_then_administrative_dir_with_hooks(
        quarantine,
        administrative_dir,
        administrative_dir_incarnation,
        |_| {},
        after_quarantine_removal,
    )
}

fn remove_quarantine_then_administrative_dir_with_hooks<B, A>(
    quarantine: &Path,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    before_final_move: B,
    after_quarantine_removal: A,
) -> Result<(), String>
where
    B: FnOnce(&Path),
    A: FnOnce(),
{
    let expected = observe_worktree_fingerprint(quarantine).ok_or_else(|| {
        "cannot observe quarantined worktree identity before deletion".to_string()
    })?;
    remove_identity_bound_directory(
        quarantine,
        &expected,
        |path| {
            observe_worktree_fingerprint(path).ok_or_else(|| {
                "cannot observe worktree identity in private final tombstone".to_string()
            })
        },
        before_final_move,
        "quarantined worktree",
    )?;
    after_quarantine_removal();
    remove_exact_worktree_administrative_dir(administrative_dir, administrative_dir_incarnation)
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

fn observe_dirty_content(repository: &Path, losses: &[CloseLossItem]) -> Result<Vec<u8>, String> {
    let mut observation = Vec::new();
    let mut paths = losses
        .iter()
        .filter_map(|loss| match loss.identity() {
            LossItemIdentity::GitPath(path) => Some(path.as_bytes().to_vec()),
            LossItemIdentity::GitOid(_)
            | LossItemIdentity::Opaque(_)
            | LossItemIdentity::Worktree(_) => None,
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let staged_entries = staged_index_entries_for_paths(repository, &paths)?;
    for path in paths {
        observation.extend_from_slice(b"CONTENT\0");
        observation.extend_from_slice(&path);
        observation.push(0);
        let filesystem_path = repository.join(path_buf_from_git_bytes(&path));
        match std::fs::symlink_metadata(&filesystem_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                observation.extend_from_slice(b"SYMLINK\0");
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt as _;
                    observation.extend_from_slice(
                        std::fs::read_link(&filesystem_path)
                            .map_err(|error| format!("cannot read dirty symlink: {error}"))?
                            .as_os_str()
                            .as_bytes(),
                    );
                }
                #[cfg(not(unix))]
                observation.extend_from_slice(
                    std::fs::read_link(&filesystem_path)
                        .map_err(|error| format!("cannot read dirty symlink: {error}"))?
                        .to_string_lossy()
                        .as_bytes(),
                );
            }
            Ok(metadata) if metadata.is_file() => {
                observation.extend_from_slice(b"FILE_SHA256\0");
                let mut file = std::fs::File::open(&filesystem_path)
                    .map_err(|error| format!("cannot open dirty file contents: {error}"))?;
                let mut digest = Sha256::new();
                let mut buffer = vec![0_u8; 64 * 1024];
                loop {
                    use std::io::Read as _;
                    let read = file
                        .read(&mut buffer)
                        .map_err(|error| format!("cannot hash dirty file contents: {error}"))?;
                    if read == 0 {
                        break;
                    }
                    digest.update(&buffer[..read]);
                }
                observation.extend_from_slice(&digest.finalize());
            }
            Ok(_) => observation.extend_from_slice(b"OTHER"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observation.extend_from_slice(b"ABSENT");
            }
            Err(error) => return Err(format!("cannot inspect dirty path contents: {error}")),
        }
        observation.push(0);
        observation.extend_from_slice(b"INDEX\0");
        observation.extend_from_slice(&path);
        observation.push(0);
        if let Some(entries) = staged_entries.get(&path) {
            for entry in entries {
                observation.extend_from_slice(entry);
                observation.push(0);
            }
        }
        observation.push(0);
    }
    Ok(observation)
}

fn staged_index_entries_for_paths(
    repository: &Path,
    paths: &[Vec<u8>],
) -> Result<std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>, String> {
    let mut entries = std::collections::BTreeMap::new();
    for batch in paths.chunks(256) {
        let mut command = phoenix_core::git::command();
        command.args(["--literal-pathspecs", "ls-files", "--stage", "-z", "--"]);
        for path in batch {
            command.arg(path_buf_from_git_bytes(path));
        }
        let output = command
            .current_dir(repository)
            .output()
            .map_err(|error| format!("cannot inspect dirty index: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        for (path, mut path_entries) in staged_index_entries_by_path(&output.stdout) {
            entries
                .entry(path)
                .or_insert_with(Vec::new)
                .append(&mut path_entries);
        }
    }
    Ok(entries)
}

fn staged_index_entries_by_path(index: &[u8]) -> std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut entries = std::collections::BTreeMap::<Vec<u8>, Vec<Vec<u8>>>::new();
    for entry in index
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        entries
            .entry(entry[tab + 1..].to_vec())
            .or_default()
            .push(entry.to_vec());
    }
    entries
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

fn rotate_inspection_generation(
    snapshot: CloseRetirementSnapshot,
    generation: Option<&str>,
) -> Result<CloseRetirementSnapshot, String> {
    let Some(generation) = generation else {
        return Ok(snapshot);
    };
    CloseRetirementSnapshot::parse(generation, snapshot.fingerprint().to_string())
        .map_err(|error| error.to_string())
}

fn snapshot_for(bytes: &[u8]) -> CloseRetirementSnapshot {
    let digest = Sha256::digest(bytes);
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut fingerprint, "{byte:02x}").expect("writing into String cannot fail");
    }
    CloseRetirementSnapshot::parse("server_git_status_v2", fingerprint)
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

fn tmux_retirement_outcome(
    outcome: TmuxRetirementOutcome,
) -> Result<RetirementOutcome, (RetirementFailureReason, String)> {
    match outcome {
        TmuxRetirementOutcome::Retired => Ok(RetirementOutcome::Retired),
        TmuxRetirementOutcome::AbsenceVerified => Ok(RetirementOutcome::AbsenceAdopted {
            absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
        }),
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
        both_worktree_paths_absent, canonical_status_observation,
        complete_persisted_worktree_administrative_cleanup, exact_worktree_administrative_dir,
        git_path_from_observation, inspect_and_remove_exact_worktree_with_hook, inspect_worktree,
        observe_administrative_dir_incarnation, parse_status_losses,
        planned_administrative_dir_is_absent, quarantine_and_remove_exact_worktree,
        remove_exact_worktree_administrative_dir_with_hook,
        remove_quarantine_then_administrative_dir,
        remove_quarantine_then_administrative_dir_with_hooks, rotate_inspection_generation,
        run_bounded_git_status_until, snapshot_for, staged_index_entries_by_path,
        staged_index_entries_for_paths, worktree_quarantine_path, CloseLeaseFailure,
        ExactWorktreeRemoval,
    };
    use phoenix_core::domain::close::{
        CloseLossItem, GitPathIdentity, WorktreeFingerprint, WorktreeId, WorktreeIdentity,
    };
    use std::path::Path;

    #[cfg(target_os = "macos")]
    #[test]
    fn full_descriptor_buffer_requires_larger_inventory() {
        assert!(super::descriptor_inventory_may_be_truncated(4096, 4096));
        assert!(super::descriptor_inventory_may_be_truncated(8192, 4096));
        assert!(!super::descriptor_inventory_may_be_truncated(4080, 4096));
    }

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

    #[test]
    fn completed_cleanup_crash_boundary_requires_both_paths_and_planned_admin_absent() {
        let temp = tempfile::tempdir().unwrap();
        let captured = temp.path().join("captured");
        let quarantine = temp.path().join("quarantine");
        let planned_admin = temp.path().join("admin");

        assert!(both_worktree_paths_absent(&captured, &quarantine).unwrap());
        assert!(planned_administrative_dir_is_absent(&planned_admin).unwrap());

        std::fs::create_dir(&captured).unwrap();
        assert!(!both_worktree_paths_absent(&captured, &quarantine).unwrap());
        std::fs::remove_dir(&captured).unwrap();
        std::fs::create_dir(&quarantine).unwrap();
        assert!(!both_worktree_paths_absent(&captured, &quarantine).unwrap());
        std::fs::remove_dir(&quarantine).unwrap();
        std::fs::create_dir(&planned_admin).unwrap();
        assert!(!planned_administrative_dir_is_absent(&planned_admin).unwrap());
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
        let unrelated = temp.path().join("unrelated-stale");
        run_git(
            &bare,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                unrelated.to_str().unwrap(),
                "HEAD",
            ],
        );
        let unrelated_admin = exact_worktree_administrative_dir(&unrelated, &bare).unwrap();
        std::fs::remove_dir_all(&unrelated).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&linked, &bare).unwrap();

        let outcome = quarantine_and_remove_exact_worktree(
            &identity,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(matches!(outcome, ExactWorktreeRemoval::Retired));
        assert!(!linked.exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
        let listing = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&bare)
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert!(!String::from_utf8_lossy(&listing.stdout).contains(linked.to_str().unwrap()));
        assert!(unrelated_admin.exists());
    }

    #[test]
    fn top_level_status_probe_drains_output_larger_than_pipe_capacity() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        for index in 0..3_000 {
            std::fs::write(
                temp.path()
                    .join(format!("untracked-{index:04}-with-a-moderately-long-name")),
                b"loss\n",
            )
            .unwrap();
        }
        let output = run_bounded_git_status_until(
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.len() > 64 * 1024);
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
        assert!(matches!(retry, ExactWorktreeRemoval::Retired));
        assert!(!retry_quarantine.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resumed_quarantine_change_requests_reinspection_without_deleting_loss() {
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
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        std::fs::rename(&linked, &quarantine).unwrap();
        std::fs::write(quarantine.join("write-after-crash"), "preserve\n").unwrap();
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
            std::fs::read_to_string(quarantine.join("write-after-crash")).unwrap(),
            "preserve\n"
        );
    }

    #[test]
    fn worktree_quarantine_swap_before_final_move_preserves_replacement_for_needs_repair() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let displaced = temp.path().join("checked-object");
        let administrative_dir = temp.path().join("already-removed-admin");
        initialize_repository(&quarantine);
        let replacement_marker = "replacement must survive\n";

        let error = remove_quarantine_then_administrative_dir_with_hooks(
            &quarantine,
            &administrative_dir,
            "unused-after-failure",
            {
                let quarantine = quarantine.clone();
                let displaced_for_swap = displaced.clone();
                move |_| {
                    std::fs::rename(&quarantine, &displaced_for_swap).unwrap();
                    initialize_repository(&quarantine);
                    std::fs::write(quarantine.join("replacement-marker"), replacement_marker)
                        .unwrap();
                }
            },
            || {},
        )
        .unwrap_err();

        assert!(error.contains("identity changed before final deletion"));
        assert!(displaced.exists());
        let preserved = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("object/replacement-marker"))
            .find(|candidate| candidate.is_file())
            .expect("swapped worktree replacement must remain in the private tombstone");
        assert_eq!(
            std::fs::read_to_string(preserved).unwrap(),
            replacement_marker
        );
    }

    #[test]
    fn administrative_quarantine_swap_before_final_move_preserves_replacement_for_needs_repair() {
        let temp = tempfile::tempdir().unwrap();
        let administrative_dir = temp.path().join("admin");
        std::fs::create_dir(&administrative_dir).unwrap();
        std::fs::write(administrative_dir.join("original"), "original\n").unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative_dir).unwrap();
        let quarantine =
            super::administrative_dir_quarantine_path(&administrative_dir, &incarnation).unwrap();
        let displaced = temp.path().join("checked-admin");
        let replacement_marker = "replacement must survive\n";

        let error = remove_exact_worktree_administrative_dir_with_hook(
            &administrative_dir,
            &incarnation,
            {
                let quarantine = quarantine.clone();
                let displaced_for_swap = displaced.clone();
                move |_| {
                    std::fs::rename(&quarantine, &displaced_for_swap).unwrap();
                    std::fs::create_dir(&quarantine).unwrap();
                    std::fs::write(quarantine.join("replacement-marker"), replacement_marker)
                        .unwrap();
                }
            },
        )
        .unwrap_err();

        assert!(error.contains("identity changed before final deletion"));
        assert!(displaced.exists());
        let preserved = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("object/replacement-marker"))
            .find(|candidate| candidate.is_file())
            .expect("swapped admin replacement must remain in the private tombstone");
        assert_eq!(
            std::fs::read_to_string(preserved).unwrap(),
            replacement_marker
        );
    }

    #[test]
    fn restart_completes_only_exact_persisted_admin_cleanup_after_quarantine_removal() {
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
        let common = super::exact_worktree_common_git_dir(&linked).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&linked, &common).unwrap();
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        let administrative_dir_incarnation =
            observe_administrative_dir_incarnation(&administrative_dir).unwrap();

        std::fs::rename(&linked, &quarantine).unwrap();
        let injected_crash = std::panic::catch_unwind(|| {
            remove_quarantine_then_administrative_dir(
                &quarantine,
                &administrative_dir,
                &administrative_dir_incarnation,
                || panic!("injected crash after quarantine removal"),
            )
            .unwrap();
        });
        assert!(injected_crash.is_err());
        assert!(!linked.exists());
        assert!(!quarantine.exists());
        assert!(administrative_dir.exists());

        complete_persisted_worktree_administrative_cleanup(
            &identity,
            &administrative_dir,
            &administrative_dir_incarnation,
        )
        .unwrap();

        assert!(!administrative_dir.exists());
        assert!(!linked.exists());
        assert!(!quarantine.exists());
        assert!(repository.exists());
    }

    #[test]
    fn restart_refuses_mismatched_persisted_admin_cleanup_plan() {
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
        let common = super::exact_worktree_common_git_dir(&linked).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&linked, &common).unwrap();
        let mismatched = common.join("worktrees").join("mismatched-plan");
        std::fs::create_dir(&mismatched).unwrap();
        std::fs::write(
            mismatched.join("gitdir"),
            linked.join(".git").as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        std::fs::rename(&linked, &quarantine).unwrap();
        std::fs::remove_dir_all(&quarantine).unwrap();

        let error = complete_persisted_worktree_administrative_cleanup(
            &identity,
            &mismatched,
            &observe_administrative_dir_incarnation(&mismatched).unwrap(),
        )
        .unwrap_err();

        assert!(error.contains("does not match the captured worktree registration incarnation"));
        assert!(mismatched.exists());
        assert!(administrative_dir.exists());
    }

    #[test]
    fn restart_refuses_replacement_admin_dir_with_same_path_and_backlink() {
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
        let common = super::exact_worktree_common_git_dir(&linked).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&linked, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative_dir).unwrap();
        let backlink = std::fs::read(administrative_dir.join("gitdir")).unwrap();
        let displaced = administrative_dir.with_extension("displaced");
        std::fs::rename(&administrative_dir, &displaced).unwrap();
        std::fs::create_dir(&administrative_dir).unwrap();
        std::fs::write(administrative_dir.join("gitdir"), backlink).unwrap();
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        std::fs::rename(&linked, &quarantine).unwrap();
        std::fs::remove_dir_all(&quarantine).unwrap();

        let error = complete_persisted_worktree_administrative_cleanup(
            &identity,
            &administrative_dir,
            &incarnation,
        )
        .unwrap_err();

        assert!(error.contains("incarnation changed"));
        assert!(administrative_dir.exists());
        assert!(displaced.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn absent_quarantine_refuses_unverified_administrative_cleanup() {
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
        let common = super::exact_worktree_common_git_dir(&linked).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&linked, &common).unwrap();
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        let planned_administrative_dir = administrative_dir.clone();
        let planned_administrative_dir_incarnation =
            observe_administrative_dir_incarnation(&administrative_dir).unwrap();
        std::fs::rename(&linked, &quarantine).unwrap();
        std::fs::remove_dir_all(&quarantine).unwrap();
        assert!(administrative_dir.exists());
        let runtime = tokio::runtime::Handle::current();

        let outcome = tokio::task::spawn_blocking(move || {
            super::inspect_and_remove_exact_worktree_with_hook_and_plan(
                &runtime,
                &identity,
                &confirmed,
                &planned_administrative_dir,
                &planned_administrative_dir_incarnation,
                |_| {},
            )
        })
        .await
        .unwrap()
        .unwrap();

        let ExactWorktreeRemoval::Residual { detail } = outcome else {
            panic!("unverifiable registration must route to repair");
        };
        assert!(detail.contains("refusing to delete unverified Git registration"));
        assert!(administrative_dir.exists());
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
    async fn dirty_file_content_changes_invalidate_snapshot_without_status_shape_change() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let identity = inspection_identity(temp.path());

        std::fs::write(temp.path().join("tracked"), "first dirty payload\n").unwrap();
        let (first, first_losses) = inspect_worktree(&identity).await.unwrap();
        std::fs::write(temp.path().join("tracked"), "second dirty payload\n").unwrap();
        let (second, second_losses) = inspect_worktree(&identity).await.unwrap();

        assert_eq!(first_losses, second_losses);
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn untracked_file_content_changes_invalidate_snapshot_without_status_shape_change() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let identity = inspection_identity(temp.path());

        std::fs::write(temp.path().join("untracked"), "first payload\n").unwrap();
        let (first, first_losses) = inspect_worktree(&identity).await.unwrap();
        std::fs::write(temp.path().join("untracked"), "second payload\n").unwrap();
        let (second, second_losses) = inspect_worktree(&identity).await.unwrap();

        assert_eq!(first_losses, second_losses);
        assert_ne!(first, second);
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
        let common = super::exact_worktree_common_git_dir(&closing).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&closing, &common).unwrap();
        let outcome = quarantine_and_remove_exact_worktree(
            &identity,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(matches!(outcome, ExactWorktreeRemoval::Retired));
        assert!(!closing.exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
    }

    #[test]
    fn descriptor_scan_has_no_external_executable_dependency() {
        let source = include_str!("close_retirement.rs");
        assert!(!source.contains("Command::new(\"lsof\")"));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn quarantine_detects_external_process_working_directory() {
        let temp = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .current_dir(temp.path())
            .spawn()
            .unwrap();
        assert!(super::quarantine_has_process_cwd(temp.path()).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
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

        let administrative_dir = closing.join(".git");
        let outcome = quarantine_and_remove_exact_worktree(
            &identity,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            move |_| {
                descriptor.write_all(b"after\n").unwrap();
                descriptor.flush().unwrap();
                std::mem::forget(descriptor);
            },
        )
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
    fn staged_index_entries_are_batched_and_preserve_pathspec_magic_literally() {
        let index = b"100644 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 0\t:(bad)file\0\
                      100644 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 0\tordinary\0";
        let entries = staged_index_entries_by_path(index);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[b":(bad)file".as_slice()],
            vec![b"100644 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 0\t:(bad)file".to_vec()]
        );
    }

    #[test]
    fn staged_index_query_materializes_only_requested_dirty_paths() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        std::fs::write(temp.path().join("unrelated"), "tracked\n").unwrap();
        run_git(temp.path(), &["add", "unrelated"]);
        run_git(temp.path(), &["commit", "--quiet", "-m", "unrelated"]);

        let entries = staged_index_entries_for_paths(temp.path(), &[b"tracked".to_vec()]).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key(b"tracked".as_slice()));
        assert!(!entries.contains_key(b"unrelated".as_slice()));
    }

    #[test]
    fn retry_reinspection_rotates_inventory_generation_without_changing_content_fingerprint() {
        let snapshot = snapshot_for(b"same worktree contents");
        let rotated =
            rotate_inspection_generation(snapshot.clone(), Some("retry-generation")).unwrap();
        assert_eq!(rotated.generation(), "retry-generation");
        assert_eq!(rotated.fingerprint(), snapshot.fingerprint());
        assert_ne!(rotated, snapshot);
    }

    #[tokio::test]
    async fn dirty_pathspec_magic_filename_is_inspected_as_a_literal_path() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let path = temp.path().join(":(bad)file");
        std::fs::write(&path, "base\n").unwrap();
        run_git(temp.path(), &["add", ":(literal):(bad)file"]);
        run_git(temp.path(), &["commit", "--quiet", "-m", "literal path"]);
        std::fs::write(path, "dirty\n").unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b":(bad)file"
        )));
    }

    #[tokio::test]
    async fn snapshot_changes_when_only_the_staged_blob_changes() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let tracked = temp.path().join("tracked");
        std::fs::write(&tracked, "base").unwrap();
        assert!(phoenix_core::git::command()
            .args(["add", "tracked"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success());
        assert!(phoenix_core::git::command()
            .args(["commit", "-m", "base"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success());

        std::fs::write(&tracked, "staged-one").unwrap();
        assert!(phoenix_core::git::command()
            .args(["add", "tracked"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success());
        std::fs::write(&tracked, "working-copy").unwrap();
        let identity = inspection_identity(temp.path());
        let (first, _) = inspect_worktree(&identity).await.unwrap();

        std::fs::write(&tracked, "staged-two").unwrap();
        assert!(phoenix_core::git::command()
            .args(["add", "tracked"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success());
        std::fs::write(&tracked, "working-copy").unwrap();
        let (second, _) = inspect_worktree(&identity).await.unwrap();

        assert_ne!(first.fingerprint(), second.fingerprint());
    }

    #[test]
    fn porcelain_snapshot_digest_changes_with_server_observation() {
        let clean = snapshot_for(b"");
        let dirty = snapshot_for(b"?? server-observed\0");
        assert_eq!(clean.generation(), "server_git_status_v2");
        assert_ne!(clean.fingerprint(), dirty.fingerprint());
    }
}
