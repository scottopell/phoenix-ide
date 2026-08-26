//! Exact-instance resource fences and receipts for Close retirement.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptId, CloseLossItem, CloseOwnedResourceInventory,
    ClosePhase, CloseRetirementSnapshot, GitPathIdentity, LossItemIdentity, OpaqueIdentity,
    RetiredResourceIdentity, RetiredResourceKind, RetirementFailureReason, RetirementOutcome,
    WorktreeIdentity,
};
use phoenix_core::work_scope::{
    ResourceScopeKey, WorkScopeId, WorkScopeRetirementOutcome, WorkScopeRetirementPrecondition,
};
use phoenix_terminal::session::{TerminalRetirementOutcome, TerminalRetirementPermit};
use phoenix_tools::{
    bash::registry::{BashRetirementOutcome, BashRetirementPermit},
    browser::session::{BrowserRetirementOutcome, BrowserRetirementPermit},
    tmux::registry::{TmuxRetirementOutcome, TmuxRetirementPermit},
};

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

impl RuntimeManager {
    /// Inspects exact server-owned captured worktrees and persists normalized loss evidence.
    pub(crate) async fn inspect_close_retirement(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<CloseRetirementSnapshot, String> {
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let mut requests = Vec::with_capacity(scopes.len());
        for scope in scopes {
            let (snapshot, losses) = match scope.captured_worktree {
                None => (snapshot_for(b"no-worktree"), Vec::new()),
                Some(CapturedWorktreeIdentity::Resolved(identity)) => {
                    inspect_worktree(&identity).await?
                }
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
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
        self.db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
            .snapshot()
            .cloned()
            .ok_or_else(|| "server inspection did not persist aggregate snapshot".to_string())
    }

    /// Acquires every registry admission fence before Close seals inventory.
    ///
    /// The map lock spans check, fencing, and insertion so concurrent callers for
    /// one exact `(attempt, scope)` cannot mint competing generations.
    pub(crate) async fn acquire_close_resource_lease(
        &self,
        attempt_id: &CloseAttemptId,
        scope: WorkScopeId,
    ) -> Vec<RetiredResourceIdentity> {
        let mut leases = self.close_retirement_leases.lock().await;
        if let Some(existing) = leases
            .get(&(attempt_id.as_str().to_string(), scope.clone()))
            .map(|lease| lease.resources.clone())
        {
            return existing;
        }
        let key = ResourceScopeKey::Work(scope.clone());
        let bash = self.bash_handles().begin_retirement(&key).await;
        let tmux = self
            .tmux_registry()
            .begin_retirement(&key, None, None)
            .await;
        let terminal = self.terminals.begin_retirement(&key);
        let browser = self.browser_sessions().begin_retirement(&key).await;
        let mut resources = Vec::new();
        for target in &bash.exact_process_groups {
            resources.push(opaque_resource(
                RetiredResourceKind::BashProcessGroup,
                format!("handle:{}:pgid:{}", target.handle_id, target.pgid),
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
        resources
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
        for captured in scopes {
            let resources = self
                .acquire_close_resource_lease(&attempt_id, captured.scope.clone())
                .await;
            let worktree = match captured.captured_worktree {
                None => None,
                Some(CapturedWorktreeIdentity::Resolved(identity)) => Some(identity),
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
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
                let LossItemIdentity::Opaque(identity) = resource.identity() else {
                    return Err("registry resource identity was not opaque".to_string());
                };
                match resource.kind() {
                    RetiredResourceKind::BashProcessGroup => {
                        inventory.bash_process_groups.insert(identity.clone());
                    }
                    RetiredResourceKind::TmuxServer => {
                        inventory.tmux_servers.insert(identity.clone());
                    }
                    RetiredResourceKind::PtySession => {
                        inventory.pty_sessions.insert(identity.clone());
                    }
                    RetiredResourceKind::BrowserSession => {
                        inventory.browser_sessions.insert(identity.clone());
                    }
                    kind => return Err(format!("unexpected permit resource kind {kind:?}")),
                }
            }
            requests.push(CaptureCloseRetirementInventoryScopeRequest {
                scope: captured.scope,
                inventory,
            });
        }
        let acquired_scopes = requests
            .iter()
            .map(|request| request.scope.clone())
            .collect::<Vec<_>>();
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
        let scopes = runtime_targets
            .iter()
            .map(|target| target.scope.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for scope in scopes {
            let mut expected = runtime_targets
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
                let observed = self
                    .acquire_close_resource_lease(&attempt_id, scope.clone())
                    .await;
                let observed_keys = observed
                    .iter()
                    .map(resource_key)
                    .collect::<std::collections::BTreeSet<_>>();
                let absent_after_dispatch = expected
                    .iter()
                    .filter(|resource| !observed_keys.contains(&resource_key(resource)))
                    .cloned()
                    .collect::<Vec<_>>();
                for resource in absent_after_dispatch {
                    let dispatched = self
                        .db()
                        .close_retirement_resource_was_dispatched(
                            &attempt_id,
                            &scope,
                            &snapshot,
                            &resource,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    if !dispatched {
                        return self
                            .record_close_residual(
                                &attempt_id,
                                &snapshot,
                                &scope,
                                resource,
                                RetirementFailureReason::IdentityNotProven,
                                "restart observed an absent runtime resource without an exact prior dispatch",
                            )
                            .await;
                    }
                    self.record_close_retired(
                        &attempt_id,
                        &snapshot,
                        &scope,
                        resource.clone(),
                        "exact prior dispatch and restart absence verification",
                    )
                    .await?;
                    expected.retain(|candidate| candidate != &resource);
                }
                let expected_keys = expected
                    .iter()
                    .map(resource_key)
                    .collect::<std::collections::BTreeSet<_>>();
                if observed_keys != expected_keys {
                    let resource = expected
                        .first()
                        .cloned()
                        .or_else(|| observed.first().cloned())
                        .ok_or_else(|| {
                            "runtime target comparison lost its exact identity".to_string()
                        })?;
                    return self
                        .record_close_residual(
                            &attempt_id,
                            &snapshot,
                            &scope,
                            resource,
                            RetirementFailureReason::IdentityNotProven,
                            "restart observed a different runtime-resource instance set than the sealed Close inventory",
                        )
                        .await;
                }
            }
            if expected.is_empty() {
                self.cancel_close_resource_lease(&attempt_id, &scope).await;
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
                Err(reason) => {
                    return self
                        .record_close_residual(
                            &attempt_id,
                            &snapshot,
                            &scope,
                            expected[0].clone(),
                            RetirementFailureReason::RemovalFailed,
                            &reason,
                        )
                        .await;
                }
            };
            let expected_keys = expected
                .iter()
                .map(resource_key)
                .collect::<std::collections::BTreeSet<_>>();
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
            .await
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
            if let Some(target) = worktree_target {
                if !retired.contains(&(scope.clone(), resource_key(&target.resource))) {
                    let was_dispatched = self
                        .db()
                        .close_retirement_resource_was_dispatched(
                            attempt_id,
                            &scope,
                            snapshot,
                            &target.resource,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    self.db()
                        .record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
                            attempt_id: attempt_id.clone(),
                            scope: scope.clone(),
                            snapshot: snapshot.clone(),
                            resource: target.resource.clone(),
                        })
                        .await
                        .map_err(|error| error.to_string())?;
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
                    if let Err(reason) = remove_exact_worktree(identity).await {
                        if was_dispatched && reason.contains("absent") {
                            self.record_close_retired(
                                attempt_id,
                                snapshot,
                                &scope,
                                target.resource.clone(),
                                "exact prior dispatch and restart worktree absence verification",
                            )
                            .await?;
                            continue;
                        }
                        let failure = if reason.contains("incarnation")
                            || reason.contains("registered")
                            || reason.contains("absent")
                            || reason.contains("server-owned")
                        {
                            RetirementFailureReason::IdentityNotProven
                        } else {
                            RetirementFailureReason::RemovalFailed
                        };
                        return self
                            .record_close_residual(
                                attempt_id,
                                snapshot,
                                &scope,
                                target.resource.clone(),
                                failure,
                                &reason,
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
    pub(crate) async fn complete_close_resource_lease(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
    ) -> Result<Vec<RetiredResourceIdentity>, String> {
        let key = (attempt_id.as_str().to_string(), scope.clone());
        let lease = self.close_retirement_leases.lock().await.remove(&key);
        let Some(lease) = lease else {
            return Err("Close resource lease is unavailable after restart; re-fence and verify durable identities".to_string());
        };
        let result = async {
            require_absent(self.bash_handles().complete_retirement(&lease.bash).await)?;
            require_tmux_absent(
                self.tmux_registry()
                    .complete_retirement(&lease.tmux)
                    .await
                    .map_err(|error| error.to_string())?,
            )?;
            require_terminal_absent(self.terminals.complete_retirement(&lease.terminal).await)?;
            require_browser_absent(
                self.browser_sessions()
                    .complete_retirement(&lease.browser)
                    .await,
            )?;
            Ok::<_, String>(lease.resources.clone())
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
    let output = tokio::task::spawn_blocking(move || {
        phoenix_core::git::command()
            .args([
                "status",
                "--porcelain=v1",
                "-z",
                "--ignored",
                "--untracked-files=all",
            ])
            .current_dir(path)
            .output()
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok((
        snapshot_for(&output.stdout),
        parse_status_losses(&output.stdout),
    ))
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
        let common = PathBuf::from(String::from_utf8_lossy(&common.stdout).trim());
        let Some(repo) = common.parent().map(Path::to_path_buf) else {
            return Ok((None, false, false));
        };
        let target = std::fs::canonicalize(&path_for_check)?;
        let listed = phoenix_core::git::command()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&repo)
            .output()?;
        let registered = listed.status.success()
            && String::from_utf8_lossy(&listed.stdout).lines().any(|line| {
                line.strip_prefix("worktree ").is_some_and(|candidate| {
                    std::fs::canonicalize(candidate).ok().as_ref() == Some(&target)
                })
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
        phoenix_core::git::command()
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .current_dir(repo)
            .output()
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn parse_status_losses(status: &[u8]) -> Vec<CloseLossItem> {
    let mut losses = Vec::new();
    for row in status.split(|byte| *byte == 0) {
        if row.len() < 4 {
            continue;
        }
        let path = GitPathIdentity::from_bytes(row[3..].to_vec());
        match &row[..2] {
            b"??" => losses.push(CloseLossItem::UntrackedNonIgnoredPath(path)),
            b"!!" => {}
            xy => {
                if xy[0] != b' ' {
                    losses.push(CloseLossItem::StagedTrackedPath(path.clone()));
                }
                if xy[1] != b' ' {
                    losses.push(CloseLossItem::UnstagedTrackedPath(path));
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
    use std::fmt::Write as _;
    use std::io::Read as _;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt as _;
    const MAX_GIT_POINTER_BYTES: u64 = 16 * 1024;
    let marker = path.join(".git");
    let metadata = std::fs::symlink_metadata(&marker).ok()?;
    let marker_is_file = metadata.is_file();
    let marker_bytes = if marker_is_file {
        if metadata.len() > MAX_GIT_POINTER_BYTES {
            return None;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
        std::fs::File::open(&marker)
            .ok()?
            .take(MAX_GIT_POINTER_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if u64::try_from(bytes.len()).ok()? > MAX_GIT_POINTER_BYTES {
            return None;
        }
        let pointer = std::str::from_utf8(&bytes).ok()?;
        let git_dir = pointer
            .strip_suffix("\r\n")
            .or_else(|| pointer.strip_suffix('\n'))
            .unwrap_or(pointer)
            .strip_prefix("gitdir: ")?;
        if git_dir.is_empty() || git_dir.contains(['\n', '\r']) {
            return None;
        }
        git_dir.as_bytes().to_vec()
    } else if metadata.is_dir() {
        Vec::new()
    } else {
        return None;
    };
    let mut encoded = String::with_capacity(marker_bytes.len() * 2);
    for byte in marker_bytes {
        write!(&mut encoded, "{byte:02x}").ok()?;
    }
    #[cfg(unix)]
    {
        let created_nanos = metadata
            .created()
            .ok()
            .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos().to_string());
        if created_nanos.is_none() && !marker_is_file {
            return None;
        }
        Some(format!(
            "git_admin_incarnation_v1:{}:{}:{}:{encoded}",
            metadata.dev(),
            metadata.ino(),
            created_nanos.unwrap_or_else(|| "unavailable".to_string())
        ))
    }
    #[cfg(not(unix))]
    Some(format!("git_admin_incarnation_v1:portable:{encoded}"))
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

fn require_tmux_absent(outcome: TmuxRetirementOutcome) -> Result<(), String> {
    match outcome {
        TmuxRetirementOutcome::Retired | TmuxRetirementOutcome::AbsenceVerified => Ok(()),
        TmuxRetirementOutcome::Residual { reason } => Err(reason),
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
    use super::{parse_status_losses, snapshot_for};
    use phoenix_core::domain::close::CloseLossItem;

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
    fn porcelain_snapshot_digest_changes_with_server_observation() {
        let clean = snapshot_for(b"");
        let dirty = snapshot_for(b"?? server-observed\0");
        assert_eq!(clean.generation(), "server_git_status_v1");
        assert_ne!(clean.fingerprint(), dirty.fingerprint());
    }
}
