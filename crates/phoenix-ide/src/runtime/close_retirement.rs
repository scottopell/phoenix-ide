//! Exact-instance resource fences and receipts for Close retirement.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use phoenix_core::domain::close::{
    AbsenceBasis, CapturedWorktreeIdentity, CloseAttemptId, CloseCompletionOutcome,
    CloseExpectedRetirementResource, CloseLossItem, CloseOwnedResourceInventory, ClosePhase,
    CloseRetirementSnapshot, CloseRunOrdinal, CloseRunRef, CloseRunStatus, CloseStopCertainty,
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
    AdmitCloseSafeRetryRequest, BindCloseWorktreeFinalTombstoneObjectRequest,
    BindCloseWorktreeFinalTombstoneRequest, CaptureCloseRetirementInventoryRequest,
    CaptureCloseRetirementInventoryScopeRequest, CloseCleanupFailureAuthority,
    CloseCleanupFailureResource, CloseCleanupResourceDisposition, CloseProcessResourceKind,
    CloseProcessStepOutcome, CloseProcessStepSuccess, CloseRetryRequestedBy, CloseSafeRetryEffect,
    CloseWorktreeFinalTombstone, RecordCloseRetirementDispatchRequest,
    RecordCloseRetirementEvidenceRequest, RecordCloseWorktreeCleanupPlanRequest,
    ReplaceCloseInspectionRequest, ReplaceCloseInspectionScopeRequest,
    TerminalizeInitialCloseCleanupFailureRequest,
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
        resource: RetiredResourceIdentity,
        reason: String,
    },
    UnattributedProcessEpoch {
        kind: RetiredResourceKind,
        captured_resources: Vec<RetiredResourceIdentity>,
        reason: String,
    },
    Persistence(String),
    Unavailable,
    Tmux {
        reason: RetirementFailureReason,
        detail: String,
    },
}

impl CloseLeaseFailure {
    fn process_epoch(
        kind: RetiredResourceKind,
        captured_resources: Vec<RetiredResourceIdentity>,
        reason: String,
    ) -> Self {
        match captured_resources.as_slice() {
            [resource] => Self::ProcessEpoch {
                resource: resource.clone(),
                reason,
            },
            _ => Self::UnattributedProcessEpoch {
                kind,
                captured_resources,
                reason,
            },
        }
    }
}

impl std::fmt::Display for CloseLeaseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProcessEpoch { resource, reason } => write!(
                formatter,
                "{} process-epoch teardown failed for {:?}: {reason}",
                resource.kind().as_str(),
                resource.identity()
            ),
            Self::UnattributedProcessEpoch {
                kind,
                captured_resources,
                reason,
            } => write!(
                formatter,
                "{} process-epoch teardown failed without an individually attributed target; captured resources {captured_resources:?}: {reason}",
                kind.as_str()
            ),
            Self::Unavailable => write!(
                formatter,
                "Close resource lease is unavailable; process-epoch identities cannot be rehydrated"
            ),
            Self::Persistence(detail) => {
                write!(formatter, "Close step success persistence failed: {detail}")
            }
            Self::Tmux { detail, .. } => write!(formatter, "tmux teardown failed: {detail}"),
        }
    }
}

fn close_safe_retry_effects(
    remaining_resources: &[CloseCleanupFailureResource],
) -> Result<Vec<CloseSafeRetryEffect>, String> {
    let plan = remaining_resources
        .iter()
        .map(|remaining| {
            if remaining.disposition == CloseCleanupResourceDisposition::Unknown
                || !matches!(
                    remaining.resource.kind(),
                    RetiredResourceKind::Worktree
                        | RetiredResourceKind::WorkScope
                        | RetiredResourceKind::TmuxServer
                )
            {
                return Err(format!(
                    "resource {:?} needs separate repair authority",
                    remaining.resource
                ));
            }
            Ok(CloseSafeRetryEffect {
                scope: remaining.scope.clone(),
                resource: remaining.resource.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if plan.is_empty()
        || plan
            .iter()
            .enumerate()
            .any(|(index, effect)| plan[..index].contains(effect))
    {
        return Err("safe retry requires distinct exact remaining resources".into());
    }
    Ok(plan)
}

fn process_step_successes_cover(
    successes: &[CloseProcessStepSuccess],
    scope: &WorkScopeId,
    resource_kind: CloseProcessResourceKind,
    identities: &[String],
) -> bool {
    !identities.is_empty()
        && identities.iter().all(|identity| {
            successes.iter().any(|success| {
                success.scope == *scope
                    && success.resource_kind == resource_kind
                    && success.identity.as_str() == identity
            })
        })
}

enum CloseSafeRetryTmuxError {
    Failed(String),
    PersistenceOrUncertain(String),
}

fn close_safe_retry_execution_order(plan: &[CloseSafeRetryEffect]) -> Vec<usize> {
    let mut indices = (0..plan.len()).collect::<Vec<_>>();
    indices.sort_by_key(|&index| match plan[index].resource.kind() {
        RetiredResourceKind::TmuxServer => 0,
        RetiredResourceKind::Worktree => 1,
        RetiredResourceKind::WorkScope => 2,
        _ => 3,
    });
    indices
}

impl RuntimeManager {
    pub(crate) async fn complete_close_retirement_and_publish(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<(), String> {
        let broadcasters = self.close_retirement_broadcasters(attempt_id).await?;
        self.db()
            .complete_close_retirement(attempt_id)
            .await
            .map_err(|error| error.to_string())?;
        Self::publish_close_retirement_updates(broadcasters, true);
        Ok(())
    }

    async fn close_retirement_broadcasters(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<Vec<crate::runtime::SseBroadcaster>, String> {
        let participant_ids = self
            .db()
            .list_close_retirement_archived_conversation_ids(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let mut broadcasters = Vec::new();
        for conversation_id in participant_ids {
            if let Some(broadcaster) = self
                .existing_conversation_broadcaster(&conversation_id)
                .await
            {
                broadcasters.push(broadcaster);
            }
        }
        Ok(broadcasters)
    }

    fn publish_close_retirement_updates(
        broadcasters: Vec<crate::runtime::SseBroadcaster>,
        archived: bool,
    ) {
        for broadcaster in broadcasters {
            let _ = broadcaster.send_seq(|seq| crate::runtime::SseEvent::ConversationUpdate {
                sequence_id: seq,
                update: crate::runtime::ConversationMetadataUpdate {
                    slug: None,
                    title: None,
                    cwd: None,
                    project_id: None,
                    project_name: None,
                    updated_at: None,
                    branch_name: None,
                    worktree_path: None,
                    conv_mode_label: None,
                    base_branch: None,
                    task_title: None,
                    work_scope_key: None,
                    model: None,
                    archived: Some(archived),
                },
            });
        }
    }

    /// Restores admission-only fences from durable unresolved Close state.
    pub(crate) async fn restore_close_admission_fences(&self) -> Result<(), String> {
        let scopes = self
            .db()
            .list_close_execution_fence_scopes()
            .await
            .map_err(|error| error.to_string())?;
        for scope in scopes {
            let resource_scope = ResourceScopeKey::Work(scope);
            self.bash_handles()
                .fence_retirement_admission(&resource_scope)
                .await;
            drop(self.terminals.begin_retirement(&resource_scope));
            drop(
                self.browser_sessions()
                    .begin_retirement(&resource_scope)
                    .await,
            );
            self.tmux_registry()
                .fence_retirement_admission(&resource_scope)
                .await;
        }
        Ok(())
    }

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
        for captured in &scopes {
            if let Err(error) = self
                .acquire_close_resource_lease(&attempt_id, captured.scope.clone())
                .await
            {
                return self
                    .route_close_attempt_to_repair(
                        &attempt_id,
                        &captured.scope,
                        RetirementFailureReason::IdentityNotProven,
                        format!("Close resource admission fencing failed: {error}"),
                    )
                    .await;
            }
        }
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
                                return self
                                    .route_close_attempt_to_repair(
                                        &attempt_id,
                                        &scope.scope,
                                        RetirementFailureReason::IdentityNotProven,
                                        "captured worktree administrative incarnation changed",
                                    )
                                    .await;
                            }
                            let (snapshot, losses) = match inspect_worktree(&identity).await {
                                Ok(inspection) => inspection,
                                Err(error) => {
                                    return self
                                        .route_close_attempt_to_repair(
                                            &attempt_id,
                                            &scope.scope,
                                            RetirementFailureReason::ManualRepairRequired,
                                            error,
                                        )
                                        .await;
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
                                return self
                                    .route_close_attempt_to_repair(
                                        &attempt_id,
                                        &scope.scope,
                                        RetirementFailureReason::IdentityNotProven,
                                        "captured worktree administrative incarnation changed",
                                    )
                                    .await;
                            }
                            let (snapshot, losses) =
                                match inspect_worktree_at(&identity, quarantine).await {
                                    Ok(inspection) => inspection,
                                    Err(error) => {
                                        return self
                                            .route_close_attempt_to_repair(
                                                &attempt_id,
                                                &scope.scope,
                                                RetirementFailureReason::ManualRepairRequired,
                                                error,
                                            )
                                            .await;
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
                            return self
                                .route_close_attempt_to_repair(
                                    &attempt_id,
                                    &scope.scope,
                                    RetirementFailureReason::IdentityNotProven,
                                    format!(
                                        "scope {} captured worktree is absent before inspection",
                                        scope.scope
                                    ),
                                )
                                .await;
                        }
                        Err(error) => {
                            return self
                                .route_close_attempt_to_repair(
                                    &attempt_id,
                                    &scope.scope,
                                    RetirementFailureReason::ManualRepairRequired,
                                    format!(
                                        "scope {} captured worktree is inaccessible before inspection: {error}",
                                        scope.scope
                                    ),
                                )
                                .await;
                        }
                    }
                }
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                    return self
                        .route_close_attempt_to_repair(
                            &attempt_id,
                            &scope.scope,
                            RetirementFailureReason::IdentityNotProven,
                            format!(
                                "scope {} has unresolved captured worktree identity",
                                scope.scope
                            ),
                        )
                        .await;
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
        self.bash_handles().fence_retirement_admission(&key).await;
        let terminal = self.terminals.begin_retirement(&key);
        let browser = self.browser_sessions().begin_retirement(&key).await;
        let tmux_expires = phoenix_tools::tmux::registry::close_deadline();
        let tmux_discovery = match self
            .tmux_registry()
            .discover_persistent_identity(&key, legacy_worktree_path.as_deref(), None, tmux_expires)
            .await
        {
            Ok(discovery) => discovery,
            Err(error) => {
                self.tmux_registry().fence_retirement_admission(&key).await;
                return Err(format!("tmux identity discovery failed: {error}"));
            }
        };
        let tmux = match tmux_discovery {
            discovery @ (PersistentTmuxDiscovery::EndpointAbsent
            | PersistentTmuxDiscovery::ServerAbsent) => match self
                .tmux_registry()
                .begin_retirement_after_discovery(&key, &discovery, tmux_expires)
                .await
            {
                Ok(permit) => permit,
                Err(outcome) => {
                    self.tmux_registry().fence_retirement_admission(&key).await;
                    return Err(format!("tmux retirement fencing failed: {outcome:?}"));
                }
            },
            PersistentTmuxDiscovery::Exact(identity) => {
                match self
                    .tmux_registry()
                    .rehydrate_retirement(&key, &identity, tmux_expires)
                    .await
                {
                    Ok(TmuxRetirementRehydration::Permit(permit)) => permit,
                    Ok(TmuxRetirementRehydration::AbsenceVerified) => match self
                        .tmux_registry()
                        .begin_retirement(&key, None, None, tmux_expires)
                        .await
                    {
                        Ok(permit) => permit,
                        Err(outcome) => {
                            self.tmux_registry().fence_retirement_admission(&key).await;
                            return Err(format!("tmux retirement fencing failed: {outcome:?}"));
                        }
                    },
                    Ok(TmuxRetirementRehydration::Residual { reason }) => {
                        self.tmux_registry().fence_retirement_admission(&key).await;
                        return Err(format!("tmux identity is ambiguous: {reason}"));
                    }
                    Err(error) => {
                        self.tmux_registry().fence_retirement_admission(&key).await;
                        return Err(format!("tmux rehydration failed: {error}"));
                    }
                }
            }
            PersistentTmuxDiscovery::Ambiguous { reason } => {
                self.tmux_registry().fence_retirement_admission(&key).await;
                return Err(format!("tmux identity is ambiguous: {reason}"));
            }
        };
        let bash = self.bash_handles().begin_retirement(&key).await;
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

    pub(crate) async fn cancel_close_resource_leases(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<(), String> {
        let mut leases = self.close_retirement_leases.lock().await;
        let keys = leases
            .keys()
            .filter(|(candidate, _)| candidate == attempt_id.as_str())
            .cloned()
            .collect::<Vec<_>>();
        let mut cancelling = Vec::with_capacity(keys.len());
        let mut permits = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(lease) = leases.remove(&key) else {
                continue;
            };
            let CloseResourceLease {
                bash,
                tmux,
                terminal,
                browser,
                resources,
            } = lease;
            cancelling.push((key, bash, terminal, browser, resources));
            permits.push(tmux);
        }
        if let Err(error) = self.tmux_registry().cancel_retirement_batch(permits).await {
            let reason = error.to_string();
            for ((key, bash, terminal, browser, resources), tmux) in
                cancelling.into_iter().zip(error.into_permits())
            {
                leases.insert(
                    key,
                    CloseResourceLease {
                        bash,
                        tmux,
                        terminal,
                        browser,
                        resources,
                    },
                );
            }
            return Err(reason);
        }
        drop(leases);
        for (_, bash, terminal, browser, _) in cancelling {
            self.bash_handles().cancel_retirement(bash).await;
            self.terminals.cancel_retirement(terminal);
            self.browser_sessions().reopen_after_permit(browser).await;
        }
        Ok(())
    }

    pub(crate) async fn cancel_close_before_retirement(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<(), String> {
        let _execution = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
        self.cancel_close_resource_leases(attempt_id).await?;
        self.db()
            .cancel_close_before_retirement(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Acquires all scope fences, then seals the exact server-owned inventory.
    #[allow(clippy::too_many_lines)]
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
            let resources = match self
                .acquire_close_resource_lease(&attempt_id, captured.scope.clone())
                .await
            {
                Ok(resources) => resources,
                Err(reason) => {
                    return self
                        .route_close_attempt_to_repair(
                            &attempt_id,
                            &captured.scope,
                            RetirementFailureReason::IdentityNotProven,
                            reason,
                        )
                        .await;
                }
            };
            let worktree = match captured.captured_worktree {
                None => None,
                Some(CapturedWorktreeIdentity::Resolved(identity)) => Some(identity),
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                    return self
                        .route_close_attempt_to_repair(
                            &attempt_id,
                            &captured.scope,
                            RetirementFailureReason::IdentityNotProven,
                            format!(
                                "scope {} has unresolved captured worktree identity",
                                captured.scope
                            ),
                        )
                        .await;
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
                        return Err(format!("unexpected durable permit resource kind {kind:?}"));
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
                self.cancel_close_resource_leases(&attempt_id)
                    .await
                    .map_err(|cancel_error| {
                        format!("{error}; fence reopening failed: {cancel_error}")
                    })?;
                Err(error.to_string())
            }
        }
    }

    /// Retires exactly the unresolved inventory targets.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn retire_close_runtime_resources(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<(), String> {
        let run = CloseRunRef::initial(attempt_id.clone());
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
            &run, &snapshot, &targets, &retired,
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
                                &run,
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
                                &run,
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
                    let tmux_expires = phoenix_tools::tmux::registry::close_deadline();
                    match self
                        .tmux_registry()
                        .rehydrate_retirement(
                            &ResourceScopeKey::Work(scope.clone()),
                            &instance,
                            tmux_expires,
                        )
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
                                        &run,
                                        &snapshot,
                                        &scope,
                                        resource.clone(),
                                        "exact durable tmux rehydration",
                                    )
                                    .await?;
                                }
                                Ok(RetirementOutcome::AbsenceAdopted { .. }) => {
                                    self.record_close_absence_adopted(
                                        &run,
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
                                            &run,
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
                                &run,
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
                                    &run,
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
                                    &run,
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
            match self
                .complete_close_resource_lease(&run, &snapshot, &scope, &expected)
                .await
            {
                Ok(()) => (),
                Err(failure @ CloseLeaseFailure::Persistence(_)) => return Err(failure.to_string()),
                Err(ref failure @ CloseLeaseFailure::ProcessEpoch { ref resource, .. }) => {
                    let detail = failure.to_string();
                    return self
                        .record_close_process_failure(
                            &run,
                            &scope,
                            Some(resource.clone()),
                            vec![],
                            &detail,
                        )
                        .await;
                }
                Err(
                    failure @ (CloseLeaseFailure::UnattributedProcessEpoch { .. }
                    | CloseLeaseFailure::Unavailable),
                ) => {
                    let detail = failure.to_string();
                    let captured_resources = match failure {
                        CloseLeaseFailure::UnattributedProcessEpoch {
                            captured_resources, ..
                        } => captured_resources,
                        _ => vec![],
                    };
                    return self
                        .record_close_process_failure(
                            &run,
                            &scope,
                            None,
                            captured_resources,
                            &detail,
                        )
                        .await;
                }
                Err(CloseLeaseFailure::Tmux { reason, detail }) => {
                    let Some(resource) = expected
                        .iter()
                        .find(|resource| resource.kind() == RetiredResourceKind::TmuxServer)
                        .cloned()
                    else {
                        return self
                            .route_close_run_to_repair(
                                &run,
                                &scope,
                                reason,
                                format!(
                                    "tmux Close teardown failed without a sealed tmux target for scope {scope}: {detail}"
                                ),
                            )
                            .await;
                    };
                    return self
                        .record_close_residual(&run, &snapshot, &scope, resource, reason, &detail)
                        .await;
                }
            };
        }
        self.retire_close_worktrees_and_scopes(&run, &snapshot, None)
            .await?;
        self.complete_close_retirement_and_publish(&attempt_id)
            .await?;
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

    /// Explicit Global-only safe retry; callers must never use this as startup recovery.
    #[allow(clippy::too_many_lines)]
    pub async fn retry_close_runtime_resources(
        &self,
        failed_run: CloseRunRef,
    ) -> Result<(), String> {
        let attempt_id = failed_run.attempt_id.clone();
        let _execution = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
        let failure = self
            .db()
            .list_close_cleanup_failures(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|failure| failure.run_ordinal == failed_run.ordinal)
            .ok_or_else(|| {
                format!(
                    "Close run {} has no exact retained failure",
                    failed_run.ordinal.get()
                )
            })?;
        let completion_only = matches!(
            failure.occurrence.authority,
            CloseCleanupFailureAuthority::AttemptInterrupted
        );
        let plan = if completion_only {
            if !self
                .db()
                .close_retry_verified_completion_eligible(&failed_run)
                .await
                .map_err(|error| error.to_string())?
            {
                return Err(
                    "attempt-level interruption has no verified completion authority".into(),
                );
            }
            Vec::new()
        } else {
            close_safe_retry_effects(&failure.occurrence.remaining_resources)?
        };
        let snapshot = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
            .snapshot()
            .cloned()
            .ok_or_else(|| "safe retry lacks original retirement snapshot".to_string())?;
        let evidence = if completion_only {
            "prior resource-plan run has exact durable success for every planned effect; this fresh run performs completion only".to_string()
        } else {
            self.validate_close_safe_retry(&attempt_id, &snapshot, &plan)
                .await?
        };
        let request = AdmitCloseSafeRetryRequest {
            failed_run,
            requested_by: CloseRetryRequestedBy::Global,
            observed_at_us: chrono::Utc::now().timestamp_micros(),
            precondition_resolution: format!("fresh read-only exact resource checks: {evidence}"),
            safety_evidence: evidence,
            remaining_effects: plan,
        };
        let run = self
            .db()
            .admit_close_safe_retry(&request)
            .await
            .map_err(|error| error.to_string())?;
        let plan = self
            .db()
            .list_close_safe_retry_effects(&run)
            .await
            .map_err(|error| error.to_string())?;
        if !plan.is_empty() {
            self.execute_close_safe_retry(&run, &snapshot, &plan)
                .await?;
        }
        let broadcasters = self.close_retirement_broadcasters(&attempt_id).await?;
        self.db()
            .complete_close_safe_retry(&run)
            .await
            .map_err(|error| error.to_string())?;
        self.discard_close_resource_leases(&attempt_id).await;
        Self::publish_close_retirement_updates(broadcasters, true);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn validate_close_safe_retry(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
        plan: &[CloseSafeRetryEffect],
    ) -> Result<String, String> {
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let expected = self
            .db()
            .list_close_expected_retirement_resources(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let inspections = self
            .db()
            .list_close_retirement_inspections(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let retired = self
            .db()
            .list_close_retirement_evidence(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        for effect in plan {
            if !expected
                .iter()
                .any(|target| target.scope == effect.scope && target.resource == effect.resource)
                || !scopes.iter().any(|captured| captured.scope == effect.scope)
            {
                return Err("retry effect lacks captured scope and sealed original target".into());
            }
        }
        let mut observations = Vec::new();
        for captured in &scopes {
            let scope = &captured.scope;
            if self
                .db()
                .work_scope_has_unresolved_product_ownership(scope)
                .await
                .map_err(|error| error.to_string())?
            {
                return Err(format!("scope {scope} has unresolved ownership"));
            }
            let planned = plan
                .iter()
                .filter(|effect| &effect.scope == scope)
                .collect::<Vec<_>>();
            for effect in &planned {
                if !expected
                    .iter()
                    .any(|target| target.scope == *scope && target.resource == effect.resource)
                {
                    return Err(format!(
                        "scope {scope} has a retry effect outside sealed original inventory"
                    ));
                }
            }
            match &captured.captured_worktree {
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                    return Err(format!("scope {scope} has unresolved worktree identity"));
                }
                Some(CapturedWorktreeIdentity::Resolved(identity)) => {
                    let path = worktree_path(identity);
                    let quarantine = worktree_quarantine_path(identity)?;
                    let cleanup_target = expected.iter().find(|target| {
                        target.scope == *scope
                            && target.resource.kind() == RetiredResourceKind::Worktree
                    });
                    let tombstone = if let Some(target) = cleanup_target {
                        self.db()
                            .close_worktree_cleanup_plan(
                                attempt_id,
                                scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?
                            .and_then(|plan| plan.final_tombstone)
                    } else {
                        None
                    };
                    probe_retry_worktree_writers(&path, &quarantine, tombstone.as_ref())?;
                    if let Some(tombstone) = &tombstone {
                        let object = tombstone.root.join("object");
                        if object.try_exists().map_err(|error| error.to_string())? {
                            let confirmed = inspections
                                .iter()
                                .find(|inspection| inspection.target.scope == *scope)
                                .ok_or_else(|| {
                                    format!("scope {scope} lacks a confirmed worktree inspection")
                                })?;
                            verify_retained_worktree(identity, object, &confirmed.snapshot).await?;
                        }
                    }
                    let active = path.try_exists().map_err(|error| error.to_string())?;
                    let quarantined = quarantine.try_exists().map_err(|error| error.to_string())?;
                    if active && quarantined {
                        return Err(format!("scope {scope} has ambiguous worktree locations"));
                    }
                    let selected = if active {
                        Some(path)
                    } else if quarantined {
                        Some(quarantine)
                    } else {
                        None
                    };
                    if let Some(path) = selected {
                        if observe_worktree_fingerprint(&path).as_deref()
                            != Some(identity.fingerprint().as_str())
                        {
                            return Err(format!(
                                "scope {scope} captured worktree fingerprint changed"
                            ));
                        }
                        if planned
                            .iter()
                            .any(|effect| effect.resource.kind() == RetiredResourceKind::Worktree)
                        {
                            let confirmed = inspections
                                .iter()
                                .find(|inspection| inspection.target.scope == *scope)
                                .ok_or_else(|| {
                                    format!("scope {scope} lacks a confirmed worktree inspection")
                                })?;
                            ensure_no_ignored_content(&path)?;
                            let (fresh, losses) = inspect_worktree_at(identity, path).await?;
                            if fresh.fingerprint() != confirmed.snapshot.fingerprint()
                                || !losses.is_empty()
                            {
                                return Err(format!(
                                    "scope {scope} worktree is not unchanged and reconstructible"
                                ));
                            }
                        }
                        observations.push(format!(
                            "{scope}: captured worktree incarnation {} observed",
                            identity.fingerprint().as_str()
                        ));
                    } else {
                        let target = expected
                            .iter()
                            .find(|target| {
                                target.scope == *scope
                                    && target.resource.kind() == RetiredResourceKind::Worktree
                            })
                            .ok_or_else(|| {
                                format!("scope {scope} lacks captured worktree target")
                            })?;
                        let prior_success = retired.iter().any(|proof| {
                            proof.scope == *scope
                                && proof.resource == target.resource
                                && matches!(
                                    proof.outcome,
                                    RetirementOutcome::Retired
                                        | RetirementOutcome::AbsenceAdopted { .. }
                                )
                        }) || self
                            .db()
                            .close_resource_has_retry_success(attempt_id, scope, &target.resource)
                            .await
                            .map_err(|error| error.to_string())?;
                        let dispatched = self
                            .db()
                            .close_retirement_resource_was_dispatched(
                                attempt_id,
                                scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        let cleanup = self
                            .db()
                            .close_worktree_cleanup_plan(
                                attempt_id,
                                scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        if !prior_success && (!dispatched || cleanup.is_none()) {
                            return Err(format!(
                                "scope {scope} absent worktree has no exact prior proof or cleanup authority"
                            ));
                        }
                        observations.push(format!(
                            "{scope}: absence bound to original captured worktree"
                        ));
                    }
                }
                None => {}
            }
            for effect in planned {
                match effect.resource.kind() {
                    RetiredResourceKind::Worktree => {
                        if !matches!(&captured.captured_worktree, Some(CapturedWorktreeIdentity::Resolved(id))
                            if effect.resource.identity() == &LossItemIdentity::Worktree(id.clone()))
                        {
                            return Err(format!(
                                "scope {scope} worktree retry lacks its exact captured identity"
                            ));
                        }
                    }
                    RetiredResourceKind::TmuxServer => {
                        let LossItemIdentity::Opaque(value) = effect.resource.identity() else {
                            return Err("tmux identity must be opaque".into());
                        };
                        let identity =
                            TmuxServerInstanceIdentity::parse_stable_identity(value.as_str())
                                .ok_or_else(|| "malformed exact tmux identity".to_string())?;
                        let legacy_path = match &captured.captured_worktree {
                            Some(CapturedWorktreeIdentity::Resolved(id)) => Some(worktree_path(id)),
                            _ => None,
                        };
                        let discovery = self
                            .tmux_registry()
                            .discover_persistent_identity(
                                &ResourceScopeKey::Work(scope.clone()),
                                legacy_path.as_deref(),
                                None,
                                phoenix_tools::tmux::registry::close_deadline(),
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        match discovery {
                            PersistentTmuxDiscovery::Exact(observed) if observed == identity => {
                                observations.push(format!(
                                    "{scope}: exact tmux {} observed",
                                    value.as_str()
                                ));
                            }
                            PersistentTmuxDiscovery::EndpointAbsent
                            | PersistentTmuxDiscovery::ServerAbsent => {
                                if !self
                                    .db()
                                    .close_retirement_resource_was_dispatched(
                                        attempt_id,
                                        scope,
                                        snapshot,
                                        &effect.resource,
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?
                                {
                                    return Err(format!(
                                        "scope {scope} tmux absence lacks original dispatch"
                                    ));
                                }
                                observations.push(format!(
                                    "{scope}: dispatched exact tmux {} absent",
                                    value.as_str()
                                ));
                            }
                            _ => {
                                return Err(format!(
                                    "scope {scope} tmux identity differs or is ambiguous"
                                ));
                            }
                        }
                    }
                    RetiredResourceKind::WorkScope => {
                        if effect.resource.identity()
                            != &LossItemIdentity::Opaque(
                                OpaqueIdentity::parse(scope.as_str())
                                    .map_err(|error| error.to_string())?,
                            )
                        {
                            return Err(format!(
                                "scope {scope} retry identity differs from captured scope"
                            ));
                        }
                        observations
                            .push(format!("{scope}: original work-scope ownership checked"));
                    }
                    _ => {
                        return Err("process-epoch retry effects require separate authority".into());
                    }
                }
            }
        }
        if observations.is_empty() {
            return Err("safe retry has no verifiable scope observations".into());
        }
        Ok(observations.join("; "))
    }

    async fn execute_close_safe_retry(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        plan: &[CloseSafeRetryEffect],
    ) -> Result<(), String> {
        for index in close_safe_retry_execution_order(plan) {
            let effect = &plan[index];
            if effect.resource.kind() != RetiredResourceKind::TmuxServer {
                continue;
            }
            match self.retry_close_tmux(run, snapshot, effect).await {
                Ok(()) => {}
                Err(CloseSafeRetryTmuxError::Failed(detail)) => {
                    return self
                        .terminalize_close_safe_retry_failure(run, snapshot, effect, &detail)
                        .await;
                }
                Err(CloseSafeRetryTmuxError::PersistenceOrUncertain(detail)) => {
                    return Err(format!(
                        "retry tmux effect may have completed but exact success was not durably established; startup observation will classify the still-running run: {detail}"
                    ));
                }
            }
        }
        let durable_plan = plan
            .iter()
            .filter(|effect| {
                matches!(
                    effect.resource.kind(),
                    RetiredResourceKind::Worktree | RetiredResourceKind::WorkScope
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if durable_plan.is_empty() {
            return Ok(());
        }
        if let Err(detail) = self
            .retire_close_worktrees_and_scopes(run, snapshot, Some(&durable_plan))
            .await
        {
            let retained = self
                .db()
                .get_close_run(run)
                .await
                .map_err(|error| error.to_string())?;
            if retained.status == CloseRunStatus::Running {
                return Err(format!(
                    "retry execution stopped before further effects; startup observation will classify the still-running exact run: {detail}"
                ));
            }
            return Err(detail);
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn retry_close_tmux(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        effect: &CloseSafeRetryEffect,
    ) -> Result<(), CloseSafeRetryTmuxError> {
        let LossItemIdentity::Opaque(value) = effect.resource.identity() else {
            return Err(CloseSafeRetryTmuxError::Failed(
                "tmux retry identity is not opaque".into(),
            ));
        };
        let identity = TmuxServerInstanceIdentity::parse_stable_identity(value.as_str())
            .ok_or_else(|| {
                CloseSafeRetryTmuxError::Failed("tmux retry identity is malformed".to_string())
            })?;
        let key = ResourceScopeKey::Work(effect.scope.clone());
        let captured = self
            .db()
            .list_close_attempt_scopes(run.attempt_id.as_str())
            .await
            .map_err(|error| CloseSafeRetryTmuxError::Failed(error.to_string()))?
            .into_iter()
            .find(|captured| captured.scope == effect.scope)
            .ok_or_else(|| {
                CloseSafeRetryTmuxError::Failed("tmux retry scope was not captured".to_string())
            })?;
        let legacy_path = match captured.captured_worktree {
            Some(CapturedWorktreeIdentity::Resolved(id)) => Some(worktree_path(&id)),
            _ => None,
        };
        let discovery = self
            .tmux_registry()
            .discover_persistent_identity(
                &key,
                legacy_path.as_deref(),
                None,
                phoenix_tools::tmux::registry::close_deadline(),
            )
            .await
            .map_err(|error| CloseSafeRetryTmuxError::Failed(error.to_string()))?;
        if !matches!(discovery, PersistentTmuxDiscovery::Exact(ref observed) if observed == &identity)
            && !matches!(
                discovery,
                PersistentTmuxDiscovery::EndpointAbsent | PersistentTmuxDiscovery::ServerAbsent
            )
        {
            return Err(CloseSafeRetryTmuxError::Failed(
                "tmux retry discovery found an ambiguous or different server".into(),
            ));
        }
        let rehydrated = self
            .tmux_registry()
            .rehydrate_retirement(
                &key,
                &identity,
                phoenix_tools::tmux::registry::close_deadline(),
            )
            .await
            .map_err(|error| CloseSafeRetryTmuxError::Failed(error.to_string()))?;
        match rehydrated {
            TmuxRetirementRehydration::Permit(permit) => {
                let outcome = self
                    .tmux_registry()
                    .complete_retirement(&permit)
                    .await
                    .map_err(|error| {
                        CloseSafeRetryTmuxError::PersistenceOrUncertain(error.to_string())
                    })?;
                match tmux_retirement_outcome(outcome) {
                    Ok(RetirementOutcome::Retired) => self
                        .record_close_retired(
                            run,
                            snapshot,
                            &effect.scope,
                            effect.resource.clone(),
                            "exact retry tmux permit retirement",
                        )
                        .await
                        .map_err(CloseSafeRetryTmuxError::PersistenceOrUncertain),
                    Ok(RetirementOutcome::AbsenceAdopted { .. }) => self
                        .record_close_absence_adopted(
                            run,
                            snapshot,
                            &effect.scope,
                            effect.resource.clone(),
                            "exact retry tmux permit absence",
                        )
                        .await
                        .map_err(CloseSafeRetryTmuxError::PersistenceOrUncertain),
                    Ok(RetirementOutcome::Residual { .. }) => Err(CloseSafeRetryTmuxError::Failed(
                        "tmux retry remains residual".into(),
                    )),
                    Err((_, detail)) => Err(CloseSafeRetryTmuxError::Failed(detail)),
                }
            }
            TmuxRetirementRehydration::AbsenceVerified => self
                .record_close_absence_adopted(
                    run,
                    snapshot,
                    &effect.scope,
                    effect.resource.clone(),
                    "exact retry tmux absence",
                )
                .await
                .map_err(CloseSafeRetryTmuxError::PersistenceOrUncertain),
            TmuxRetirementRehydration::Residual { reason } => {
                Err(CloseSafeRetryTmuxError::Failed(reason))
            }
        }
    }

    async fn terminalize_close_safe_retry_failure(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        failed: &CloseSafeRetryEffect,
        detail: &str,
    ) -> Result<(), String> {
        let progress = self
            .db()
            .close_safe_retry_progress(run)
            .await
            .map_err(|error| error.to_string())?;
        if !progress.pending.contains(failed) {
            return Err("retry failure is not a pending exact-run effect".into());
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
        self.terminalize_close_cleanup_failure(
            run,
            CloseCleanupFailureAuthority::ExpectedResource {
                scope: failed.scope.clone(),
                snapshot: snapshot.clone(),
                resource: failed.resource.clone(),
            },
            remaining_resources,
            RetirementFailureReason::IdentityNotProven,
            detail,
            CloseStopCertainty::ShutdownUncertain,
        )
        .await
    }

    async fn validate_close_worktrees_before_runtime_retirement(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        targets: &[CloseExpectedRetirementResource],
        retired: &std::collections::BTreeSet<(WorkScopeId, (String, String))>,
    ) -> Result<(), String> {
        let attempt_id = &run.attempt_id;
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
                        run,
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
                            run,
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
                        run,
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
                            run,
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
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        plan: Option<&[CloseSafeRetryEffect]>,
    ) -> Result<(), String> {
        let attempt_id = &run.attempt_id;
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
            if plan.is_some_and(|plan| !plan.iter().any(|effect| effect.scope == scope)) {
                continue;
            }
            let worktree_target = targets.iter().find(|target| {
                target.scope == scope
                    && target.resource.kind() == RetiredResourceKind::Worktree
                    && plan.is_none_or(|plan| {
                        plan.iter().any(|effect| {
                            effect.scope == scope && effect.resource == target.resource
                        })
                    })
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
                    .record_close_cleanup_failure(
                        run,
                        snapshot,
                        &scope,
                        resource,
                        RetirementFailureReason::IdentityNotProven,
                        "work scope has unresolved ProductConversation ownership",
                    )
                    .await;
            }
            if let Some(target) = worktree_target {
                'worktree_cleanup: {
                    if plan.is_some()
                        || !retired.contains(&(scope.clone(), resource_key(&target.resource)))
                    {
                        let identity = match &captured.captured_worktree {
                            Some(CapturedWorktreeIdentity::Resolved(identity)) => identity,
                            Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                                return self
                                    .record_close_cleanup_failure(
                                        run,
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
                                .record_close_cleanup_failure(
                                    run,
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
                        let quarantine_path = match worktree_quarantine_path(identity) {
                            Ok(path) => path,
                            Err(detail) => {
                                return self
                                    .record_close_cleanup_failure(
                                        run,
                                        snapshot,
                                        &scope,
                                        target.resource.clone(),
                                        RetirementFailureReason::IdentityNotProven,
                                        &detail,
                                    )
                                    .await;
                            }
                        };
                        let existing_cleanup_plan = self
                            .db()
                            .close_worktree_cleanup_plan(
                                attempt_id,
                                &scope,
                                snapshot,
                                &target.resource,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        if let Some(cleanup_plan) = existing_cleanup_plan
                            .as_ref()
                            .filter(|plan| plan.final_tombstone.is_some())
                        {
                            let confirmed = self
                                .db()
                                .list_close_retirement_inspections(attempt_id.as_str())
                                .await
                                .map_err(|error| error.to_string())?
                                .into_iter()
                                .find(|inspection| inspection.target.scope == scope)
                                .ok_or_else(|| {
                                    "final tombstone lacks confirmed worktree inspection"
                                        .to_string()
                                })?;
                            let confirmed_snapshot = confirmed.snapshot;
                            let identity = identity.clone();
                            let cleanup_plan = cleanup_plan.clone();
                            let recovery = tokio::task::spawn_blocking(move || {
                                match resume_final_worktree_tombstone(
                                    cleanup_plan
                                        .final_tombstone
                                        .as_ref()
                                        .expect("filtered above"),
                                    &identity,
                                    &confirmed_snapshot,
                                ) {
                                    FinalTombstoneRecovery::Completed => {}
                                    FinalTombstoneRecovery::Residual(detail) => return Err(detail),
                                }
                                complete_persisted_worktree_administrative_cleanup(
                                    &identity,
                                    &cleanup_plan.administrative_dir,
                                    &cleanup_plan.administrative_dir_incarnation,
                                )
                            })
                            .await;
                            let recovery = match recovery {
                                Ok(recovery) => recovery,
                                Err(error) => {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            &format!("worktree cleanup task failed: {error}"),
                                        )
                                        .await;
                                }
                            };
                            if let Err(detail) = recovery {
                                return self
                                    .record_close_cleanup_failure(
                                        run,
                                        snapshot,
                                        &scope,
                                        target.resource.clone(),
                                        RetirementFailureReason::IdentityNotProven,
                                        &detail,
                                    )
                                    .await;
                            }
                            self.record_close_absence_adopted(
                            run,
                            snapshot,
                            &scope,
                            target.resource.clone(),
                            "resumed the exact recorded private final tombstone and administrative cleanup",
                        )
                        .await?;
                            break 'worktree_cleanup;
                        }
                        let worktree_absent =
                            match both_worktree_paths_absent(&captured_path, &quarantine_path) {
                                Ok(absent) => absent,
                                Err(detail) => {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
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
                                    .record_close_cleanup_failure(
                                        run,
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
                                .record_close_cleanup_failure(
                                    run,
                                    snapshot,
                                    &scope,
                                    target.resource.clone(),
                                    RetirementFailureReason::IdentityNotProven,
                                    "absent worktree lacks an exact durable retirement dispatch",
                                )
                                .await;
                            }
                            let confirmed = self
                                .db()
                                .list_close_retirement_inspections(attempt_id.as_str())
                                .await
                                .map_err(|error| error.to_string())?
                                .into_iter()
                                .find(|inspection| inspection.target.scope == scope)
                                .ok_or_else(|| {
                                    "final tombstone lacks confirmed worktree inspection"
                                        .to_string()
                                })?;
                            let confirmed_snapshot = confirmed.snapshot;
                            let identity = identity.clone();
                            let recovery = tokio::task::spawn_blocking(move || {
                                if let Some(tombstone) = &cleanup_plan.final_tombstone {
                                    match resume_final_worktree_tombstone(
                                        tombstone,
                                        &identity,
                                        &confirmed_snapshot,
                                    ) {
                                        FinalTombstoneRecovery::Completed => {}
                                        FinalTombstoneRecovery::Residual(detail) => {
                                            return Err(detail);
                                        }
                                    }
                                }
                                complete_persisted_worktree_administrative_cleanup(
                                    &identity,
                                    &cleanup_plan.administrative_dir,
                                    &cleanup_plan.administrative_dir_incarnation,
                                )
                            })
                            .await;
                            let recovery = match recovery {
                                Ok(recovery) => recovery,
                                Err(error) => {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            &format!("worktree cleanup task failed: {error}"),
                                        )
                                        .await;
                                }
                            };
                            if let Err(detail) = recovery {
                                return self
                                    .record_close_cleanup_failure(
                                        run,
                                        snapshot,
                                        &scope,
                                        target.resource.clone(),
                                        RetirementFailureReason::IdentityNotProven,
                                        &detail,
                                    )
                                    .await;
                            }
                            self.record_close_absence_adopted(
                            run,
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
                                    .record_close_cleanup_failure(
                                        run,
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
                                .record_close_cleanup_failure(
                                    run,
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
                            let cleanup_plan = if let Some(plan) = existing_cleanup_plan {
                                plan
                            } else {
                                let identity = identity.clone();
                                let discovered = tokio::task::spawn_blocking(move || {
                                    let path = worktree_path(&identity);
                                    let quarantine = worktree_quarantine_path(&identity)?;
                                    let inspection_path =
                                        if path.exists() { &path } else { &quarantine };
                                    let common = exact_worktree_common_git_dir(inspection_path)?;
                                    let administrative_dir = exact_worktree_administrative_dir(
                                        inspection_path,
                                        &common,
                                    )?;
                                    let administrative_dir_incarnation =
                                        observe_administrative_dir_incarnation(
                                            &administrative_dir,
                                        )?;
                                    Ok::<_, String>((
                                        administrative_dir,
                                        administrative_dir_incarnation,
                                    ))
                                })
                                .await;
                                let discovered = match discovered {
                                    Ok(Ok(discovered)) => discovered,
                                    Ok(Err(detail)) => {
                                        return self
                                            .record_close_cleanup_failure(
                                                run,
                                                snapshot,
                                                &scope,
                                                target.resource.clone(),
                                                RetirementFailureReason::IdentityNotProven,
                                                &detail,
                                            )
                                            .await;
                                    }
                                    Err(error) => {
                                        return self
                                            .record_close_cleanup_failure(
                                                run,
                                                snapshot,
                                                &scope,
                                                target.resource.clone(),
                                                RetirementFailureReason::IdentityNotProven,
                                                &format!(
                                                    "worktree cleanup-plan task failed: {error}"
                                                ),
                                            )
                                            .await;
                                    }
                                };
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
                                    final_tombstone: None,
                                }
                            };
                            let identity = identity.clone();
                            let confirmed_snapshot = confirmed.snapshot.clone();
                            let db = self.db().clone();
                            let attempt = attempt_id.clone();
                            let scope_for_tombstone = scope.clone();
                            let resource_for_tombstone = target.resource.clone();
                            let snapshot_for_tombstone = snapshot.clone();
                            let runtime = tokio::runtime::Handle::current();
                            let persistence_runtime = runtime.clone();
                            let final_removal = tokio::task::spawn_blocking(move || {
                                inspect_and_remove_exact_worktree(
                                    &runtime,
                                    &identity,
                                    &confirmed_snapshot,
                                    &cleanup_plan.administrative_dir,
                                    &cleanup_plan.administrative_dir_incarnation,
                                    cleanup_plan.final_tombstone.as_ref(),
                                    move |root, (device, inode), object| {
                                        if let Some((object_device, object_inode)) = object {
                                            persistence_runtime
                                            .block_on(db.bind_close_worktree_final_tombstone_object(
                                                BindCloseWorktreeFinalTombstoneObjectRequest {
                                                    attempt_id: attempt.clone(),
                                                    scope: scope_for_tombstone.clone(),
                                                    snapshot: snapshot_for_tombstone.clone(),
                                                    resource: resource_for_tombstone.clone(),
                                                    object_device,
                                                    object_inode,
                                                },
                                            ))
                                            .map_err(|error| error.to_string())
                                        } else {
                                            persistence_runtime
                                                .block_on(db.bind_close_worktree_final_tombstone(
                                                    BindCloseWorktreeFinalTombstoneRequest {
                                                        attempt_id: attempt.clone(),
                                                        scope: scope_for_tombstone.clone(),
                                                        snapshot: snapshot_for_tombstone.clone(),
                                                        resource: resource_for_tombstone.clone(),
                                                        tombstone: CloseWorktreeFinalTombstone {
                                                            root: root.to_path_buf(),
                                                            device,
                                                            inode,
                                                            object_device: None,
                                                            object_inode: None,
                                                        },
                                                    },
                                                ))
                                                .map_err(|error| error.to_string())
                                        }
                                    },
                                )
                            })
                            .await;
                            let final_removal = match final_removal {
                                Ok(final_removal) => final_removal,
                                Err(error) => {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            &format!("worktree removal task failed: {error}"),
                                        )
                                        .await;
                                }
                            };
                            let fresh_snapshot: Option<CloseRetirementSnapshot> =
                                match final_removal {
                                    Ok(ExactWorktreeRemoval::Retired) => {
                                        self.record_close_retired(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            "exact captured Git worktree removal",
                                        )
                                        .await?;
                                        None
                                    }
                                    Ok(ExactWorktreeRemoval::StopFailed { detail }) => {
                                        return self
                                            .record_close_cleanup_failure(
                                                run,
                                                snapshot,
                                                &scope,
                                                target.resource.clone(),
                                                RetirementFailureReason::RemovalFailed,
                                                &detail,
                                            )
                                            .await;
                                    }
                                    Ok(ExactWorktreeRemoval::ReinspectionRequired { detail }) => {
                                        if run.ordinal != CloseRunOrdinal::INITIAL {
                                            return self
                                                .record_close_cleanup_failure(
                                                    run,
                                                    snapshot,
                                                    &scope,
                                                    target.resource.clone(),
                                                    RetirementFailureReason::IdentityNotProven,
                                                    &detail,
                                                )
                                                .await;
                                        }
                                        self.db()
                                            .return_close_attempt_to_reinspection(attempt_id)
                                            .await
                                            .map_err(|error| error.to_string())?;
                                        Box::pin(
                                            self.inspect_close_retirement_only(attempt_id.clone()),
                                        )
                                        .await?;
                                        return Err(detail);
                                    }
                                    Ok(ExactWorktreeRemoval::Residual { detail }) => {
                                        return self
                                            .record_close_cleanup_failure(
                                                run,
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
                                            .record_close_cleanup_failure(
                                                run,
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
                                if fresh_snapshot.fingerprint() != confirmed.snapshot.fingerprint()
                                {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            "worktree changed after Close inspection confirmation",
                                        )
                                        .await;
                                }
                                self.record_close_retired(
                                    run,
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
            }
            if plan.is_some_and(|plan| {
                !plan.iter().any(|effect| {
                    effect.scope == scope
                        && effect.resource.kind() == RetiredResourceKind::WorkScope
                })
            }) {
                continue;
            }
            let Some(work_scope_target) = targets.iter().find(|target| {
                target.scope == scope
                    && target.resource.kind() == RetiredResourceKind::WorkScope
                    && plan.is_none_or(|plan| {
                        plan.iter().any(|effect| {
                            effect.scope == scope && effect.resource == target.resource
                        })
                    })
            }) else {
                return self
                    .route_close_run_to_repair(
                        run,
                        &scope,
                        RetirementFailureReason::IdentityNotProven,
                        format!("Close scope {scope} lacks mandatory WorkScope target"),
                    )
                    .await;
            };
            if plan.is_none()
                && retired.contains(&(scope.clone(), resource_key(&work_scope_target.resource)))
            {
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
                        run,
                        snapshot,
                        &scope,
                        work_scope_target.resource.clone(),
                        "exact Close WorkScope retirement",
                    )
                    .await?;
                }
                WorkScopeRetirementOutcome::Blocked(blocker) => {
                    return self
                        .record_close_cleanup_failure(
                            run,
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
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        detail: &str,
    ) -> Result<(), String> {
        if run.ordinal != CloseRunOrdinal::INITIAL {
            return self
                .db()
                .record_close_safe_retry_success(
                    run,
                    &CloseSafeRetryEffect {
                        scope: scope.clone(),
                        resource,
                    },
                    detail,
                )
                .await
                .map_err(|error| error.to_string());
        }
        let attempt_id = &run.attempt_id;
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
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        detail: &str,
    ) -> Result<(), String> {
        if run.ordinal != CloseRunOrdinal::INITIAL {
            return self
                .db()
                .record_close_safe_retry_success(
                    run,
                    &CloseSafeRetryEffect {
                        scope: scope.clone(),
                        resource,
                    },
                    detail,
                )
                .await
                .map_err(|error| error.to_string());
        }
        let attempt_id = &run.attempt_id;
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

    async fn append_live_process_remaining_resources(
        &self,
        run: &CloseRunRef,
        remaining: &mut Vec<CloseCleanupFailureResource>,
    ) -> Result<(), String> {
        let attempt_id = &run.attempt_id;
        let successes = self
            .db()
            .list_close_process_step_successes(run)
            .await
            .map_err(|error| error.to_string())?;
        let leases = self.close_retirement_leases.lock().await;
        for ((lease_attempt, scope), lease) in leases.iter() {
            if lease_attempt != attempt_id.as_str() {
                continue;
            }
            for resource in lease.resources.iter().filter(|resource| {
                matches!(
                    resource.kind(),
                    RetiredResourceKind::BashProcessGroup
                        | RetiredResourceKind::PtySession
                        | RetiredResourceKind::BrowserSession
                        | RetiredResourceKind::EquivalentLiveResource
                )
            }) {
                if !remaining
                    .iter()
                    .any(|item| item.scope == *scope && item.resource == *resource)
                {
                    remaining.push(CloseCleanupFailureResource {
                        scope: scope.clone(),
                        resource: resource.clone(),
                        disposition: CloseCleanupResourceDisposition::Unknown,
                    });
                }
            }
        }
        remaining.retain(|item| {
            !successes.iter().any(|success| {
                success.scope == item.scope
                    && success.resource_kind.as_str() == item.resource.kind().as_str()
                    && item.resource.identity()
                        == &LossItemIdentity::Opaque(success.identity.clone())
            })
        });
        Ok(())
    }

    fn order_close_failure_resources(
        authority_scope: &WorkScopeId,
        authority_resource: &RetiredResourceIdentity,
        resources: &mut [CloseCleanupFailureResource],
    ) {
        resources.sort_by(|left, right| {
            let left_authority =
                left.scope == *authority_scope && left.resource == *authority_resource;
            let right_authority =
                right.scope == *authority_scope && right.resource == *authority_resource;
            right_authority.cmp(&left_authority).then_with(|| {
                (
                    left.scope.as_str(),
                    left.resource.kind().as_str(),
                    left.resource.identity().value(),
                )
                    .cmp(&(
                        right.scope.as_str(),
                        right.resource.kind().as_str(),
                        right.resource.identity().value(),
                    ))
            })
        });
    }

    async fn record_close_process_failure<T>(
        &self,
        run: &CloseRunRef,
        scope: &WorkScopeId,
        failed_resource: Option<RetiredResourceIdentity>,
        captured_resources: Vec<RetiredResourceIdentity>,
        detail: &str,
    ) -> Result<T, String> {
        let attempt_id = &run.attempt_id;
        let mut durable_remaining = self
            .db()
            .unresolved_expected_close_cleanup_resources(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let authority_resource = failed_resource.clone().unwrap_or_else(|| {
            opaque_resource(RetiredResourceKind::WorkScope, scope.as_str().to_string())
        });
        let (authority, disposition) = match failed_resource {
            Some(resource) => (
                CloseCleanupFailureAuthority::ObservedProcessResource {
                    scope: scope.clone(),
                    resource,
                },
                CloseCleanupResourceDisposition::Failed,
            ),
            None => (
                CloseCleanupFailureAuthority::CapturedScope {
                    scope: scope.clone(),
                    resource: opaque_resource(
                        RetiredResourceKind::WorkScope,
                        scope.as_str().to_string(),
                    ),
                },
                CloseCleanupResourceDisposition::Unknown,
            ),
        };
        let mut remaining_resources = vec![CloseCleanupFailureResource {
            scope: scope.clone(),
            resource: authority_resource.clone(),
            disposition,
        }];
        durable_remaining.retain(|remaining| {
            remaining.scope != *scope || remaining.resource != authority_resource
        });
        remaining_resources.append(&mut durable_remaining);
        for resource in captured_resources {
            if !remaining_resources
                .iter()
                .any(|remaining| remaining.resource == resource)
            {
                remaining_resources.push(CloseCleanupFailureResource {
                    scope: scope.clone(),
                    resource,
                    disposition: CloseCleanupResourceDisposition::Unknown,
                });
            }
        }
        self.append_live_process_remaining_resources(run, &mut remaining_resources)
            .await?;
        Self::order_close_failure_resources(scope, &authority_resource, &mut remaining_resources);
        self.terminalize_close_cleanup_failure(
            run,
            authority,
            remaining_resources,
            RetirementFailureReason::IdentityNotProven,
            detail,
            CloseStopCertainty::ShutdownUncertain,
        )
        .await
    }

    pub(crate) async fn route_close_attempt_to_repair<T>(
        &self,
        attempt_id: &CloseAttemptId,
        scope: &WorkScopeId,
        reason: RetirementFailureReason,
        detail: impl Into<String>,
    ) -> Result<T, String> {
        self.route_close_run_to_repair(
            &CloseRunRef::initial(attempt_id.clone()),
            scope,
            reason,
            detail,
        )
        .await
    }

    async fn route_close_run_to_repair<T>(
        &self,
        run: &CloseRunRef,
        scope: &WorkScopeId,
        reason: RetirementFailureReason,
        detail: impl Into<String>,
    ) -> Result<T, String> {
        let detail = detail.into();
        if run.ordinal != CloseRunOrdinal::INITIAL {
            let progress = self
                .db()
                .close_safe_retry_progress(run)
                .await
                .map_err(|error| error.to_string())?;
            let failed = progress
                .pending
                .iter()
                .find(|effect| effect.scope == *scope)
                .ok_or_else(|| {
                    "retry repair has no exact pending effect in the scope".to_string()
                })?;
            let snapshot = self
                .db()
                .get_close_obligation(run.attempt_id.as_str())
                .await
                .map_err(|error| error.to_string())?
                .snapshot()
                .cloned()
                .ok_or_else(|| "retry repair lacks original retirement snapshot".to_string())?;
            return self
                .terminalize_known_close_retry_failure(
                    run,
                    &snapshot,
                    scope,
                    failed.resource.clone(),
                    reason,
                    &detail,
                    CloseStopCertainty::ShutdownUncertain,
                )
                .await;
        }
        let attempt_id = &run.attempt_id;
        let captured = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|captured| captured.scope == *scope)
            .ok_or_else(|| format!("Close failure scope {scope} was not captured"))?;
        let residual = match captured.captured_worktree {
            Some(CapturedWorktreeIdentity::Resolved(identity)) => RetiredResourceIdentity::parse(
                RetiredResourceKind::Worktree,
                LossItemIdentity::Worktree(identity),
            ),
            None | Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::WorkScope,
                    LossItemIdentity::Opaque(
                        OpaqueIdentity::parse(scope.as_str().to_string())
                            .map_err(|error| error.to_string())?,
                    ),
                )
            }
        }
        .map_err(|error| error.to_string())?;
        let authority = CloseCleanupFailureAuthority::CapturedScope {
            scope: scope.clone(),
            resource: residual.clone(),
        };
        let mut remaining_resources = self
            .db()
            .unresolved_expected_close_cleanup_resources(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        remaining_resources
            .retain(|remaining| remaining.scope != *scope || remaining.resource != residual);
        remaining_resources.push(CloseCleanupFailureResource {
            scope: scope.clone(),
            resource: residual.clone(),
            disposition: CloseCleanupResourceDisposition::Failed,
        });
        self.append_live_process_remaining_resources(run, &mut remaining_resources)
            .await?;
        Self::order_close_failure_resources(scope, &residual, &mut remaining_resources);
        self.terminalize_close_cleanup_failure(
            run,
            authority,
            remaining_resources,
            reason,
            &detail,
            CloseStopCertainty::ShutdownUncertain,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn terminalize_known_close_retry_failure<T>(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        reason: RetirementFailureReason,
        detail: &str,
        stop_certainty: CloseStopCertainty,
    ) -> Result<T, String> {
        let progress = self
            .db()
            .close_safe_retry_progress(run)
            .await
            .map_err(|error| error.to_string())?;
        let failed = CloseSafeRetryEffect {
            scope: scope.clone(),
            resource: resource.clone(),
        };
        if !progress.pending.contains(&failed) {
            return Err("known retry failure is not an exact pending effect".into());
        }
        let remaining_resources = progress
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
        self.terminalize_close_cleanup_failure(
            run,
            CloseCleanupFailureAuthority::ExpectedResource {
                scope: scope.clone(),
                snapshot: snapshot.clone(),
                resource,
            },
            remaining_resources,
            reason,
            detail,
            stop_certainty,
        )
        .await
    }

    async fn record_close_residual<T>(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        reason: RetirementFailureReason,
        detail: &str,
    ) -> Result<T, String> {
        if run.ordinal != CloseRunOrdinal::INITIAL {
            return self
                .terminalize_known_close_retry_failure(
                    run,
                    snapshot,
                    scope,
                    resource,
                    reason,
                    detail,
                    CloseStopCertainty::ShutdownUncertain,
                )
                .await;
        }
        let attempt_id = &run.attempt_id;
        let mut remaining_resources = self
            .db()
            .expected_close_cleanup_failure_resources(attempt_id.as_str(), scope, &resource)
            .await
            .map_err(|error| error.to_string())?;
        self.append_live_process_remaining_resources(run, &mut remaining_resources)
            .await?;
        Self::order_close_failure_resources(scope, &resource, &mut remaining_resources);
        self.terminalize_close_cleanup_failure(
            run,
            CloseCleanupFailureAuthority::ExpectedResource {
                scope: scope.clone(),
                snapshot: snapshot.clone(),
                resource,
            },
            remaining_resources,
            reason,
            detail,
            CloseStopCertainty::ShutdownUncertain,
        )
        .await
    }

    async fn record_close_cleanup_failure<T>(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        resource: RetiredResourceIdentity,
        reason: RetirementFailureReason,
        detail: &str,
    ) -> Result<T, String> {
        if run.ordinal != CloseRunOrdinal::INITIAL {
            return self
                .terminalize_known_close_retry_failure(
                    run,
                    snapshot,
                    scope,
                    resource,
                    reason,
                    detail,
                    CloseStopCertainty::ConversationAndProcessesStopped {
                        confirmed_at_us: chrono::Utc::now().timestamp_micros(),
                    },
                )
                .await;
        }
        let attempt_id = &run.attempt_id;
        let mut remaining_resources = self
            .db()
            .expected_close_cleanup_failure_resources(attempt_id.as_str(), scope, &resource)
            .await
            .map_err(|error| error.to_string())?;
        self.append_live_process_remaining_resources(run, &mut remaining_resources)
            .await?;
        Self::order_close_failure_resources(scope, &resource, &mut remaining_resources);
        self.terminalize_close_cleanup_failure(
            run,
            CloseCleanupFailureAuthority::ExpectedResource {
                scope: scope.clone(),
                snapshot: snapshot.clone(),
                resource,
            },
            remaining_resources,
            reason,
            detail,
            CloseStopCertainty::ConversationAndProcessesStopped {
                confirmed_at_us: chrono::Utc::now().timestamp_micros(),
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn terminalize_close_cleanup_failure<T>(
        &self,
        run: &CloseRunRef,
        authority: CloseCleanupFailureAuthority,
        remaining_resources: Vec<CloseCleanupFailureResource>,
        reason: RetirementFailureReason,
        detail: &str,
        stop_certainty: CloseStopCertainty,
    ) -> Result<T, String> {
        let attempt_id = &run.attempt_id;
        let obligation = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        let failure_occurrence_id = run.failure_occurrence_id();
        let persisted_timing = self
            .db()
            .close_cleanup_failure_timing(&failure_occurrence_id)
            .await
            .map_err(|error| error.to_string())?;
        let occurred_at_us = persisted_timing
            .map(|(occurred_at_us, _)| occurred_at_us)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
        let stop_certainty = match (stop_certainty, persisted_timing) {
            (
                CloseStopCertainty::ConversationAndProcessesStopped { .. },
                Some((_, Some(confirmed_at_us))),
            ) => CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us },
            (certainty, _) => certainty,
        };
        self.commit_close_cleanup_failure(
            run,
            &TerminalizeInitialCloseCleanupFailureRequest {
                failure_occurrence_id,
                attempt_id: attempt_id.clone(),
                source_product_conversation_id: obligation.product_conversation_id().clone(),
                authority,
                remaining_resources,
                reason,
                detail: detail.to_string(),
                stop_certainty,
                occurred_at_us,
            },
        )
        .await
    }

    async fn commit_close_cleanup_failure<T>(
        &self,
        run: &CloseRunRef,
        request: &TerminalizeInitialCloseCleanupFailureRequest,
    ) -> Result<T, String> {
        let broadcasters = self
            .close_retirement_broadcasters(&request.attempt_id)
            .await?;
        let completed = self
            .db()
            .terminalize_close_run_cleanup_failure(run, request)
            .await
            .map_err(|error| error.to_string())?;
        self.discard_close_resource_leases(&request.attempt_id)
            .await;
        match completed.close_outcome() {
            Some(CloseCompletionOutcome::ArchivedCleanupAttention) => {
                Self::publish_close_retirement_updates(broadcasters, true);
            }
            Some(CloseCompletionOutcome::CloseIncomplete) => {
                Self::publish_close_retirement_updates(broadcasters, false);
            }
            Some(CloseCompletionOutcome::Archived | CloseCompletionOutcome::Cancelled) | None => {}
        }
        self.kick_direct_turn_worker();
        Err(request.detail.clone())
    }

    async fn persist_close_process_step(
        &self,
        run: &CloseRunRef,
        scope: &WorkScopeId,
        resource_kind: CloseProcessResourceKind,
        identities: Vec<String>,
        outcome: CloseProcessStepOutcome,
    ) -> Result<(), CloseLeaseFailure> {
        let observed_at_us = chrono::Utc::now().timestamp_micros();
        for identity in identities {
            self.db()
                .record_close_process_step_success(&CloseProcessStepSuccess {
                    run: run.clone(),
                    scope: scope.clone(),
                    resource_kind,
                    identity: OpaqueIdentity::parse(identity)
                        .expect("registry stable instance identity is non-empty"),
                    outcome,
                    observed_at_us,
                })
                .await
                .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
        }
        Ok(())
    }

    /// Completes one live lease.
    #[allow(clippy::too_many_lines)]
    async fn complete_close_resource_lease(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        expected: &[RetiredResourceIdentity],
    ) -> Result<(), CloseLeaseFailure> {
        let key = (run.attempt_id.as_str().to_string(), scope.clone());
        let lease = self.close_retirement_leases.lock().await.remove(&key);
        let Some(lease) = lease else {
            return Err(CloseLeaseFailure::Unavailable);
        };
        let result = async {
            let prior_process_successes = self
                .db()
                .list_close_process_step_successes(run)
                .await
                .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
            if expected.iter().any(|resource| {
                resource.kind() != RetiredResourceKind::TmuxServer
                    || !lease.resources.contains(resource)
            }) {
                return Err(CloseLeaseFailure::Tmux {
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail: "live Close lease differs from sealed unresolved inventory".to_string(),
                });
            }
            let bash_identities = lease
                .bash
                .exact_process_groups
                .iter()
                .map(phoenix_tools::bash::registry::BashRetirementTarget::stable_resource_identity)
                .collect::<Vec<_>>();
            if !process_step_successes_cover(
                &prior_process_successes,
                scope,
                CloseProcessResourceKind::BashProcessGroup,
                &bash_identities,
            ) {
            let bash_outcome = self.bash_handles().complete_retirement(&lease.bash).await;
            let bash_generation_is_stale =
                matches!(&bash_outcome, BashRetirementOutcome::StaleGeneration(_));
            let failed_bash_resources = lease
                .bash
                .exact_process_groups
                .iter()
                .filter(|target| {
                    bash_generation_is_stale
                        || bash_outcome.report().kill_failures.iter().any(|(pid, _)| {
                        *pid == target.pgid
                            || u32::try_from(*pid)
                                .ok()
                                .is_some_and(|pid| target.pid == Some(pid))
                        })
                })
                .map(|target| {
                    opaque_resource(
                        RetiredResourceKind::BashProcessGroup,
                        target.stable_resource_identity(),
                    )
                })
                .collect();
            let bash_success = require_absent(bash_outcome).map_err(|reason| {
                CloseLeaseFailure::process_epoch(
                    RetiredResourceKind::BashProcessGroup,
                    failed_bash_resources,
                    reason,
                )
            })?;
            self.persist_close_process_step(
                run,
                scope,
                CloseProcessResourceKind::BashProcessGroup,
                bash_identities,
                bash_success,
            )
            .await?;
            }
            let retired = self
                .db()
                .list_close_retirement_evidence(run.attempt_id.as_str())
                .await
                .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
            let tmux_already_complete = expected.iter().all(|resource| {
                retired.iter().any(|proof| {
                    proof.scope == *scope
                        && proof.resource == *resource
                        && matches!(
                            proof.outcome,
                            RetirementOutcome::Retired
                                | RetirementOutcome::AbsenceAdopted { .. }
                        )
                })
            });
            if !tmux_already_complete {
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
            for resource in expected {
                self.db()
                    .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                        attempt_id: run.attempt_id.clone(),
                        snapshot: snapshot.clone(),
                        scope: scope.clone(),
                        resource: resource.clone(),
                        outcome: tmux_outcome.clone(),
                        detail: Some("exact registry permit retirement".to_string()),
                    })
                    .await
                    .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
            }
            }
            let terminal_identities = lease
                .terminal
                .instance
                .iter()
                .map(phoenix_terminal::session::TerminalInstanceIdentity::stable_identity)
                .collect::<Vec<_>>();
            if !process_step_successes_cover(
                &prior_process_successes,
                scope,
                CloseProcessResourceKind::PtySession,
                &terminal_identities,
            ) {
            let terminal_success =
                require_terminal_absent(self.terminals.complete_retirement(&lease.terminal).await)
                    .map_err(|reason| {
                        CloseLeaseFailure::process_epoch(
                            RetiredResourceKind::PtySession,
                            lease
                                .resources
                                .iter()
                                .filter(|resource| {
                                    resource.kind() == RetiredResourceKind::PtySession
                                })
                                .cloned()
                                .collect(),
                            reason,
                        )
                    })?;
            self.persist_close_process_step(
                run,
                scope,
                CloseProcessResourceKind::PtySession,
                terminal_identities,
                terminal_success,
            )
            .await?;
            }
            let browser_identities = lease
                .browser
                .instances
                .iter()
                .map(phoenix_tools::browser::session::BrowserSessionInstanceIdentity::stable_identity)
                .collect::<Vec<_>>();
            if !process_step_successes_cover(
                &prior_process_successes,
                scope,
                CloseProcessResourceKind::BrowserSession,
                &browser_identities,
            ) {
            let browser_success = require_browser_absent(
                self.browser_sessions()
                    .complete_retirement(&lease.browser)
                    .await,
            )
            .map_err(|reason| {
                CloseLeaseFailure::process_epoch(
                    RetiredResourceKind::BrowserSession,
                    lease
                        .resources
                        .iter()
                        .filter(|resource| resource.kind() == RetiredResourceKind::BrowserSession)
                        .cloned()
                        .collect(),
                    reason,
                )
            })?;
            self.persist_close_process_step(
                run,
                scope,
                CloseProcessResourceKind::BrowserSession,
                browser_identities,
                browser_success,
            )
            .await?;
            }
            Ok(())
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
    let hidden_path = path.clone();
    let hidden_dirty =
        tokio::task::spawn_blocking(move || observe_hidden_worktree_changes(&hidden_path))
            .await
            .map_err(|error| error.to_string())??;
    for path in hidden_dirty {
        observation.extend_from_slice(b"HIDDEN_UNSTAGED\\0");
        observation.extend_from_slice(path.as_bytes());
        observation.push(0);
        losses.push(CloseLossItem::UnstagedTrackedPath(path));
    }
    losses.sort_by_key(|loss| (loss.category().as_str(), loss.identity().value()));
    losses.dedup();
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
        let mut symbolic = phoenix_core::git::command();
        bind_git_command_to_worktree(&mut symbolic, &path)?;
        let symbolic = symbolic
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
            .map_err(|error| error.to_string())?;
        let mut result = Vec::new();
        if symbolic.status.success() {
            result.push((
                b"HEAD_SYMBOLIC".to_vec(),
                symbolic.stdout.trim_ascii().to_vec(),
            ));
        } else {
            let mut head = phoenix_core::git::command();
            bind_git_command_to_worktree(&mut head, &path)?;
            let head = head
                .args(["rev-parse", "--verify", "HEAD"])
                .output()
                .map_err(|error| error.to_string())?;
            if !head.status.success() {
                return Err(String::from_utf8_lossy(&head.stderr).trim().to_string());
            }
            let head_oid = head.stdout.trim_ascii().to_vec();
            result.push((b"HEAD".to_vec(), head_oid.clone()));
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
                observation.extend_from_slice(b"DETACHED_UNREACHABLE\0");
                observation.extend_from_slice(&oid);
                observation.push(0);
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
    let mut symbolic = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut symbolic, repository)?;
    let detached = !symbolic
        .args(["symbolic-ref", "-q", "HEAD"])
        .status()
        .map_err(|error| error.to_string())?
        .success();
    if !detached {
        return Ok(false);
    }
    let mut head = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut head, repository)?;
    let head = head
        .args(["rev-parse", "--verify", "HEAD"])
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
    let mut stash = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut stash, repository)?;
    let stash_exists = stash
        .args(["show-ref", "--verify", "--quiet", "refs/stash"])
        .status()
        .map_err(|error| error.to_string())?
        .success();
    let mut command = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut command, repository)?;
    command.args([
        "rev-list",
        head_text,
        "--not",
        "--branches",
        "--remotes",
        "--tags",
    ]);
    if stash_exists {
        command.arg("refs/stash");
    }
    let output = command.output().map_err(|error| error.to_string())?;
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
    let mut reachable = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut reachable, repository)?;
    let reachable = reachable
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
    let mut listing = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut listing, repository)?;
    let listing = listing
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()
        .map_err(|error| error.to_string())?;
    if !listing.status.success() {
        return Err(String::from_utf8_lossy(&listing.stderr).trim().to_string());
    }
    let repository = repository
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize inspected worktree: {error}"))?;
    let mut git_dir = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut git_dir, &repository)?;
    let git_dir = git_dir
        .args(["rev-parse", "--absolute-git-dir"])
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

fn git_directory_for_worktree(repository: &Path) -> Result<PathBuf, String> {
    let dot_git = repository.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).map_err(|error| {
        format!(
            "cannot inspect Git metadata at {}: {error}",
            dot_git.display()
        )
    })?;
    if metadata.is_dir() {
        return dot_git
            .canonicalize()
            .map_err(|error| format!("cannot resolve Git directory: {error}"));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Git metadata at {} is not a file or directory",
            dot_git.display()
        ));
    }
    let pointer = std::fs::read(&dot_git)
        .map_err(|error| format!("cannot read Git metadata at {}: {error}", dot_git.display()))?;
    let pointer = pointer
        .strip_prefix(b"gitdir: ")
        .map(<[u8]>::trim_ascii_end)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            format!(
                "Git metadata at {} has no gitdir pointer",
                dot_git.display()
            )
        })?;
    let pointer = path_buf_from_git_bytes(pointer);
    let candidate = if pointer.is_absolute() {
        pointer
    } else {
        repository.join(pointer)
    };
    if let Ok(candidate) = candidate.canonicalize() {
        return Ok(candidate);
    }

    let parent_repository = repository
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join(".git").exists())
        .ok_or_else(|| {
            format!(
                "cannot resolve moved Git directory for {}",
                repository.display()
            )
        })?;
    let relative = repository
        .strip_prefix(parent_repository)
        .map_err(|error| error.to_string())?;
    let candidate = git_directory_for_worktree(parent_repository)?
        .join("modules")
        .join(relative);
    candidate.canonicalize().map_err(|error| {
        format!(
            "cannot resolve moved Git directory {}: {error}",
            candidate.display()
        )
    })
}

fn bind_git_command_to_worktree(
    command: &mut std::process::Command,
    repository: &Path,
) -> Result<(), String> {
    command
        .env("GIT_DIR", git_directory_for_worktree(repository)?)
        .env("GIT_WORK_TREE", repository)
        .current_dir(repository);
    Ok(())
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
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree(&mut command, repository)?;
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
    run_bounded_spawned_git(command, deadline, "Git status inspection")
}

fn run_bounded_git_command_until(
    repository: &Path,
    arguments: &[&str],
    index: Option<&Path>,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree(&mut command, repository)?;
    command
        .args(arguments)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    run_bounded_spawned_git(command, deadline, operation)
}

fn run_bounded_git_paths_until(
    repository: &Path,
    arguments: &[&str],
    paths: &[Vec<u8>],
    literal_pathspec: bool,
    index: &Path,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    // `Command::args` accepts path OsStrings, preserving bytes on Unix.
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree(&mut command, repository)?;
    command
        .args(arguments)
        .args(paths.iter().map(|path| path_buf_from_git_bytes(path)))
        .env("GIT_INDEX_FILE", index)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if literal_pathspec {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    run_bounded_spawned_git(command, deadline, operation)
}

fn run_bounded_spawned_git(
    mut command: std::process::Command,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    let child = command.spawn().map_err(|error| error.to_string())?;
    wait_bounded_child_output(child, deadline, operation)
}

fn wait_bounded_child_output(
    mut child: std::process::Child,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    let stdout = child.stdout.take().ok_or("Git stdout was not piped")?;
    let stderr = child.stderr.take().ok_or("Git stderr was not piped")?;
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
                    .map_err(|error| format!("Git process id overflow: {error}"))?;
                libc::kill(-process_group, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = child.kill();
            child.wait().map_err(|error| error.to_string())?;
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!("{operation} exceeded its deadline"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "Git stdout reader panicked".to_string())?
        .map_err(|error| error.to_string())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "Git stderr reader panicked".to_string())?
        .map_err(|error| error.to_string())?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

struct IndexedGitlink {
    path: GitPathIdentity,
    stage_zero_oid: Option<Vec<u8>>,
    has_unmerged_stage: bool,
}

type IndexedGitlinks = (Vec<u8>, Vec<IndexedGitlink>);

fn index_gitlinks(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<IndexedGitlinks, String> {
    let output = run_bounded_git_command_until(
        repository,
        &["ls-files", "--stage", "-z", "--"],
        None,
        deadline,
        "gitlink inspection",
    )?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect index gitlinks: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut gitlinks = std::collections::BTreeMap::<Vec<u8>, IndexedGitlink>::new();
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
        if mode == b"160000" {
            let path = git_path_from_observation(&record[separator + 1..])?;
            let gitlink =
                gitlinks
                    .entry(path.as_bytes().to_vec())
                    .or_insert_with(|| IndexedGitlink {
                        path,
                        stage_zero_oid: None,
                        has_unmerged_stage: false,
                    });
            if stage == b"0" {
                gitlink.stage_zero_oid = Some(oid.to_vec());
            } else {
                gitlink.has_unmerged_stage = true;
            }
        }
    }
    Ok((output.stdout, gitlinks.into_values().collect()))
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
    let (index_observation, gitlinks) = index_gitlinks(repository, deadline)?;
    observation.push((
        [b"SUBMODULE_GITLINK_INDEX\0".as_slice(), relative_prefix].concat(),
        index_observation,
    ));
    for gitlink in gitlinks {
        if std::time::Instant::now() >= deadline {
            return Err("Git submodule inspection exceeded its aggregate deadline".to_string());
        }
        let relative_path = join_git_paths(relative_prefix, gitlink.path.as_bytes())?;
        let submodule_path = repository.join(path_buf_from_git_bytes(gitlink.path.as_bytes()));
        if !submodule_path.is_dir() {
            continue;
        }
        let metadata_path = submodule_path.join(".git");
        match std::fs::symlink_metadata(&metadata_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let populated = std::fs::read_dir(&submodule_path)
                    .map_err(|error| {
                        format!(
                            "cannot inspect gitlink {}: {error}",
                            String::from_utf8_lossy(&relative_path)
                        )
                    })?
                    .next()
                    .transpose()
                    .map_err(|error| {
                        format!(
                            "cannot inspect gitlink {}: {error}",
                            String::from_utf8_lossy(&relative_path)
                        )
                    })?
                    .is_some();
                if populated {
                    observation.push((
                        [b"SUBMODULE_METADATA_MISSING\0".as_slice(), &relative_path].concat(),
                        gitlink.stage_zero_oid.unwrap_or_default(),
                    ));
                    observation.push((
                        [
                            b"SUBMODULE_LOSS\0submodule\0".as_slice(),
                            relative_path.as_slice(),
                        ]
                        .concat(),
                        Vec::new(),
                    ));
                }
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect git metadata for {}: {error}",
                    String::from_utf8_lossy(&relative_path)
                ));
            }
        }
        let canonical = submodule_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !visited.insert(canonical.clone()) {
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
        let mut submodule_losses = parse_status_losses(&status.stdout);
        for hidden in observe_hidden_worktree_changes(&submodule_path)? {
            let loss = CloseLossItem::UnstagedTrackedPath(hidden);
            if !submodule_losses.contains(&loss) {
                submodule_losses.push(loss);
            }
        }
        let mut submodule_status = canonical_status_observation(&status.stdout);
        submodule_status
            .extend_from_slice(&observe_dirty_content(&submodule_path, &submodule_losses)?);
        observation.push((
            [b"SUBMODULE_STATUS\0".as_slice(), relative_path.as_slice()].concat(),
            submodule_status,
        ));

        let mut symbolic_head = phoenix_core::git::command();
        bind_git_command_to_worktree(&mut symbolic_head, &submodule_path)?;
        let symbolic_head = symbolic_head
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
            .map_err(|error| error.to_string())?;
        let mut submodule_head = phoenix_core::git::command();
        bind_git_command_to_worktree(&mut submodule_head, &submodule_path)?;
        let submodule_head = submodule_head
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .map_err(|error| error.to_string())?;
        if !submodule_head.status.success() && symbolic_head.status.success() {
            observation.push((
                [
                    b"SUBMODULE_HEAD_SYMBOLIC\0".as_slice(),
                    relative_path.as_slice(),
                ]
                .concat(),
                symbolic_head.stdout.trim_ascii().to_vec(),
            ));
            visited.remove(&canonical);
            continue;
        }
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
            [
                gitlink.stage_zero_oid.as_deref().unwrap_or_default(),
                b"\0",
                submodule_head_oid,
            ]
            .concat(),
        ));
        let detached_loss =
            detached_head_is_unreachable(&submodule_path, observation, &relative_path)?;
        let gitlink_changed = gitlink.stage_zero_oid.as_deref() != Some(submodule_head_oid);
        if gitlink_changed || gitlink.has_unmerged_stage {
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
    StopFailed { detail: String },
    ReinspectionRequired { detail: String },
    Residual { detail: String },
}

#[derive(Debug)]
enum FinalTombstoneRecovery {
    Completed,
    Residual(String),
}

fn inspect_and_remove_exact_worktree<B>(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    final_tombstone: Option<&CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
) -> Result<ExactWorktreeRemoval, String>
where
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    inspect_and_remove_exact_worktree_with_hook_and_plan(
        runtime,
        identity,
        confirmed_snapshot,
        administrative_dir,
        administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
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
        None,
        |_, _, _| Ok(()),
        after_quarantine,
    )
}

#[allow(clippy::too_many_arguments)]
fn inspect_and_remove_exact_worktree_with_hook_and_plan<F, B>(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    final_tombstone: Option<&CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
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
    ensure_no_ignored_content(inspection_path)?;
    runtime.block_on(quarantine_and_remove_exact_worktree(
        identity,
        confirmed_snapshot,
        administrative_dir.to_path_buf(),
        administrative_dir_incarnation.to_string(),
        final_tombstone.cloned(),
        bind_tombstone,
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

#[cfg(target_os = "linux")]
unsafe fn renameat_exclusive(
    directory: libc::c_int,
    source: *const libc::c_char,
    destination: *const libc::c_char,
) -> libc::c_int {
    // SAFETY: caller supplies valid descriptors and NUL-terminated names.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory,
            source,
            directory,
            destination,
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        0
    } else {
        -1
    }
}

#[cfg(target_os = "macos")]
unsafe fn renameat_exclusive(
    directory: libc::c_int,
    source: *const libc::c_char,
    destination: *const libc::c_char,
) -> libc::c_int {
    // SAFETY: caller supplies valid descriptors and NUL-terminated names.
    unsafe { libc::renameatx_np(directory, source, directory, destination, libc::RENAME_EXCL) }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
unsafe fn renameat_exclusive(
    _directory: libc::c_int,
    _source: *const libc::c_char,
    _destination: *const libc::c_char,
) -> libc::c_int {
    -1
}

#[cfg(unix)]
fn bind_deletion_entry_to_private_slot(
    directory: libc::c_int,
    name: &std::ffi::CStr,
    expected: &libc::stat,
) -> Result<(std::ffi::CString, libc::stat), String> {
    let slot = std::ffi::CString::new(format!(
        ".phoenix-delete-entry-{}",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("generated deletion slot contains no NUL");
    // SAFETY: the descriptor and C strings are valid. Exclusive rename
    // cannot overwrite another entry at the unpredictable destination.
    if unsafe { renameat_exclusive(directory, name.as_ptr(), slot.as_ptr()) } < 0 {
        return Err(format!(
            "cannot bind deletion entry to private slot: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut moved = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: descriptor and slot are valid; moved points to writable storage.
    if unsafe {
        libc::fstatat(
            directory,
            slot.as_ptr(),
            moved.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(format!(
            "cannot identify deletion entry in private slot: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fstatat initialized moved on success.
    let moved = unsafe { moved.assume_init() };
    if moved.st_dev != expected.st_dev
        || moved.st_ino != expected.st_ino
        || moved.st_mode & libc::S_IFMT != expected.st_mode & libc::S_IFMT
    {
        return Err(format!(
            "deletion entry was replaced before identity binding; replacement preserved in private slot {}",
            slot.to_string_lossy()
        ));
    }
    Ok((slot, moved))
}

#[cfg(unix)]
fn remove_directory_contents_at(directory: &std::os::fd::OwnedFd) -> Result<(), String> {
    remove_directory_contents_at_with_hook(directory, &mut |_, _| {})
}

#[cfg(unix)]
#[allow(clippy::too_many_lines)]
fn remove_directory_contents_at_with_hook<H>(
    directory: &std::os::fd::OwnedFd,
    after_inspection: &mut H,
) -> Result<(), String>
where
    H: FnMut(libc::c_int, &std::ffi::CStr),
{
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
            after_inspection(directory.as_raw_fd(), name);
            let (slot, moved) =
                bind_deletion_entry_to_private_slot(directory.as_raw_fd(), name, &metadata)?;
            if moved.st_mode & libc::S_IFMT == libc::S_IFDIR {
                // SAFETY: openat does not follow the private slot because O_NOFOLLOW is set.
                let child = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        slot.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if child < 0 {
                    return Err(format!(
                        "cannot open identity-bound deletion subdirectory: {}",
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
                if opened.st_dev != moved.st_dev || opened.st_ino != moved.st_ino {
                    return Err(
                        "private deletion slot was replaced before descriptor binding".to_string(),
                    );
                }
                remove_directory_contents_at_with_hook(&child, after_inspection)?;
                // SAFETY: unlinkat targets only the unpredictable identity-checked slot.
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), slot.as_ptr(), libc::AT_REMOVEDIR)
                } < 0
                {
                    return Err(format!(
                        "cannot remove identity-bound deletion subdirectory: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            } else {
                // SAFETY: unlinkat targets only the unpredictable identity-checked slot.
                if unsafe { libc::unlinkat(directory.as_raw_fd(), slot.as_ptr(), 0) } < 0 {
                    return Err(format!(
                        "cannot remove identity-bound deletion entry: {}",
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

fn reserve_private_tombstone(root: &Path, description: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder
            .mode(0o700)
            .create(root)
            .map_err(|error| format!("cannot reserve private {description} tombstone: {error}"))
    }
    #[cfg(not(unix))]
    {
        let _ = (root, description);
        Err("identity-bound deletion is unsupported on this platform".to_string())
    }
}

#[cfg(unix)]
fn tombstone_identity(root: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|error| format!("cannot identify private final tombstone: {error}"))?;
    Ok((metadata.dev(), metadata.ino()))
}

#[allow(
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::unnecessary_cast,
    reason = "libc stat field signedness and width vary across supported Unix targets"
)]
#[allow(clippy::too_many_arguments)]
fn remove_identity_bound_directory<F, B, A, O>(
    deletion_target: &Path,
    tombstone_root: Option<&Path>,
    expected_identity: &str,
    observe_identity: O,
    before_final_move: F,
    mut bind_root: B,
    after_identity_observation: A,
    description: &str,
) -> Result<(), String>
where
    F: FnOnce(&Path),
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String>,
    A: FnOnce(&Path, &Path) -> Result<(), String>,
    O: Fn(&Path) -> Result<String, String>,
{
    before_final_move(deletion_target);
    let parent = deletion_target
        .parent()
        .ok_or_else(|| format!("{description} has no parent directory"))?;
    let tombstone_root = tombstone_root.map_or_else(
        || parent.join(format!(".phoenix-delete-{}", uuid::Uuid::new_v4().simple())),
        Path::to_path_buf,
    );
    if tombstone_root.parent() != Some(parent) {
        return Err(format!(
            "private {description} tombstone is outside deletion parent"
        ));
    }
    if !tombstone_root.exists() {
        reserve_private_tombstone(&tombstone_root, description)?;
    }
    #[cfg(not(unix))]
    {
        return Err(format!(
            "identity-bound {description} deletion is unsupported on this platform"
        ));
    }
    let tombstone = tombstone_root.join("object");
    #[cfg(unix)]
    let verified_root = tombstone_identity(&tombstone_root)
        .map_err(|error| format!("cannot identify private {description} tombstone: {error}"))?;
    if let Err(error) = bind_root(&tombstone_root, verified_root, None) {
        let _ = std::fs::remove_dir(&tombstone_root);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(deletion_target, &tombstone) {
        let _ = std::fs::remove_dir(&tombstone_root);
        return Err(format!(
            "cannot move {description} into private final tombstone: {error}"
        ));
    }
    #[cfg(unix)]
    let verified_object = {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = std::fs::symlink_metadata(&tombstone)
            .map_err(|error| format!("cannot identify identity-bound {description}: {error}"))?;
        (metadata.dev(), metadata.ino())
    };
    bind_root(&tombstone_root, verified_root, Some(verified_object))?;
    if observe_identity(&tombstone)? != expected_identity {
        return Err(format!(
            "{description} identity changed before final deletion; replacement preserved at {}",
            tombstone.display()
        ));
    }
    after_identity_observation(&tombstone_root, &tombstone)?;
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
        let mut opened_root = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: descriptor is open and fstat initializes opened_root on success.
        if unsafe {
            libc::fstat(
                std::os::fd::AsRawFd::as_raw_fd(&root_descriptor),
                opened_root.as_mut_ptr(),
            )
        } < 0
        {
            return Err(format!(
                "cannot identify opened private {description} tombstone: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: fstat succeeded.
        let opened_root = unsafe { opened_root.assume_init() };
        if (opened_root.st_dev as u64, opened_root.st_ino as u64) != verified_root {
            return Err(format!(
                "private {description} tombstone root was replaced before descriptor binding; replacement preserved at {}",
                tombstone_root.display()
            ));
        }
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
        let mut opened_object = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: descriptor is open and fstat initializes opened_object on success.
        if unsafe {
            libc::fstat(
                std::os::fd::AsRawFd::as_raw_fd(&object_descriptor),
                opened_object.as_mut_ptr(),
            )
        } < 0
        {
            return Err(format!(
                "cannot identify opened identity-bound {description}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: fstat succeeded.
        let opened_object = unsafe { opened_object.assume_init() };
        if (opened_object.st_dev as u64, opened_object.st_ino as u64) != verified_object {
            return Err(format!(
                "identity-bound {description} object was replaced before descriptor binding; replacement preserved at {}",
                tombstone.display()
            ));
        }
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

#[cfg(unix)]
fn inspect_retained_before_deletion(
    identity: &WorktreeIdentity,
    object: &Path,
    confirmed: &CloseRetirementSnapshot,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(verify_retained_worktree(
        identity,
        object.to_path_buf(),
        confirmed,
    ))
}

#[cfg(unix)]
#[allow(
    clippy::useless_conversion,
    reason = "libc stat device width differs across supported Unix targets"
)]
#[allow(
    clippy::too_many_lines,
    reason = "exhaustive recovery keeps every persisted tombstone state and authority check visible"
)]
fn resume_final_worktree_tombstone(
    tombstone: &CloseWorktreeFinalTombstone,
    identity: &WorktreeIdentity,
    confirmed: &CloseRetirementSnapshot,
) -> FinalTombstoneRecovery {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    let expected_identity = identity.fingerprint().as_str();
    let captured_path = worktree_path(identity);
    let quarantine_path = match worktree_quarantine_path(identity) {
        Ok(path) => path,
        Err(detail) => return FinalTombstoneRecovery::Residual(detail),
    };
    let Ok(root) = CString::new(tombstone.root.as_os_str().as_bytes()) else {
        return FinalTombstoneRecovery::Residual(
            "recorded final tombstone path contains NUL".to_string(),
        );
    };
    // SAFETY: root is a valid C path and O_NOFOLLOW rejects replacement links.
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let root_missing = matches!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::NotFound
        );
        let captured_missing = match captured_path.try_exists() {
            Ok(exists) => !exists,
            Err(error) => {
                return FinalTombstoneRecovery::Residual(format!(
                    "cannot observe captured worktree path during final tombstone recovery: {error}"
                ));
            }
        };
        let quarantine_missing = match quarantine_path.try_exists() {
            Ok(exists) => !exists,
            Err(error) => {
                return FinalTombstoneRecovery::Residual(format!(
                    "cannot observe quarantined worktree path during final tombstone recovery: {error}"
                ));
            }
        };
        return if root_missing && captured_missing && quarantine_missing {
            FinalTombstoneRecovery::Completed
        } else {
            FinalTombstoneRecovery::Residual(format!(
                "recorded final tombstone is missing or inaccessible: {}",
                std::io::Error::last_os_error()
            ))
        };
    }
    // SAFETY: fd is newly owned on success above.
    let root_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let mut root_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: root_fd is open and fstat initializes root_stat on success.
    if unsafe { libc::fstat(root_fd.as_raw_fd(), root_stat.as_mut_ptr()) } < 0 {
        return FinalTombstoneRecovery::Residual(format!(
            "cannot identify recorded final tombstone: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fstat succeeded.
    let root_stat = unsafe { root_stat.assume_init() };
    let root_device = match u64::try_from(root_stat.st_dev) {
        Ok(device) => device,
        Err(error) => {
            return FinalTombstoneRecovery::Residual(format!(
                "final tombstone root device is invalid: {error}"
            ));
        }
    };
    if (root_device, root_stat.st_ino) != (tombstone.device, tombstone.inode) {
        return FinalTombstoneRecovery::Residual(
            "recorded final tombstone root was replaced; preserved for manual repair".to_string(),
        );
    }
    let name = CString::new("object").expect("static name contains no NUL");
    let (expected_object_device, expected_object_inode) = match (
        tombstone.object_device,
        tombstone.object_inode,
    ) {
        (Some(device), Some(inode)) => (device, inode),
        (None, None) => {
            let captured_exists = match captured_path.try_exists() {
                Ok(exists) => exists,
                Err(error) => {
                    return FinalTombstoneRecovery::Residual(format!(
                        "cannot observe captured worktree path during final tombstone recovery: {error}"
                    ));
                }
            };
            let quarantine_exists = match quarantine_path.try_exists() {
                Ok(exists) => exists,
                Err(error) => {
                    return FinalTombstoneRecovery::Residual(format!(
                        "cannot observe quarantined worktree path during final tombstone recovery: {error}"
                    ));
                }
            };
            match (captured_exists, quarantine_exists) {
                    (true, false) => {
                        if observe_worktree_fingerprint(&captured_path).as_deref()
                            != Some(expected_identity)
                        {
                            return FinalTombstoneRecovery::Residual(
                                "captured worktree does not match recorded final tombstone identity; preserved for manual repair".to_string(),
                            );
                        }
                        let tombstone_object = tombstone.root.join("object");
                        if let Err(error) = std::fs::rename(&captured_path, &tombstone_object) {
                            return FinalTombstoneRecovery::Residual(format!(
                                "cannot resume recorded final tombstone pre-rename state: {error}"
                            ));
                        }
                        let metadata = match std::fs::symlink_metadata(&tombstone_object) {
                            Ok(metadata) => metadata,
                            Err(error) => {
                                return FinalTombstoneRecovery::Residual(format!(
                                    "cannot identify resumed final tombstone object: {error}"
                                ))
                            }
                        };
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::MetadataExt as _;
                            (metadata.dev(), metadata.ino())
                        }
                        #[cfg(not(unix))]
                        unreachable!()
                    }
                    (false, false) => {
                        let mut object_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                        // SAFETY: root_fd is a verified owned directory descriptor, name is a valid C string,
                        // and object_stat points to writable storage.
                        let status = unsafe {
                            libc::fstatat(
                                root_fd.as_raw_fd(),
                                name.as_ptr(),
                                object_stat.as_mut_ptr(),
                                libc::AT_SYMLINK_NOFOLLOW,
                            )
                        };
                        if status == 0 {
                            return FinalTombstoneRecovery::Residual(
                                "unbound object remains inside the exact final tombstone root; preserved for manual repair".to_string(),
                            );
                        }
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::ENOENT) {
                            return FinalTombstoneRecovery::Completed;
                        }
                        return FinalTombstoneRecovery::Residual(format!(
                            "cannot inspect unbound final tombstone object: {error}"
                        ));
                    }
                    (true, true) => {
                        return FinalTombstoneRecovery::Residual(
                            "captured and quarantined worktree paths are both present during final tombstone recovery; preserved for manual repair".to_string(),
                        )
                    }
                    (false, true) => {
                        return FinalTombstoneRecovery::Residual(
                            "recorded final tombstone object identity is absent and only quarantine remains; preserved for manual repair".to_string(),
                        )
                    }
                }
        }
        _ => return FinalTombstoneRecovery::Residual(
            "recorded final tombstone object identity is incomplete; preserved for manual repair"
                .to_string(),
        ),
    };
    let object = tombstone.root.join("object");
    match object.try_exists() {
        Ok(true) => match quarantine_has_external_writer(&object) {
            Ok(true) => {
                return FinalTombstoneRecovery::Residual(
                    "external writer holds final tombstone object; preserved".into(),
                )
            }
            Ok(false) => {}
            Err(detail) => return FinalTombstoneRecovery::Residual(detail),
        },
        Ok(false) => {}
        Err(error) => {
            return FinalTombstoneRecovery::Residual(format!(
                "cannot probe final tombstone object: {error}"
            ))
        }
    }
    // SAFETY: openat is rooted in the verified descriptor and O_NOFOLLOW rejects replacement links.
    let object_fd = unsafe {
        libc::openat(
            root_fd.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if object_fd < 0 {
        return FinalTombstoneRecovery::Residual(format!(
            "recorded final tombstone object is missing or inaccessible: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: object_fd is newly owned on success above.
    let object_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(object_fd) };
    let mut object_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: object_fd is valid and object_stat is writable.
    if unsafe { libc::fstat(object_fd.as_raw_fd(), object_stat.as_mut_ptr()) } != 0 {
        return FinalTombstoneRecovery::Residual(format!(
            "cannot inspect recorded final tombstone object: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fstat succeeded.
    let object_stat = unsafe { object_stat.assume_init() };
    let object_device = match u64::try_from(object_stat.st_dev) {
        Ok(device) => device,
        Err(error) => {
            return FinalTombstoneRecovery::Residual(format!(
                "final tombstone object device is invalid: {error}"
            ));
        }
    };
    if (object_device, object_stat.st_ino) != (expected_object_device, expected_object_inode) {
        return FinalTombstoneRecovery::Residual(
            "recorded final tombstone object was replaced; preserved for manual repair".to_string(),
        );
    }
    let object = tombstone.root.join("object");
    if observe_worktree_fingerprint(&object).as_deref() != Some(expected_identity) {
        return FinalTombstoneRecovery::Residual("recorded final tombstone object does not match captured worktree identity; preserved for manual repair".to_string());
    }
    if let Err(detail) = inspect_retained_before_deletion(identity, &object, confirmed) {
        return FinalTombstoneRecovery::Residual(detail);
    }
    if let Err(detail) = remove_directory_contents_at(&object_fd) {
        return FinalTombstoneRecovery::Residual(detail);
    }
    // SAFETY: unlinkat is rooted at verified root and targets its fixed object name.
    if unsafe { libc::unlinkat(root_fd.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return FinalTombstoneRecovery::Residual(format!(
            "cannot remove recorded final tombstone object: {}",
            std::io::Error::last_os_error()
        ));
    }
    match std::fs::remove_dir(&tombstone.root) {
        Ok(()) => FinalTombstoneRecovery::Completed,
        Err(error) => FinalTombstoneRecovery::Residual(format!(
            "cannot remove empty recorded final tombstone: {error}"
        )),
    }
}

#[cfg(not(unix))]
fn resume_final_worktree_tombstone(
    _tombstone: &CloseWorktreeFinalTombstone,
    _identity: &WorktreeIdentity,
    _confirmed: &CloseRetirementSnapshot,
) -> FinalTombstoneRecovery {
    FinalTombstoneRecovery::Residual(
        "recorded final tombstone deletion is unsupported on this platform".to_string(),
    )
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
        None,
        expected_incarnation,
        observe_administrative_dir_incarnation,
        before_final_move,
        |_, _, _| Ok(()),
        |_, _| Ok(()),
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

#[derive(Debug, Eq, PartialEq)]
enum FsmonitorStop {
    Stopped,
    NotRunning,
}

fn classify_fsmonitor_stop(output: &std::process::Output) -> Result<FsmonitorStop, String> {
    if output.status.success() {
        return Ok(FsmonitorStop::Stopped);
    }
    if output.status.code() == Some(128)
        && output.stdout.is_empty()
        && output.stderr.trim_ascii() == b"fatal: fsmonitor--daemon is not running"
    {
        return Ok(FsmonitorStop::NotRunning);
    }
    Err(format!(
        "fsmonitor stop failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn stop_bound_fsmonitor_daemons(path: &Path) -> Result<FsmonitorStop, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let submodules = run_bounded_git_command_until(
        path,
        &[
            "submodule",
            "foreach",
            "--quiet",
            "--recursive",
            r#"output=$(git fsmonitor--daemon stop 2>&1); result=$?; if [ "$result" -eq 0 ] || { [ "$result" -eq 128 ] && [ "$output" = 'fatal: fsmonitor--daemon is not running' ]; }; then exit 0; fi; printf '%s\n' "$output" >&2; exit "$result""#,
        ],
        None,
        deadline,
        "submodule fsmonitor shutdown",
    )?;
    if !submodules.status.success() {
        return Err(format!(
            "submodule fsmonitor shutdown failed ({}): {}",
            submodules.status,
            String::from_utf8_lossy(&submodules.stderr).trim()
        ));
    }
    let worktree = run_bounded_git_command_until(
        path,
        &["fsmonitor--daemon", "stop"],
        None,
        deadline,
        "worktree fsmonitor shutdown",
    )?;
    classify_fsmonitor_stop(&worktree)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExternalWriterEvidence {
    PositiveWriterFound,
    NoPositiveEvidence,
}

impl ExternalWriterEvidence {
    fn found(self) -> bool {
        matches!(self, Self::PositiveWriterFound)
    }
}

fn probe_retry_worktree_writers(
    original: &Path,
    quarantine: &Path,
    tombstone: Option<&CloseWorktreeFinalTombstone>,
) -> Result<(), String> {
    let object = tombstone.map(|record| record.root.join("object"));
    for path in [Some(original), Some(quarantine), object.as_deref()]
        .into_iter()
        .flatten()
    {
        if path
            .try_exists()
            .map_err(|error| format!("cannot probe retry writer target: {error}"))?
            && quarantine_has_external_writer(path)?
        {
            return Err(format!(
                "external writer holds exact retry target {}; no new run admitted",
                path.display()
            ));
        }
    }
    Ok(())
}

async fn verify_retained_worktree(
    identity: &WorktreeIdentity,
    path: PathBuf,
    confirmed: &CloseRetirementSnapshot,
) -> Result<(), String> {
    ensure_no_ignored_content(&path)?;
    let (fresh, losses) = inspect_worktree_at(identity, path).await?;
    if !losses.is_empty() || fresh.fingerprint() != confirmed.fingerprint() {
        return Err(
            "retained worktree is changed or not reconstructible; preserved for repair".into(),
        );
    }
    Ok(())
}

fn quarantine_has_external_writer(path: &Path) -> Result<bool, String> {
    Ok(quarantine_has_open_descriptors(path)?.found()
        || quarantine_has_process_cwd(path)?
        || quarantine_has_writable_mappings(path)?)
}

#[cfg(target_os = "linux")]
fn quarantine_has_writable_mappings(path: &Path) -> Result<bool, String> {
    // SAFETY: `geteuid` has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    quarantine_has_writable_mappings_in(path, Path::new("/proc"), effective_uid)
}

#[cfg(target_os = "linux")]
fn quarantine_has_writable_mappings_in(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantine before mapping inspection: {error}")
    })?;
    let Ok(processes) = std::fs::read_dir(proc_root) else {
        return Ok(false);
    };
    for process in processes.flatten() {
        if !linux_process_is_relevant(&process, effective_uid, "mapping") {
            continue;
        }
        let Ok(mappings) = std::fs::read_to_string(process.path().join("maps")) else {
            continue;
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
    let Some(pids) = macos_all_pids() else {
        return Ok(false);
    };
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
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
                // Ambient process inspection is observational. A short kernel
                // result proves nothing about this process, so skip it.
                break;
            }
            let info = unsafe { info.assume_init() };
            let Some(next) = info.region.address.checked_add(info.region.size) else {
                break;
            };
            if next <= address {
                break;
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
    // SAFETY: `geteuid` has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    quarantine_has_process_cwd_in(path, Path::new("/proc"), effective_uid)
}

#[cfg(target_os = "linux")]
fn quarantine_has_process_cwd_in(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantine before cwd inspection: {error}")
    })?;
    let Ok(processes) = std::fs::read_dir(proc_root) else {
        return Ok(false);
    };
    for process in processes.flatten() {
        if !linux_process_is_relevant(&process, effective_uid, "cwd") {
            continue;
        }
        if std::fs::read_link(process.path().join("cwd"))
            .is_ok_and(|cwd| path_is_within(&cwd, &canonical))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn macos_all_pids() -> Option<Vec<i32>> {
    macos_all_pids_with(|buffer, buffer_bytes| unsafe {
        libc::proc_listpids(1, 0, buffer, buffer_bytes)
    })
}

#[cfg(target_os = "macos")]
fn macos_all_pids_with(mut list: impl FnMut(*mut libc::c_void, i32) -> i32) -> Option<Vec<i32>> {
    use std::mem::size_of;

    let mut capacity = 4096_usize;
    loop {
        let mut pids = vec![0_i32; capacity];
        let capacity_bytes = pids.len().checked_mul(size_of::<i32>())?;
        let capacity_bytes_i32 = i32::try_from(capacity_bytes).ok()?;
        let pid_bytes = list(pids.as_mut_ptr().cast(), capacity_bytes_i32);
        if pid_bytes < 0 {
            return None;
        }
        let pid_bytes = usize::try_from(pid_bytes).ok()?;
        if pid_bytes < capacity_bytes {
            // A non-integral short result is not a trustworthy PID inventory.
            if !pid_bytes.is_multiple_of(size_of::<i32>()) {
                return None;
            }
            pids.truncate(pid_bytes / size_of::<i32>());
            return Some(pids);
        }
        capacity = capacity.checked_mul(2)?;
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
    let Some(pids) = macos_all_pids() else {
        return Ok(false);
    };
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinuxProcessEffectiveUid(libc::uid_t);

#[cfg(target_os = "linux")]
impl LinuxProcessEffectiveUid {
    fn parse_status(status: &str) -> Result<Self, String> {
        let mut uid_lines = status.lines().filter_map(|line| line.strip_prefix("Uid:"));
        let uid_fields = uid_lines
            .next()
            .ok_or_else(|| "status has no Uid field".to_string())?;
        if uid_lines.next().is_some() {
            return Err("status has multiple Uid fields".to_string());
        }
        let uid_fields = uid_fields
            .split_ascii_whitespace()
            .map(|uid| {
                uid.parse::<libc::uid_t>()
                    .map_err(|error| format!("status UID is malformed: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let [_real, effective_uid, _saved, _filesystem] = uid_fields.as_slice() else {
            return Err("status Uid field must contain exactly four credentials".to_string());
        };
        Ok(Self(*effective_uid))
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxProcessOwner {
    Relevant,
    Unrelated,
    Vanished,
}

#[cfg(target_os = "linux")]
fn linux_process_owner(
    process: &std::fs::DirEntry,
    effective_uid: libc::uid_t,
    inventory: &str,
) -> Result<LinuxProcessOwner, String> {
    let status = match std::fs::read_to_string(process.path().join("status")) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LinuxProcessOwner::Vanished);
        }
        Err(error) => {
            return Err(format!(
                "cannot attribute process {} {inventory} inventory from kernel credentials: {error}",
                process.file_name().to_string_lossy()
            ));
        }
    };
    let process_effective_uid =
        LinuxProcessEffectiveUid::parse_status(&status).map_err(|error| {
            format!(
            "cannot attribute process {} {inventory} inventory from kernel credentials: {error}",
            process.file_name().to_string_lossy()
        )
        })?;

    // The scanned proc files use `PTRACE_MODE_READ_FSCREDS`: Linux compares the
    // scanner's filesystem UID with the target's real, effective, and saved UIDs.
    // Effective UID is Phoenix's process-ownership boundary; unlike proc-dir inode
    // ownership, it is not rewritten to root when a same-user target is nondumpable.
    Ok(
        if process_effective_uid == LinuxProcessEffectiveUid(effective_uid) {
            LinuxProcessOwner::Relevant
        } else {
            LinuxProcessOwner::Unrelated
        },
    )
}

#[cfg(target_os = "linux")]
fn linux_process_is_relevant(
    process: &std::fs::DirEntry,
    effective_uid: libc::uid_t,
    inventory: &str,
) -> bool {
    process
        .file_name()
        .as_encoded_bytes()
        .iter()
        .all(u8::is_ascii_digit)
        && matches!(
            linux_process_owner(process, effective_uid, inventory),
            Ok(LinuxProcessOwner::Relevant)
        )
}

#[cfg(target_os = "linux")]
fn linux_descriptor_target_is_within(
    target: std::io::Result<PathBuf>,
    canonical_worktree: &Path,
) -> bool {
    target.is_ok_and(|candidate| path_is_within(&candidate, canonical_worktree))
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors(path: &Path) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_in(path, Path::new("/proc"))
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors_in(
    path: &Path,
    proc_root: &Path,
) -> Result<ExternalWriterEvidence, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        format!("cannot canonicalize quarantined worktree before descriptor inspection: {error}")
    })?;
    let Ok(processes) = std::fs::read_dir(proc_root) else {
        return Ok(ExternalWriterEvidence::NoPositiveEvidence);
    };
    for process in processes.flatten().filter(|process| {
        process
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
    }) {
        let Ok(descriptors) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            if linux_descriptor_target_is_within(std::fs::read_link(descriptor.path()), &canonical)
            {
                return Ok(ExternalWriterEvidence::PositiveWriterFound);
            }
        }
    }
    Ok(ExternalWriterEvidence::NoPositiveEvidence)
}

#[cfg(target_os = "macos")]
fn descriptor_inventory_may_be_truncated(returned_bytes: usize, capacity_bytes: usize) -> bool {
    returned_bytes >= capacity_bytes
}

#[allow(clippy::too_many_lines)]
#[cfg(target_os = "macos")]
fn quarantine_has_open_descriptors(path: &Path) -> Result<ExternalWriterEvidence, String> {
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
    let Some(pids) = macos_all_pids() else {
        return Ok(ExternalWriterEvidence::NoPositiveEvidence);
    };
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
            let Ok(capacity_bytes_i32) = i32::try_from(capacity_bytes) else {
                break Vec::new();
            };
            // SAFETY: the vector provides writable storage for exactly the byte count passed.
            let descriptor_bytes = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    descriptors.as_mut_ptr().cast(),
                    capacity_bytes_i32,
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
            let Some(next_capacity) = descriptor_capacity.checked_mul(2) else {
                break Vec::new();
            };
            descriptor_capacity = next_capacity;
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
                return Ok(ExternalWriterEvidence::PositiveWriterFound);
            }
        }
    }
    Ok(ExternalWriterEvidence::NoPositiveEvidence)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_open_descriptors(_path: &Path) -> Result<ExternalWriterEvidence, String> {
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
async fn quarantine_and_remove_exact_worktree<F, B>(
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
    final_tombstone: Option<CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    quarantine_and_remove_exact_worktree_with_stop(
        identity,
        confirmed_snapshot,
        planned_administrative_dir,
        planned_administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
        after_quarantine,
        stop_bound_fsmonitor_daemons,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn quarantine_and_remove_exact_worktree_with_stop<F, B, S>(
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
    final_tombstone: Option<CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    stop: S,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
    S: FnOnce(&Path) -> Result<FsmonitorStop, String> + Send + 'static,
{
    let path = worktree_path(identity);
    let quarantine = worktree_quarantine_path(identity)?;
    let resuming_quarantine = !path.exists() && quarantine.exists();
    if !path.exists() && !resuming_quarantine {
        return Err("captured worktree path is absent without an exact prior receipt".to_string());
    }
    let expected = identity.fingerprint().as_str().to_string();
    let inspection_identity = identity.clone();
    let confirmed_fingerprint = confirmed_snapshot.fingerprint().to_string();
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

        if let Err(detail) = stop(inspection_path) {
            return Ok(ExactWorktreeRemoval::StopFailed { detail });
        }
        let verification_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let verify_confirmed_snapshot = |candidate: &Path| -> Result<(), String> {
            let (fresh, _) = verification_runtime
                .block_on(inspect_worktree_at(&inspection_identity, candidate.to_path_buf()))?;
            if fresh.fingerprint() != confirmed_fingerprint {
                return Err(
                    "worktree changed after Close inspection confirmation; fresh confirmation is required"
                        .to_string(),
                );
            }
            ensure_no_ignored_content(candidate)
        };
        if let Err(detail) = verify_confirmed_snapshot(inspection_path) {
            return Ok(ExactWorktreeRemoval::ReinspectionRequired { detail });
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
        if quarantine_has_open_descriptors(&quarantine)?.found() {
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
        if let Err(detail) = verify_confirmed_snapshot(&quarantine) {
            return Ok(ExactWorktreeRemoval::ReinspectionRequired { detail });
        }
        let administrative_dir = exact_worktree_administrative_dir(&quarantine, &common)?;
        if administrative_dir != planned_administrative_dir {
            return Err(
                "exact worktree administrative directory differs from durable cleanup plan"
                    .to_string(),
            );
        }
        let expected = observe_worktree_fingerprint(&quarantine).ok_or_else(|| {
            "cannot observe quarantined worktree identity before final deletion".to_string()
        })?;
        remove_identity_bound_directory(
            &quarantine,
            final_tombstone
                .as_ref()
                .map(|binding| binding.root.as_path()),
            &expected,
            |path| {
                observe_worktree_fingerprint(path).ok_or_else(|| {
                    "cannot observe worktree identity in final tombstone".to_string()
                })
            },
            |_| {},
            bind_tombstone,
            |_, object| verify_confirmed_snapshot(object),
            "quarantined worktree",
        )?;
        remove_exact_worktree_administrative_dir(
            &administrative_dir,
            &planned_administrative_dir_incarnation,
        )?;
        Ok(ExactWorktreeRemoval::Retired)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
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

#[cfg(test)]
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
        None,
        &expected,
        |path| {
            observe_worktree_fingerprint(path).ok_or_else(|| {
                "cannot observe worktree identity in private final tombstone".to_string()
            })
        },
        before_final_move,
        |_, _, _| Ok(()),
        |_, _| Ok(()),
        "quarantined worktree",
    )?;
    after_quarantine_removal();
    remove_exact_worktree_administrative_dir(administrative_dir, administrative_dir_incarnation)
}

fn ensure_no_ignored_content(path: &Path) -> Result<(), String> {
    fn inspect(
        path: &Path,
        deadline: std::time::Instant,
        visited: &mut std::collections::HashSet<PathBuf>,
    ) -> Result<(), String> {
        if std::time::Instant::now() >= deadline {
            return Err("ignored-content inspection exceeded its deadline".into());
        }
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !visited.insert(canonical) {
            return Err("initialized submodule graph contains a cycle".into());
        }
        let status = run_bounded_git_status_until(path, deadline)?;
        if !status.status.success() {
            return Err(format!(
                "cannot inspect ignored content: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
        if status
            .stdout
            .split(|byte| *byte == 0)
            .any(|row| row.starts_with(b"!! "))
        {
            return Err(format!(
                "ignored content at {} has no exact discard authority; worktree preserved",
                path.display()
            ));
        }
        let (_, gitlinks) = index_gitlinks(path, deadline)?;
        for gitlink in gitlinks {
            if std::time::Instant::now() >= deadline {
                return Err("ignored-content inspection exceeded its deadline".into());
            }
            let submodule = path.join(path_buf_from_git_bytes(gitlink.path.as_bytes()));
            if submodule
                .join(".git")
                .try_exists()
                .map_err(|error| error.to_string())?
            {
                inspect(&submodule, deadline, visited)?;
            }
        }
        Ok(())
    }
    inspect(
        path,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        &mut std::collections::HashSet::new(),
    )
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

/// Detect modifications that `git status` intentionally suppresses for index entries
/// marked assume-unchanged or skip-worktree. All index mutations happen in a copied index.
fn observe_hidden_worktree_changes(repository: &Path) -> Result<Vec<GitPathIdentity>, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let index_path = git_index_path(repository, deadline)?;
    if !index_path.exists() {
        return Ok(Vec::new());
    }
    let private_index = tempfile::NamedTempFile::new_in(
        index_path
            .parent()
            .ok_or("Git index has no parent directory")?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::copy(&index_path, private_index.path())
        .map_err(|error| format!("cannot copy Git index for hidden-change inspection: {error}"))?;
    let private_index = private_index.path();
    let flags = run_bounded_git_command_until(
        repository,
        &["ls-files", "-v", "-z", "--"],
        Some(private_index),
        deadline,
        "hidden-index inspection",
    )?;
    if !flags.status.success() {
        return Err(format!(
            "cannot inspect hidden index flags: {}",
            String::from_utf8_lossy(&flags.stderr).trim()
        ));
    }
    let candidates = hidden_index_candidates(&flags.stdout);
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let sparse_absent = sparse_checkout_absent_paths(repository, &candidates, deadline)?;

    let mut dirty = BTreeSet::new();
    for batch in candidates.chunks(256) {
        let batch_paths = batch
            .iter()
            .filter(|candidate| !sparse_absent.contains(candidate.path.as_slice()))
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();
        if batch_paths.is_empty() {
            continue;
        }
        // Path arguments are OsStrings below so non-UTF-8 paths remain literal.
        let clear = run_bounded_git_paths_until(
            repository,
            &["update-index", "--no-assume-unchanged", "--"],
            &batch_paths,
            true,
            private_index,
            deadline,
            "hidden-index flag clearing",
        )?;
        if !clear.status.success() {
            return Err(format!(
                "cannot clear hidden flags in private index: {}",
                String::from_utf8_lossy(&clear.stderr).trim()
            ));
        }
        let clear_skip = run_bounded_git_paths_until(
            repository,
            &["update-index", "--no-skip-worktree", "--"],
            &batch_paths,
            true,
            private_index,
            deadline,
            "hidden-index flag clearing",
        )?;
        if !clear_skip.status.success() {
            return Err(format!(
                "cannot clear hidden flags in private index: {}",
                String::from_utf8_lossy(&clear_skip.stderr).trim()
            ));
        }
        let diff = run_bounded_git_command_until(
            repository,
            &["diff-files", "--raw", "-z", "--no-ext-diff"],
            Some(private_index),
            deadline,
            "hidden worktree comparison",
        )?;
        if !diff.status.success() {
            return Err(format!(
                "cannot compare hidden worktree changes: {}",
                String::from_utf8_lossy(&diff.stderr).trim()
            ));
        }
        let batch = batch_paths.iter().collect::<BTreeSet<_>>();
        dirty.extend(
            raw_diff_paths(&diff.stdout)
                .into_iter()
                .filter(|path| batch.contains(path)),
        );
    }
    Ok(dirty.into_iter().map(GitPathIdentity::from_bytes).collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HiddenIndexFlag {
    AssumeUnchanged,
    SkipWorktree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HiddenIndexCandidate {
    flag: HiddenIndexFlag,
    path: Vec<u8>,
}

fn hidden_index_candidates(entries: &[u8]) -> Vec<HiddenIndexCandidate> {
    entries
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let (flag, path) = entry.split_first()?;
            let flag = match flag {
                b'h' => HiddenIndexFlag::AssumeUnchanged,
                b'S' | b's' => HiddenIndexFlag::SkipWorktree,
                _ => return None,
            };
            path.strip_prefix(b" ").map(|path| HiddenIndexCandidate {
                flag,
                path: path.to_vec(),
            })
        })
        .collect()
}

fn sparse_checkout_absent_paths(
    repository: &Path,
    candidates: &[HiddenIndexCandidate],
    deadline: std::time::Instant,
) -> Result<BTreeSet<Vec<u8>>, String> {
    let skip_absent = candidates
        .iter()
        .filter(|candidate| candidate.flag == HiddenIndexFlag::SkipWorktree)
        .filter(|candidate| {
            !repository
                .join(path_buf_from_git_bytes(&candidate.path))
                .exists()
        })
        .map(|candidate| candidate.path.clone())
        .collect::<Vec<_>>();
    if skip_absent.is_empty() {
        return Ok(BTreeSet::new());
    }

    let sparse_patterns = git_index_path(repository, deadline)?
        .parent()
        .ok_or("Git index has no parent directory")?
        .join("info")
        .join("sparse-checkout");
    if !sparse_patterns.exists() {
        return Ok(BTreeSet::new());
    }

    let sparse_matches = git_sparse_checkout_selected_paths(
        repository,
        &skip_absent,
        git_sparse_checkout_cone_mode(repository, deadline)?,
        deadline,
    )?
    .into_iter()
    .collect::<BTreeSet<_>>();
    Ok(skip_absent
        .into_iter()
        .filter(|path| !sparse_matches.contains(path))
        .collect())
}

fn git_sparse_checkout_cone_mode(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<bool, String> {
    let output = run_bounded_git_command_until(
        repository,
        &["config", "--bool", "--get", "core.sparseCheckoutCone"],
        None,
        deadline,
        "sparse-checkout cone inspection",
    )?;
    Ok(output.status.success() && output.stdout.trim_ascii() == b"true")
}

fn git_sparse_checkout_selected_paths(
    repository: &Path,
    paths: &[Vec<u8>],
    cone: bool,
    deadline: std::time::Instant,
) -> Result<Vec<Vec<u8>>, String> {
    let mut command = phoenix_core::git::command();
    bind_git_command_to_worktree(&mut command, repository)?;
    command
        .arg("sparse-checkout")
        .arg("check-rules")
        .arg("-z")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0");
    if cone {
        command.arg("--cone");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot inspect sparse-checkout rules: {error}"))?;
    {
        use std::io::Write as _;
        let mut stdin = child
            .stdin
            .take()
            .ok_or("cannot open sparse-checkout stdin")?;
        for path in paths {
            stdin
                .write_all(path)
                .and_then(|()| stdin.write_all(&[0]))
                .map_err(|error| format!("cannot write sparse-checkout probe paths: {error}"))?;
        }
    }
    let output = wait_bounded_child_output(child, deadline, "sparse-checkout rule inspection")?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect sparse-checkout rules: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn raw_diff_paths(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut paths = Vec::new();
    let mut records = raw.split(|byte| *byte == 0);
    while let Some(header) = records.next() {
        if header.is_empty() {
            continue;
        }
        if header.starts_with(b":") {
            if let Some(path) = records.next() {
                paths.push(path.to_vec());
            }
        }
    }
    paths
}

fn git_index_path(repository: &Path, deadline: std::time::Instant) -> Result<PathBuf, String> {
    let output = run_bounded_git_command_until(
        repository,
        &["rev-parse", "--git-path", "index"],
        None,
        deadline,
        "Git index path inspection",
    )?;
    if !output.status.success() {
        return Err(format!(
            "cannot locate Git index: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output = output.stdout.trim_ascii();
    let path = path_buf_from_git_bytes(output);
    Ok(if path.is_absolute() {
        path
    } else {
        repository.join(path)
    })
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
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    observation.extend_from_slice(b"EXECUTABLE\0");
                    observation.push(u8::from(metadata.permissions().mode() & 0o111 != 0));
                    observation.push(0);
                }
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
        bind_git_command_to_worktree(&mut command, repository)?;
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
                    if let Some(source) = rows
                        .next()
                        .map(|source| GitPathIdentity::from_bytes(source.to_vec()))
                    {
                        if xy[0] == b'R' {
                            losses.push(CloseLossItem::StagedTrackedPath(source.clone()));
                        }
                        if xy[1] == b'R' {
                            losses.push(CloseLossItem::UnstagedTrackedPath(source));
                        }
                    }
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

fn require_absent(outcome: BashRetirementOutcome) -> Result<CloseProcessStepOutcome, String> {
    match outcome {
        BashRetirementOutcome::StaleGeneration(_) => {
            Err("bash retirement generation is stale".to_string())
        }
        BashRetirementOutcome::Retired(report) if report.kill_failures.is_empty() => {
            Ok(CloseProcessStepOutcome::Retired)
        }
        BashRetirementOutcome::AbsenceVerified(report) if report.kill_failures.is_empty() => {
            Ok(CloseProcessStepOutcome::AbsenceVerified)
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

fn require_terminal_absent(
    outcome: TerminalRetirementOutcome,
) -> Result<CloseProcessStepOutcome, String> {
    match outcome {
        TerminalRetirementOutcome::Retired => Ok(CloseProcessStepOutcome::Retired),
        TerminalRetirementOutcome::AbsenceVerified => Ok(CloseProcessStepOutcome::AbsenceVerified),
        TerminalRetirementOutcome::Residual { reason } => Err(reason),
    }
}

fn require_browser_absent(
    outcome: BrowserRetirementOutcome,
) -> Result<CloseProcessStepOutcome, String> {
    match outcome {
        BrowserRetirementOutcome::Retired => Ok(CloseProcessStepOutcome::Retired),
        BrowserRetirementOutcome::AbsenceVerified => Ok(CloseProcessStepOutcome::AbsenceVerified),
        BrowserRetirementOutcome::Residual { reason } => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        both_worktree_paths_absent, canonical_status_observation,
        complete_persisted_worktree_administrative_cleanup, exact_worktree_administrative_dir,
        git_path_from_observation, inspect_and_remove_exact_worktree_with_hook, inspect_worktree,
        observe_administrative_dir_incarnation, observe_worktree_fingerprint, parse_status_losses,
        planned_administrative_dir_is_absent, quarantine_and_remove_exact_worktree,
        remove_directory_contents_at_with_hook, remove_exact_worktree_administrative_dir_with_hook,
        remove_identity_bound_directory, remove_quarantine_then_administrative_dir,
        remove_quarantine_then_administrative_dir_with_hooks, resume_final_worktree_tombstone,
        rotate_inspection_generation, run_bounded_git_status_until, snapshot_for,
        staged_index_entries_by_path, staged_index_entries_for_paths, worktree_quarantine_path,
        CloseLeaseFailure, ExactWorktreeRemoval, FinalTombstoneRecovery,
    };
    use crate::db::{CloseCleanupResourceDisposition, CloseWorktreeFinalTombstone};
    use phoenix_core::domain::close::{
        CloseLossItem, GitPathIdentity, WorktreeFingerprint, WorktreeId, WorktreeIdentity,
    };
    use std::io::Write as _;
    #[cfg(target_os = "linux")]
    use std::io::{BufRead as _, Read as _};
    use std::path::Path;

    #[test]
    fn empty_process_identity_sets_require_fresh_registry_absence_verification() {
        let scope = phoenix_core::work_scope::WorkScopeId::parse("empty-process-scope").unwrap();
        assert!(!super::process_step_successes_cover(
            &[],
            &scope,
            crate::db::CloseProcessResourceKind::BashProcessGroup,
            &[],
        ));
    }

    #[test]
    fn safe_retry_plan_rejects_process_equivalent_unknown_and_duplicate_effects() {
        use crate::db::{
            CloseCleanupFailureResource, CloseCleanupResourceDisposition as Disposition,
        };
        use phoenix_core::domain::close::RetiredResourceKind as Kind;
        let scope = phoenix_core::work_scope::WorkScopeId::parse("retry-scope").unwrap();
        let target = CloseCleanupFailureResource {
            scope: scope.clone(),
            resource: super::opaque_resource(Kind::WorkScope, scope.as_str().to_string()),
            disposition: Disposition::Failed,
        };
        assert!(super::close_safe_retry_effects(&[]).is_err());
        assert_eq!(
            super::close_safe_retry_effects(std::slice::from_ref(&target)).unwrap()[0].resource,
            target.resource
        );
        assert!(super::close_safe_retry_effects(&[target.clone(), target.clone()]).is_err());
        let mut unknown = target.clone();
        unknown.disposition = Disposition::Unknown;
        assert!(super::close_safe_retry_effects(&[unknown]).is_err());
        for kind in [
            Kind::BashProcessGroup,
            Kind::PtySession,
            Kind::BrowserSession,
            Kind::EquivalentLiveResource,
        ] {
            let process = CloseCleanupFailureResource {
                resource: super::opaque_resource(kind, format!("process-{kind:?}")),
                ..target.clone()
            };
            assert!(super::close_safe_retry_effects(&[process]).is_err());
        }
    }

    #[test]
    fn safe_retry_executes_tmux_and_worktree_before_scope_despite_failure_child_order() {
        use phoenix_core::domain::close::{
            LossItemIdentity, RetiredResourceIdentity, RetiredResourceKind as Kind,
        };
        let scope = phoenix_core::work_scope::WorkScopeId::parse("retry-scope").unwrap();
        let worktree = RetiredResourceIdentity::parse(
            Kind::Worktree,
            LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                WorktreeId::parse("retry-worktree").unwrap(),
                WorktreeFingerprint::parse("original-incarnation").unwrap(),
                GitPathIdentity::from_bytes(b"/tmp/retry-worktree".to_vec()),
            )),
        )
        .unwrap();
        let effect = |resource| crate::db::CloseSafeRetryEffect {
            scope: scope.clone(),
            resource,
        };
        let plan = [
            effect(super::opaque_resource(
                Kind::WorkScope,
                scope.as_str().to_string(),
            )),
            effect(worktree),
            effect(super::opaque_resource(
                Kind::TmuxServer,
                "persisted-tmux".into(),
            )),
        ];
        assert_eq!(
            super::close_safe_retry_execution_order(&plan),
            vec![2, 1, 0]
        );
    }

    async fn assert_close_admission_fenced(
        manager: &super::RuntimeManager,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) {
        let key = phoenix_core::work_scope::ResourceScopeKey::Work(scope.clone());
        assert!(matches!(
            manager.bash_handles().reserve_spawn(&key).await,
            Err(phoenix_tools::bash::registry::BashHandleError::SpawnFenced)
        ));
        assert!(matches!(
            manager.terminals.reserve_spawn(&key),
            Err(phoenix_terminal::session::ActiveTerminalInsertError::RetirementFenced)
        ));
        assert!(matches!(
            manager.browser_sessions().get_session(&key).await,
            Err(phoenix_tools::browser::session::BrowserError::RetirementFenced { .. })
        ));
        assert!(matches!(
            manager
                .tmux_registry()
                .ensure_live(&key, Path::new("/tmp"), None, None)
                .await,
            Err(phoenix_tools::tmux::registry::TmuxError::RetirementFenced { .. })
        ));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn captured_cleanup_failure_terminalizes_and_kicks_on_exact_replay() {
        use phoenix_core::domain::close::{
            CloseAttemptId, CloseCompletionOutcome, ClosePhase, RetirementFailureReason,
            TranscriptConversationId,
        };
        use std::sync::Arc;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("close-source", "close-source", "/tmp", true, None, None)
            .await
            .unwrap();
        let scope = conversation.attached_work_scope_id.unwrap();
        let attempt_id = CloseAttemptId::parse("runtime-cleanup-failure").unwrap();
        db.begin_close_foundation(
            &conversation.product_conversation_id,
            &TranscriptConversationId::parse(&conversation.id).unwrap(),
            attempt_id.as_str(),
        )
        .await
        .unwrap();
        let manager = super::RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "test".to_string(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        );
        manager
            .acquire_close_resource_lease(&attempt_id, scope.clone())
            .await
            .unwrap();
        let broadcaster = manager.conversation_broadcaster(&conversation.id).await;
        let mut receiver = broadcaster.subscribe();
        let mut kicks = manager.direct_turn_kick_tx.subscribe();
        let initial_kick = *kicks.borrow_and_update();
        let detail = "process shutdown could not be proven";
        let reason = RetirementFailureReason::IdentityNotProven;
        assert_eq!(
            manager
                .route_close_attempt_to_repair::<()>(&attempt_id, &scope, reason, detail)
                .await,
            Err(detail.to_string())
        );
        assert!(kicks.has_changed().unwrap());
        assert_eq!(*kicks.borrow_and_update(), initial_kick + 1);
        let completed = db.get_close_obligation(attempt_id.as_str()).await.unwrap();
        assert_eq!(completed.phase(), ClosePhase::Completed);
        assert_eq!(
            completed.close_outcome(),
            Some(CloseCompletionOutcome::CloseIncomplete)
        );
        assert!(
            !db.get_conversation(&conversation.id)
                .await
                .unwrap()
                .archived
        );
        assert_close_admission_fenced(&manager, &scope).await;
        assert!(manager.close_retirement_leases.lock().await.is_empty());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            crate::runtime::SseEvent::ConversationUpdate {
                update: crate::runtime::ConversationMetadataUpdate {
                    archived: Some(false),
                    ..
                },
                ..
            }
        ));
        let failure_occurrence_id =
            phoenix_core::domain::close::CloseRunRef::initial(attempt_id.clone())
                .failure_occurrence_id();
        let failures: Vec<(String, i64)> = sqlx::query_as(
            "SELECT failure_occurrence_id, occurred_at_us FROM close_cleanup_failures",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, failure_occurrence_id);
        let persisted = db
            .list_close_cleanup_failures(attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(
            persisted[0].occurrence.remaining_resources,
            vec![super::CloseCleanupFailureResource {
                scope: scope.clone(),
                resource: super::opaque_resource(
                    phoenix_core::domain::close::RetiredResourceKind::WorkScope,
                    scope.as_str().to_string()
                ),
                disposition: super::CloseCleanupResourceDisposition::Failed,
            }]
        );
        assert_eq!(
            manager
                .route_close_attempt_to_repair::<()>(&attempt_id, &scope, reason, detail)
                .await,
            Err(detail.to_string())
        );
        assert!(kicks.has_changed().unwrap());
        assert_eq!(*kicks.borrow_and_update(), initial_kick + 2);
        assert_close_admission_fenced(&manager, &scope).await;
        assert!(matches!(
            receiver.try_recv().unwrap(),
            crate::runtime::SseEvent::ConversationUpdate {
                update: crate::runtime::ConversationMetadataUpdate {
                    archived: Some(false),
                    ..
                },
                ..
            }
        ));
        let events: Vec<(String, String)> =
            sqlx::query_as("SELECT event_id, route_kind FROM coordinator_watch_events")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(
            events,
            vec![(failure_occurrence_id, "mandatory_close_failure".to_string())]
        );
        assert_eq!(
            db.get_close_obligation(attempt_id.as_str()).await.unwrap(),
            completed
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn cleanup_attention_publishes_all_participants_only_after_commit_and_replays() {
        use super::*;
        use phoenix_core::domain::close::TranscriptConversationId;
        use std::sync::Arc;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let scope = conversation.attached_work_scope_id.clone().unwrap();
        db.create_subagent_conversation(
            "participant",
            "participant",
            "/tmp",
            &conversation.id,
            "test-model",
            &crate::db::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            Some(&scope),
            crate::db::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
        let worktree = WorktreeIdentity::from_parts(
            WorktreeId::parse("failure-worktree").unwrap(),
            WorktreeFingerprint::parse("failure-fingerprint").unwrap(),
            GitPathIdentity::from_bytes(b"/tmp/runtime-cleanup-attention-worktree".to_vec()),
        );
        sqlx::query("UPDATE work_scopes SET environment_kind='allocated_worktree', worktree_path=?1, worktree_id=?2, worktree_fingerprint=?3, branch_name='test', base_branch='main' WHERE id=?4")
            .bind("/tmp/runtime-cleanup-attention-worktree").bind(worktree.id().as_str()).bind(worktree.fingerprint().as_str()).bind(scope.as_str())
            .execute(db.pool()).await.unwrap();
        db.update_conversation_state(
            &conversation.id,
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "continue onto another scope".into(),
            },
        )
        .await
        .unwrap();
        let crate::db::ContinueOutcome::Created(latest) =
            db.continue_conversation(&conversation.id).await.unwrap()
        else {
            panic!("expected a new continuation");
        };
        let later_scope = WorkScopeId::parse("later-scope").unwrap();
        sqlx::query("INSERT INTO work_scopes (id, authority_kind, lifecycle, environment_kind, cwd, created_at, updated_at) SELECT ?1, 'work', 'active', 'unowned_cwd', '/tmp/later', created_at, updated_at FROM work_scopes WHERE id=?2")
            .bind(later_scope.as_str()).bind(scope.as_str()).execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id=?1 WHERE id=?2")
            .bind(later_scope.as_str())
            .bind(&latest.id)
            .execute(db.pool())
            .await
            .unwrap();
        let attempt_id = CloseAttemptId::parse("runtime-cleanup-attention").unwrap();
        db.begin_close_foundation(
            &conversation.product_conversation_id,
            &TranscriptConversationId::parse(&latest.id).unwrap(),
            attempt_id.as_str(),
        )
        .await
        .unwrap();
        db.confirm_close_stop_work(attempt_id.as_str())
            .await
            .unwrap();
        db.begin_close_active_work_settlement(attempt_id.as_str())
            .await
            .unwrap();
        db.advance_close_settlement_when_quiescent(attempt_id.as_str())
            .await
            .unwrap();
        let manager = RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "test".to_string(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        );
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: attempt_id.clone(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("failure-gen", "failure-fp").unwrap(),
                losses: vec![],
            }],
        })
        .await
        .unwrap();
        let snapshot = db
            .get_close_obligation(attempt_id.as_str())
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
        manager
            .acquire_close_resource_lease(&attempt_id, scope.clone())
            .await
            .unwrap();
        let resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(worktree.clone()),
        )
        .unwrap();
        db.capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
            attempt_id: attempt_id.clone(),
            snapshot: snapshot.clone(),
            scopes: vec![
                CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: Some(worktree),
                        work_scopes: BTreeSet::new(),
                        bash_process_groups: BTreeSet::new(),
                        tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(),
                        browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                },
                CaptureCloseRetirementInventoryScopeRequest {
                    scope: later_scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: None,
                        work_scopes: BTreeSet::new(),
                        bash_process_groups: BTreeSet::new(),
                        tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(),
                        browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                },
            ],
        })
        .await
        .unwrap();
        let mut receivers = Vec::new();
        for id in [&conversation.id, "participant", &latest.id] {
            receivers.push((id, manager.conversation_broadcaster(id).await.subscribe()));
        }
        let detail = "scope retirement failed";
        let reason = RetirementFailureReason::RemovalFailed;
        sqlx::query("CREATE TRIGGER reject_failure_event BEFORE INSERT ON coordinator_watch_events BEGIN SELECT RAISE(ABORT, 'injected outbox failure'); END")
            .execute(db.pool()).await.unwrap();
        let error = manager
            .record_close_cleanup_failure::<()>(
                &super::CloseRunRef::initial(attempt_id.clone()),
                &snapshot,
                &scope,
                resource.clone(),
                reason,
                detail,
            )
            .await
            .unwrap_err();
        assert!(error.contains("injected outbox failure"), "{error}");
        for (id, receiver) in &mut receivers {
            assert!(!db.get_conversation(id).await.unwrap().archived);
            assert!(receiver.try_recv().is_err());
        }
        assert_eq!(manager.close_retirement_leases.lock().await.len(), 1);
        assert_close_admission_fenced(&manager, &scope).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER reject_failure_event")
            .execute(db.pool())
            .await
            .unwrap();

        for _ in 0..2 {
            assert_eq!(
                manager
                    .record_close_cleanup_failure::<()>(
                        &super::CloseRunRef::initial(attempt_id.clone()),
                        &snapshot,
                        &scope,
                        resource.clone(),
                        reason,
                        detail
                    )
                    .await,
                Err(detail.to_string())
            );
            assert_eq!(
                db.get_close_obligation(attempt_id.as_str())
                    .await
                    .unwrap()
                    .close_outcome(),
                Some(CloseCompletionOutcome::ArchivedCleanupAttention)
            );
            let failures = db
                .list_close_cleanup_failures(attempt_id.as_str())
                .await
                .unwrap();
            assert_eq!(failures.len(), 1);
            let remaining = &failures[0].occurrence.remaining_resources;
            assert_eq!(remaining.len(), 3);
            assert!(remaining.contains(&CloseCleanupFailureResource {
                scope: scope.clone(),
                resource: resource.clone(),
                disposition: CloseCleanupResourceDisposition::Failed,
            }));
            for captured_scope in [&scope, &later_scope] {
                assert!(remaining.contains(&CloseCleanupFailureResource {
                    scope: captured_scope.clone(),
                    resource: opaque_resource(
                        RetiredResourceKind::WorkScope,
                        captured_scope.as_str().to_string()
                    ),
                    disposition: CloseCleanupResourceDisposition::Unattempted,
                }));
            }
            for (id, receiver) in &mut receivers {
                assert!(db.get_conversation(id).await.unwrap().archived);
                assert!(matches!(
                    receiver.try_recv().unwrap(),
                    crate::runtime::SseEvent::ConversationUpdate {
                        update: crate::runtime::ConversationMetadataUpdate {
                            archived: Some(true),
                            ..
                        },
                        ..
                    }
                ));
                assert!(receiver.try_recv().is_err());
            }
            assert!(manager.close_retirement_leases.lock().await.is_empty());
            assert_close_admission_fenced(&manager, &scope).await;
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn process_cleanup_failure_preserves_exact_identity_and_unknown_candidates() {
        use super::*;
        use phoenix_core::domain::close::TranscriptConversationId;
        use std::sync::Arc;

        let bash = opaque_resource(
            RetiredResourceKind::BashProcessGroup,
            "epoch-A:pgid-17".into(),
        );
        let pty = opaque_resource(RetiredResourceKind::PtySession, "epoch-A:pty-19".into());
        let browser = opaque_resource(
            RetiredResourceKind::BrowserSession,
            "epoch-A:browser-23".into(),
        );
        let other_browser = opaque_resource(
            RetiredResourceKind::BrowserSession,
            "epoch-A:browser-29".into(),
        );
        for (failed, candidates, lease_resources) in [
            (
                Some(bash.clone()),
                vec![],
                Some(vec![bash.clone(), pty.clone(), browser.clone()]),
            ),
            (
                Some(pty.clone()),
                vec![],
                Some(vec![pty.clone(), browser.clone()]),
            ),
            (
                Some(browser.clone()),
                vec![],
                Some(vec![browser.clone(), other_browser.clone()]),
            ),
            (
                None,
                vec![browser.clone(), other_browser.clone()],
                Some(vec![browser.clone(), other_browser.clone(), pty.clone()]),
            ),
            (None, vec![], Some(vec![pty.clone()])),
            (None, vec![], None),
        ] {
            let db = crate::db::Database::open_in_memory().await.unwrap();
            let conversation = db
                .create_conversation("source", "source", "/tmp", true, None, None)
                .await
                .unwrap();
            let scope = conversation.attached_work_scope_id.clone().unwrap();
            let attempt_id = CloseAttemptId::parse("process-cleanup-failure").unwrap();
            db.begin_close_foundation(
                &conversation.product_conversation_id,
                &TranscriptConversationId::parse(&conversation.id).unwrap(),
                attempt_id.as_str(),
            )
            .await
            .unwrap();
            let manager = RuntimeManager::new(
                db.clone(),
                Arc::new(phoenix_llm::ModelRegistry::new_empty()),
                crate::platform::PlatformCapability::None {
                    details: "test".into(),
                },
                Arc::new(crate::tools::mcp::McpClientManager::new()),
                None,
            );
            let has_lease = lease_resources.is_some();
            let mut expected_candidates = candidates.clone();
            if let Some(resources) = lease_resources {
                manager
                    .acquire_close_resource_lease(&attempt_id, scope.clone())
                    .await
                    .unwrap();
                expected_candidates.extend(resources.clone());
                manager
                    .close_retirement_leases
                    .lock()
                    .await
                    .get_mut(&(attempt_id.as_str().to_string(), scope.clone()))
                    .unwrap()
                    .resources = resources;
            }
            let mut receiver = manager
                .conversation_broadcaster(&conversation.id)
                .await
                .subscribe();
            let detail = "process shutdown uncertain";
            assert_eq!(
                manager
                    .record_close_process_failure::<()>(
                        &super::CloseRunRef::initial(attempt_id.clone()),
                        &scope,
                        failed.clone(),
                        candidates,
                        detail,
                    )
                    .await,
                Err(detail.into())
            );
            let failures = db
                .list_close_cleanup_failures(attempt_id.as_str())
                .await
                .unwrap();
            assert_eq!(failures.len(), 1);
            let occurrence = &failures[0].occurrence;
            assert_eq!(
                occurrence.stop_certainty,
                CloseStopCertainty::ShutdownUncertain
            );
            let authority_resource = failed.clone().unwrap_or_else(|| {
                opaque_resource(RetiredResourceKind::WorkScope, scope.as_str().to_string())
            });
            assert_eq!(
                occurrence.authority,
                match &failed {
                    Some(resource) => CloseCleanupFailureAuthority::ObservedProcessResource {
                        scope: scope.clone(),
                        resource: resource.clone()
                    },
                    None => CloseCleanupFailureAuthority::CapturedScope {
                        scope: scope.clone(),
                        resource: authority_resource.clone()
                    },
                }
            );
            let mut expected = vec![CloseCleanupFailureResource {
                scope: scope.clone(),
                resource: authority_resource.clone(),
                disposition: if failed.is_some() {
                    CloseCleanupResourceDisposition::Failed
                } else {
                    CloseCleanupResourceDisposition::Unknown
                },
            }];
            for resource in expected_candidates {
                if !expected.iter().any(|target| target.resource == resource) {
                    expected.push(CloseCleanupFailureResource {
                        scope: scope.clone(),
                        resource,
                        disposition: CloseCleanupResourceDisposition::Unknown,
                    });
                }
            }
            RuntimeManager::order_close_failure_resources(
                &scope,
                &authority_resource,
                &mut expected,
            );
            assert_eq!(occurrence.remaining_resources, expected);
            assert_eq!(
                db.get_close_obligation(attempt_id.as_str())
                    .await
                    .unwrap()
                    .close_outcome(),
                Some(CloseCompletionOutcome::CloseIncomplete)
            );
            assert!(
                !db.get_conversation(&conversation.id)
                    .await
                    .unwrap()
                    .archived
            );
            if has_lease {
                assert_close_admission_fenced(&manager, &scope).await;
            }
            assert!(matches!(
                receiver.try_recv().unwrap(),
                crate::runtime::SseEvent::ConversationUpdate {
                    update: crate::runtime::ConversationMetadataUpdate {
                        archived: Some(false),
                        ..
                    },
                    ..
                }
            ));
            assert!(manager.close_retirement_leases.lock().await.is_empty());
        }
    }

    struct ProcessStepFixture {
        manager: super::RuntimeManager,
        run: super::CloseRunRef,
        scopes: Vec<super::WorkScopeId>,
        bash: Vec<super::RetiredResourceIdentity>,
        sockets: Vec<std::path::PathBuf>,
        fake: std::sync::Arc<phoenix_tools::tmux::fake_backend::FakeTmuxBackend>,
        terminal: std::sync::Arc<phoenix_terminal::session::TerminalHandle>,
        _relay: tokio::sync::OwnedSemaphorePermit,
        _socket_dir: tempfile::TempDir,
    }

    impl Drop for ProcessStepFixture {
        fn drop(&mut self) {
            // The fixture retains relay authority, so Close cannot reap this child.
            let _ =
                nix::sys::signal::kill(self.terminal.child_pid, nix::sys::signal::Signal::SIGKILL);
            let child_pid = self.terminal.child_pid;
            if matches!(
                nix::sys::wait::waitpid(child_pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
                Ok(nix::sys::wait::WaitStatus::StillAlive)
            ) {
                // PTY master fds are dropped after this fixture's Drop returns.
                std::thread::spawn(move || {
                    let _ = nix::sys::wait::waitpid(child_pid, None);
                });
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn process_step_fixture(two_scopes: bool) -> ProcessStepFixture {
        use super::*;
        use phoenix_core::domain::close::TranscriptConversationId;
        use phoenix_terminal::{
            session::Dims,
            spawn::{spawn_pty, PtyExecPlan},
        };
        use phoenix_tools::bash::handle::{FinalCause, Handle};
        use std::sync::Arc;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("process-steps", "process-steps", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut scopes = vec![source.attached_work_scope_id.clone().unwrap()];
        let mut latest_id = source.id.clone();
        if two_scopes {
            db.update_conversation_state(
                &source.id,
                &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                    summary: "continue".into(),
                },
            )
            .await
            .unwrap();
            let crate::db::ContinueOutcome::Created(latest) =
                db.continue_conversation(&source.id).await.unwrap()
            else {
                panic!("expected continuation");
            };
            let later_scope = WorkScopeId::parse("z-process-step-later-scope").unwrap();
            sqlx::query("INSERT INTO work_scopes (id, authority_kind, lifecycle, environment_kind, cwd, created_at, updated_at) SELECT ?1, 'work', 'active', 'unowned_cwd', '/tmp', created_at, updated_at FROM work_scopes WHERE id=?2")
                .bind(later_scope.as_str()).bind(scopes[0].as_str()).execute(db.pool()).await.unwrap();
            sqlx::query("UPDATE conversations SET work_scope_id=?1 WHERE id=?2")
                .bind(later_scope.as_str())
                .bind(&latest.id)
                .execute(db.pool())
                .await
                .unwrap();
            scopes.push(later_scope);
            latest_id = latest.id;
        }
        scopes.sort();
        let run = CloseRunRef::initial(CloseAttemptId::parse("process-step-run").unwrap());
        db.begin_close_foundation(
            &source.product_conversation_id,
            &TranscriptConversationId::parse(latest_id).unwrap(),
            run.attempt_id.as_str(),
        )
        .await
        .unwrap();
        db.confirm_close_stop_work(run.attempt_id.as_str())
            .await
            .unwrap();
        db.begin_close_active_work_settlement(run.attempt_id.as_str())
            .await
            .unwrap();
        db.advance_close_settlement_when_quiescent(run.attempt_id.as_str())
            .await
            .unwrap();
        let socket_dir = tempfile::tempdir().unwrap();
        let fake = phoenix_tools::tmux::fake_backend::FakeTmuxBackend::new();
        let mut manager = RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "process step tests".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        );
        manager.tmux_registry =
            Arc::new(phoenix_tools::tmux::registry::TmuxRegistry::with_backend(
                socket_dir.path().to_path_buf(),
                fake.clone(),
                None,
            ));
        let mut children = Vec::new();
        let mut sockets = Vec::new();
        for scope in &scopes {
            let key = ResourceScopeKey::Work(scope.clone());
            let child = tokio::process::Command::new("sleep")
                .arg("60")
                .process_group(0)
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let pid = child.id().unwrap();
            let mut reservation = manager.bash_handles().reserve_spawn(&key).await.unwrap();
            let handle = Handle::new_live(
                key.clone(),
                reservation.handle_id().clone(),
                "sleep 60".into(),
                None,
                PathBuf::from("/tmp"),
                i32::try_from(pid).unwrap(),
                pid,
                1024,
            );
            manager
                .bash_handles()
                .commit_spawn(&mut reservation, handle.clone())
                .await
                .unwrap();
            children.push((child, handle));
            let server = manager
                .tmux_registry()
                .ensure_live(&key, Path::new("/tmp"), None, None)
                .await
                .unwrap();
            sockets.push(server.read().await.socket_path.clone());
        }
        let terminal_scope = ResourceScopeKey::Work(scopes.last().unwrap().clone());
        let terminal = manager
            .terminals
            .try_insert_exact(
                terminal_scope.clone(),
                tokio::task::spawn_blocking(|| {
                    spawn_pty(
                        Path::new("/tmp"),
                        Dims::try_new(80, 24).unwrap(),
                        PtyExecPlan::Shell,
                    )
                    .unwrap()
                })
                .await
                .unwrap(),
            )
            .unwrap();
        let relay = terminal
            .attach_permit
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let snapshot = manager
            .inspect_close_retirement_only(run.attempt_id.clone())
            .await
            .unwrap();
        manager
            .capture_close_retirement_inventory(run.attempt_id.clone(), snapshot)
            .await
            .unwrap();
        let mut leases = manager.close_retirement_leases.lock().await;
        let bash = scopes
            .iter()
            .map(|scope| {
                leases[&(run.attempt_id.as_str().to_string(), scope.clone())]
                    .resources
                    .iter()
                    .find(|resource| resource.kind() == RetiredResourceKind::BashProcessGroup)
                    .unwrap()
                    .clone()
            })
            .collect();
        leases
            .get_mut(&(
                run.attempt_id.as_str().to_string(),
                scopes.last().unwrap().clone(),
            ))
            .unwrap()
            .terminal = manager
            .terminals
            .begin_retirement_by(&terminal_scope, tokio::time::Instant::now());
        drop(leases);
        for (mut child, handle) in children {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            handle
                .transition_to_terminal(
                    FinalCause::Killed {
                        exit_code: None,
                        signal_number: Some(libc::SIGKILL),
                    },
                    std::time::Duration::ZERO,
                    std::time::SystemTime::now(),
                    0,
                )
                .await;
        }
        ProcessStepFixture {
            manager,
            run,
            scopes,
            bash,
            sockets,
            fake,
            terminal,
            _relay: relay,
            _socket_dir: socket_dir,
        }
    }

    async fn assert_process_step_successes(fixture: &ProcessStepFixture, expected: usize) {
        let successes = fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap();
        assert_eq!(successes.len(), expected, "{successes:?}");
        for (scope, bash) in fixture.scopes.iter().zip(&fixture.bash).take(expected) {
            assert!(successes.iter().any(|success| success.run == fixture.run
                && success.scope == *scope
                && success.resource_kind == super::CloseProcessResourceKind::BashProcessGroup
                && super::LossItemIdentity::Opaque(success.identity.clone()) == *bash.identity()
                && success.outcome == super::CloseProcessStepOutcome::Retired));
        }
    }

    async fn assert_process_step_failure_omits_successes(fixture: &ProcessStepFixture) {
        let failures = fixture
            .manager
            .db()
            .list_close_cleanup_failures(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].occurrence.failure_occurrence_id,
            fixture.run.failure_occurrence_id()
        );
        for bash in &fixture.bash {
            assert!(!failures[0]
                .occurrence
                .remaining_resources
                .iter()
                .any(|remaining| remaining.resource == *bash));
        }
    }

    #[tokio::test]
    async fn process_step_stale_bash_generation_cannot_mint_success() {
        let fixture = process_step_fixture(false).await;
        let key = super::ResourceScopeKey::Work(fixture.scopes[0].clone());
        let _newer = fixture.manager.bash_handles().begin_retirement(&key).await;
        let error = fixture
            .manager
            .retire_close_runtime_resources(fixture.run.attempt_id.clone())
            .await
            .unwrap_err();
        assert!(error.contains("generation is stale"), "{error}");
        assert_process_step_successes(&fixture, 0).await;
        let failures = fixture
            .manager
            .db()
            .list_close_cleanup_failures(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert!(failures[0]
            .occurrence
            .remaining_resources
            .iter()
            .any(|item| item.resource == fixture.bash[0]
                && item.disposition == CloseCleanupResourceDisposition::Failed));
        assert_eq!(fixture.fake.kill_server_count(&fixture.sockets[0]), 0);
        assert_eq!(
            *fixture.terminal.stop_tx.borrow(),
            phoenix_terminal::session::StopReason::Running
        );
    }

    #[tokio::test]
    async fn process_step_bash_success_survives_tmux_failure() {
        let fixture = process_step_fixture(false).await;
        fixture.fake.stall(
            &fixture.sockets[0],
            phoenix_tools::tmux::fake_backend::Stall::AfterProbe,
        );
        let error = fixture
            .manager
            .retire_close_runtime_resources(fixture.run.attempt_id.clone())
            .await
            .unwrap_err();
        assert!(error.contains("tmux"), "{error}");
        assert_process_step_successes(&fixture, 1).await;
        assert_process_step_failure_omits_successes(&fixture).await;
        assert_eq!(
            *fixture.terminal.stop_tx.borrow(),
            phoenix_terminal::session::StopReason::Running
        );
        assert_eq!(fixture.fake.kill_server_count(&fixture.sockets[0]), 0);
    }

    #[tokio::test]
    async fn process_step_bash_and_tmux_success_survive_pty_failure() {
        let fixture = process_step_fixture(false).await;
        let error = fixture
            .manager
            .retire_close_runtime_resources(fixture.run.attempt_id.clone())
            .await
            .unwrap_err();
        assert!(error.contains("terminal relay"), "{error}");
        assert_process_step_successes(&fixture, 1).await;
        assert_process_step_failure_omits_successes(&fixture).await;
        let evidence = fixture
            .manager
            .db()
            .list_close_retirement_evidence(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert!(evidence
            .iter()
            .any(|evidence| evidence.scope == fixture.scopes[0]
                && evidence.resource.kind() == super::RetiredResourceKind::TmuxServer
                && evidence.outcome == super::RetirementOutcome::Retired));
        let failures = fixture
            .manager
            .db()
            .list_close_cleanup_failures(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert!(!failures[0]
            .occurrence
            .remaining_resources
            .iter()
            .any(|resource| resource.resource.kind() == super::RetiredResourceKind::TmuxServer));
        assert_eq!(fixture.fake.kill_server_count(&fixture.sockets[0]), 1);
    }

    #[tokio::test]
    async fn process_step_earlier_scope_success_survives_later_scope_failure() {
        let fixture = process_step_fixture(true).await;
        fixture.fake.stall(
            &fixture.sockets[1],
            phoenix_tools::tmux::fake_backend::Stall::AfterProbe,
        );
        let error = fixture
            .manager
            .retire_close_runtime_resources(fixture.run.attempt_id.clone())
            .await
            .unwrap_err();
        assert!(error.contains("tmux"), "{error}");
        assert_process_step_successes(&fixture, 2).await;
        assert_process_step_failure_omits_successes(&fixture).await;
        assert_eq!(fixture.fake.kill_server_count(&fixture.sockets[0]), 1);
        assert_eq!(fixture.fake.kill_server_count(&fixture.sockets[1]), 0);
        assert_eq!(
            *fixture.terminal.stop_tx.borrow(),
            phoenix_terminal::session::StopReason::Running
        );
    }

    #[tokio::test]
    async fn process_step_persistence_failure_stops_before_next_registry() {
        for fail_tmux_evidence in [false, true] {
            let fixture = process_step_fixture(false).await;
            let trigger = if fail_tmux_evidence {
                "CREATE TRIGGER reject_step_success BEFORE INSERT ON close_retirement_resources BEGIN SELECT RAISE(ABORT, 'injected step persistence failure'); END"
            } else {
                "CREATE TRIGGER reject_step_success BEFORE INSERT ON close_process_step_successes BEGIN SELECT RAISE(ABORT, 'injected step persistence failure'); END"
            };
            sqlx::query(trigger)
                .execute(fixture.manager.db().pool())
                .await
                .unwrap();
            let error = fixture
                .manager
                .retire_close_runtime_resources(fixture.run.attempt_id.clone())
                .await
                .unwrap_err();
            assert!(
                error.contains("injected step persistence failure"),
                "{error}"
            );
            assert_process_step_successes(&fixture, usize::from(fail_tmux_evidence)).await;
            assert_eq!(
                fixture.fake.kill_server_count(&fixture.sockets[0]),
                usize::from(fail_tmux_evidence)
            );
            assert_eq!(
                *fixture.terminal.stop_tx.borrow(),
                phoenix_terminal::session::StopReason::Running
            );
            assert!(fixture
                .manager
                .db()
                .list_close_cleanup_failures(fixture.run.attempt_id.as_str())
                .await
                .unwrap()
                .is_empty());
        }
    }

    #[tokio::test]
    async fn process_step_successes_survive_failure_transaction_rollback() {
        let fixture = process_step_fixture(false).await;
        sqlx::query("CREATE TRIGGER reject_step_failure_event BEFORE INSERT ON coordinator_watch_events BEGIN SELECT RAISE(ABORT, 'injected failure transaction rollback'); END")
            .execute(fixture.manager.db().pool()).await.unwrap();
        let error = fixture
            .manager
            .retire_close_runtime_resources(fixture.run.attempt_id.clone())
            .await
            .unwrap_err();
        assert!(
            error.contains("injected failure transaction rollback"),
            "{error}"
        );
        assert_process_step_successes(&fixture, 1).await;
        let evidence = fixture
            .manager
            .db()
            .list_close_retirement_evidence(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(
            evidence[0].resource.kind(),
            super::RetiredResourceKind::TmuxServer
        );
        assert!(fixture
            .manager
            .db()
            .list_close_cleanup_failures(fixture.run.attempt_id.as_str())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                .fetch_one(fixture.manager.db().pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            fixture.manager.close_retirement_leases.lock().await.len(),
            1
        );
        assert_eq!(fixture.fake.kill_server_count(&fixture.sockets[0]), 1);
        sqlx::query("DROP TRIGGER reject_step_failure_event")
            .execute(fixture.manager.db().pool())
            .await
            .unwrap();
        let obligation = fixture
            .manager
            .db()
            .get_close_obligation(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        let snapshot = obligation.snapshot().unwrap().clone();
        let expected = fixture
            .manager
            .db()
            .list_close_expected_retirement_resources(fixture.run.attempt_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .filter(|target| target.scope == fixture.scopes[0])
            .map(|target| target.resource)
            .collect::<Vec<_>>();
        let replay = fixture
            .manager
            .complete_close_resource_lease(&fixture.run, &snapshot, &fixture.scopes[0], &expected)
            .await;
        assert!(!format!("{replay:?}").contains("process step success payload mismatch"));
        assert_process_step_successes(&fixture, 1).await;
        assert_close_admission_fenced(&fixture.manager, &fixture.scopes[0]).await;
    }

    #[test]
    fn process_epoch_failure_does_not_invent_one_failed_resource_for_aggregate_errors() {
        use phoenix_core::domain::close::RetiredResourceKind;
        let resources = vec![
            super::opaque_resource(RetiredResourceKind::BrowserSession, "browser-a".to_string()),
            super::opaque_resource(RetiredResourceKind::BrowserSession, "browser-b".to_string()),
        ];
        let failure = CloseLeaseFailure::process_epoch(
            RetiredResourceKind::BrowserSession,
            resources.clone(),
            "batch failure".to_string(),
        );
        assert!(
            matches!(failure, CloseLeaseFailure::UnattributedProcessEpoch { captured_resources, .. } if captured_resources == resources)
        );
        assert!(
            matches!(CloseLeaseFailure::process_epoch(RetiredResourceKind::PtySession, vec![], "no captured target".to_string()), CloseLeaseFailure::UnattributedProcessEpoch { captured_resources, .. } if captured_resources.is_empty())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn full_descriptor_buffer_requires_larger_inventory() {
        assert!(super::descriptor_inventory_may_be_truncated(4096, 4096));
        assert!(super::descriptor_inventory_may_be_truncated(8192, 4096));
        assert!(!super::descriptor_inventory_may_be_truncated(4080, 4096));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_inventory_failures_are_absent_evidence() {
        assert_eq!(super::macos_all_pids_with(|_, _| -1), None);
        assert_eq!(super::macos_all_pids_with(|_, _| 3), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_inventory_retries_full_buffer_and_accepts_complete_list() {
        use std::mem::size_of;

        let mut calls = 0;
        let pids = super::macos_all_pids_with(|buffer, capacity_bytes| {
            calls += 1;
            if calls == 1 {
                return capacity_bytes;
            }
            let values = [17_i32, 23_i32];
            // SAFETY: the scanner supplied at least its initial 4096-PID buffer.
            unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), buffer.cast(), values.len()) };
            i32::try_from(values.len() * size_of::<i32>()).unwrap()
        });

        assert_eq!(pids, Some(vec![17, 23]));
        assert_eq!(calls, 2);
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
        let confirmed = inspect_worktree(&identity).await.unwrap().0;

        let outcome = quarantine_and_remove_exact_worktree(
            &identity,
            &confirmed,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            None,
            |_, _, _| Ok(()),
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

    #[cfg(unix)]
    fn open_directory(path: &Path) -> std::os::fd::OwnedFd {
        std::fs::File::open(path).unwrap().into()
    }

    #[cfg(unix)]
    fn preserved_private_slot(root: &Path) -> std::path::PathBuf {
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name().is_some_and(|name| {
                    name.as_encoded_bytes()
                        .starts_with(b".phoenix-delete-entry-")
                })
            })
            .expect("replacement must remain in an identity-bound private slot")
    }

    #[cfg(unix)]
    #[test]
    fn directory_child_swap_after_inspection_preserves_replacement_for_needs_repair() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        let displaced = temp.path().join("inspected-directory");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("original"), "original\n").unwrap();
        let descriptor = open_directory(&root);
        let mut swapped = false;

        let error = remove_directory_contents_at_with_hook(&descriptor, &mut |_, name| {
            if !swapped && name.to_bytes() == b"child" {
                std::fs::rename(&child, &displaced).unwrap();
                std::fs::create_dir(&child).unwrap();
                std::fs::write(child.join("replacement-marker"), "must survive\n").unwrap();
                swapped = true;
            }
        })
        .unwrap_err();

        assert!(error.contains("replaced before identity binding"));
        assert!(displaced.join("original").is_file());
        let preserved = preserved_private_slot(&root);
        assert_eq!(
            std::fs::read_to_string(preserved.join("replacement-marker")).unwrap(),
            "must survive\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_child_swap_after_inspection_preserves_replacement_for_needs_repair() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        let displaced = temp.path().join("inspected-file");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&child, "original\n").unwrap();
        let descriptor = open_directory(&root);
        let mut swapped = false;

        let error = remove_directory_contents_at_with_hook(&descriptor, &mut |_, name| {
            if !swapped && name.to_bytes() == b"child" {
                std::fs::rename(&child, &displaced).unwrap();
                std::fs::write(&child, "replacement must survive\n").unwrap();
                swapped = true;
            }
        })
        .unwrap_err();

        assert!(error.contains("replaced before identity binding"));
        assert_eq!(std::fs::read_to_string(&displaced).unwrap(), "original\n");
        assert_eq!(
            std::fs::read_to_string(preserved_private_slot(&root)).unwrap(),
            "replacement must survive\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tombstone_object_swap_after_observation_preserves_replacement_for_needs_repair() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("original"), "original\n").unwrap();
        let identity = observe_administrative_dir_incarnation(&target).unwrap();
        let displaced = temp.path().join("verified-object");

        let error = remove_identity_bound_directory(
            &target,
            None,
            &identity,
            observe_administrative_dir_incarnation,
            |_| {},
            |_, _, _| Ok(()),
            |_, object| {
                std::fs::rename(object, &displaced).unwrap();
                std::fs::create_dir(object).unwrap();
                std::fs::write(object.join("replacement-marker"), "must survive\n").unwrap();
                Ok(())
            },
            "test directory",
        )
        .unwrap_err();

        assert!(error.contains("object was replaced before descriptor binding"));
        assert!(displaced.join("original").is_file());
        let replacement = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("object/replacement-marker"))
            .find(|candidate| candidate.is_file())
            .expect("replacement must remain in the exact tombstone object slot");
        assert_eq!(
            std::fs::read_to_string(replacement).unwrap(),
            "must survive\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tombstone_root_swap_after_observation_preserves_replacement_for_needs_repair() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("original"), "original\n").unwrap();
        let identity = observe_administrative_dir_incarnation(&target).unwrap();
        let displaced_root = temp.path().join("verified-root");

        let error = remove_identity_bound_directory(
            &target,
            None,
            &identity,
            observe_administrative_dir_incarnation,
            |_| {},
            |_, _, _| Ok(()),
            |root, _| {
                std::fs::rename(root, &displaced_root).unwrap();
                std::fs::create_dir(root).unwrap();
                let replacement = root.join("object");
                std::fs::create_dir(&replacement).unwrap();
                std::fs::write(replacement.join("replacement-marker"), "must survive\n").unwrap();
                Ok(())
            },
            "test directory",
        )
        .unwrap_err();

        assert!(error.contains("root was replaced before descriptor binding"));
        assert!(displaced_root.join("object/original").is_file());
        assert_eq!(
            std::fs::read_to_string(
                std::fs::read_dir(temp.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path().join("object/replacement-marker"))
                    .find(|candidate| candidate.is_file())
                    .unwrap()
            )
            .unwrap(),
            "must survive\n"
        );
    }

    #[cfg(unix)]
    fn linked_close_worktree(temp: &tempfile::TempDir) -> std::path::PathBuf {
        let repository = temp.path().join("repository");
        initialize_repository(&repository);
        let target = temp.path().join("linked");
        run_git(
            &repository,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "close-target",
                target.to_str().unwrap(),
            ],
        );
        target
    }

    #[cfg(unix)]
    #[test]
    fn ignored_content_preserves_linked_worktree_without_a_loss_item() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        std::fs::write(target.join(".gitignore"), "cache/\n").unwrap();
        run_git(&target, &["add", ".gitignore"]);
        run_git(&target, &["commit", "--quiet", "-m", "ignore cache"]);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        std::fs::create_dir(target.join("cache")).unwrap();
        std::fs::write(target.join("cache/unique"), "preserve").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (fresh, losses) = runtime.block_on(inspect_worktree(&identity)).unwrap();
        assert_eq!(fresh.fingerprint(), confirmed.fingerprint());
        assert!(losses.is_empty());
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let administrative = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative).unwrap();
        let error = super::inspect_and_remove_exact_worktree(
            runtime.handle(),
            &identity,
            &confirmed,
            &administrative,
            &incarnation,
            None,
            |_, _, _| Ok(()),
        )
        .err()
        .expect("ignored data must prevent deletion");
        assert!(error.contains("ignored content"), "{error}");
        assert!(target.join("cache/unique").exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn initialized_submodule_ignored_content_preserves_linked_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let subrepo = temp.path().join("subrepo");
        initialize_repository(&subrepo);
        std::fs::write(subrepo.join(".gitignore"), "build/\n").unwrap();
        run_git(&subrepo, &["add", ".gitignore"]);
        run_git(&subrepo, &["commit", "--quiet", "-m", "ignore build"]);
        let repository = temp.path().join("repository");
        initialize_repository(&repository);
        run_git(
            &repository,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                subrepo.to_str().unwrap(),
                "nested",
            ],
        );
        run_git(&repository, &["commit", "--quiet", "-am", "add submodule"]);
        let target = temp.path().join("linked");
        run_git(
            &repository,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "close-target",
                target.to_str().unwrap(),
            ],
        );
        run_git(
            &target,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "--quiet",
            ],
        );
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        std::fs::create_dir(target.join("nested/build")).unwrap();
        std::fs::write(target.join("nested/build/unique"), "preserve").unwrap();
        assert!(super::ensure_no_ignored_content(&target)
            .unwrap_err()
            .contains("ignored content"));
        let fresh = test_worktree_snapshot(&identity, &target);
        assert_eq!(fresh.fingerprint(), confirmed.fingerprint());
        assert!(target.join("nested/build/unique").exists());
        std::fs::remove_dir_all(target.join("nested/build")).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let administrative = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let outcome = super::inspect_and_remove_exact_worktree(
            runtime.handle(),
            &identity,
            &confirmed,
            &administrative,
            &observe_administrative_dir_incarnation(&administrative).unwrap(),
            None,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert!(matches!(outcome, ExactWorktreeRemoval::Retired));
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn fsmonitor_stop_only_accepts_proven_no_daemon_or_success() {
        use std::os::unix::process::ExitStatusExt as _;
        let output = |code, stderr: &[u8]| std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.to_vec(),
        };
        assert_eq!(
            super::classify_fsmonitor_stop(&output(0, b"")).unwrap(),
            super::FsmonitorStop::Stopped
        );
        assert_eq!(
            super::classify_fsmonitor_stop(&output(
                128,
                b"fatal: fsmonitor--daemon is not running\n"
            ))
            .unwrap(),
            super::FsmonitorStop::NotRunning
        );
        assert!(
            super::classify_fsmonitor_stop(&output(128, b"fatal: permission denied\n")).is_err()
        );
        assert!(super::classify_fsmonitor_stop(&output(
            1,
            b"fatal: fsmonitor--daemon is not running\n"
        ))
        .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_fsmonitor_stop_never_reaches_quarantine_or_later_effects() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let administrative = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative).unwrap();
        let confirmed = inspect_worktree(&identity).await.unwrap().0;
        let later_effect = target.with_extension("later-effect");
        let result = super::quarantine_and_remove_exact_worktree_with_stop(
            &identity,
            &confirmed,
            administrative.clone(),
            incarnation,
            None,
            |_, _, _| panic!("must not bind a tombstone"),
            {
                let later_effect = later_effect.clone();
                move |_| {
                    std::fs::write(later_effect, "ran").unwrap();
                }
            },
            |_| Err("injected fsmonitor timeout".into()),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, ExactWorktreeRemoval::StopFailed { detail } if detail.contains("injected fsmonitor timeout"))
        );
        assert!(target.exists());
        assert!(administrative.exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
        assert!(!later_effect.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_completed_during_fsmonitor_stop_requires_reinspection_before_quarantine() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let tracked = target.join("tracked");
        std::fs::write(&tracked, "confirmed\n").unwrap();
        run_git(&target, &["add", "tracked"]);
        run_git(&target, &["commit", "--quiet", "-m", "tracked"]);
        let identity = inspection_identity(&target);
        let confirmed = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let administrative = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let result = super::quarantine_and_remove_exact_worktree_with_stop(
            &identity,
            &confirmed,
            administrative,
            observe_administrative_dir_incarnation(
                &super::exact_worktree_administrative_dir(&target, &common).unwrap(),
            )
            .unwrap(),
            None,
            |_, _, _| panic!("must not bind a tombstone"),
            |_| panic!("must not quarantine a changed worktree"),
            move |_| {
                std::fs::write(&tracked, "changed while stopping\n").unwrap();
                Ok(super::FsmonitorStop::Stopped)
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            result,
            ExactWorktreeRemoval::ReinspectionRequired { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(target.join("tracked")).unwrap(),
            "changed while stopping\n"
        );
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn safe_retry_writer_rejection_does_not_allocate_an_ordinal() {
        use super::*;
        use phoenix_core::domain::close::TranscriptConversationId;
        use std::io::Read as _;
        use std::sync::Arc;

        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        std::fs::write(target.join("held"), "stable\n").unwrap();
        run_git(&target, &["add", "held"]);
        run_git(&target, &["commit", "--quiet", "-m", "held"]);
        let target_text = target.to_string_lossy().into_owned();
        let identity = inspection_identity(&target);
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation(
                "writer-retry",
                "writer-retry",
                &target_text,
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let scope = conversation.attached_work_scope_id.unwrap();
        sqlx::query("UPDATE work_scopes SET environment_kind='allocated_worktree', worktree_path=?1, worktree_id=?2, worktree_fingerprint=?3, branch_name='close-target', base_branch='main' WHERE id=?4")
            .bind(&target_text)
            .bind(identity.id().as_str())
            .bind(identity.fingerprint().as_str())
            .bind(scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        let attempt_id = CloseAttemptId::parse("writer-retry-attempt").unwrap();
        db.begin_close_foundation(
            &conversation.product_conversation_id,
            &TranscriptConversationId::parse(&conversation.id).unwrap(),
            attempt_id.as_str(),
        )
        .await
        .unwrap();
        db.confirm_close_stop_work(attempt_id.as_str())
            .await
            .unwrap();
        db.begin_close_active_work_settlement(attempt_id.as_str())
            .await
            .unwrap();
        db.advance_close_settlement_when_quiescent(attempt_id.as_str())
            .await
            .unwrap();
        let confirmed = inspect_worktree(&identity).await.unwrap().0;
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: attempt_id.clone(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: confirmed.clone(),
                losses: vec![],
            }],
        })
        .await
        .unwrap();
        let snapshot = db
            .get_close_obligation(attempt_id.as_str())
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
        db.capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
            attempt_id: attempt_id.clone(),
            snapshot: snapshot.clone(),
            scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                scope: scope.clone(),
                inventory: CloseOwnedResourceInventory {
                    worktree: Some(identity.clone()),
                    work_scopes: BTreeSet::new(),
                    bash_process_groups: BTreeSet::new(),
                    tmux_servers: BTreeSet::new(),
                    pty_sessions: BTreeSet::new(),
                    browser_sessions: BTreeSet::new(),
                    equivalent_live_resources: BTreeSet::new(),
                },
            }],
        })
        .await
        .unwrap();
        let manager = RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        );
        manager
            .acquire_close_resource_lease(&attempt_id, scope.clone())
            .await
            .unwrap();
        let worktree = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(identity),
        )
        .unwrap();
        let detail = "injected initial worktree cleanup failure";
        assert_eq!(
            manager
                .record_close_cleanup_failure::<()>(
                    &CloseRunRef::initial(attempt_id.clone()),
                    &snapshot,
                    &scope,
                    worktree,
                    RetirementFailureReason::RemovalFailed,
                    detail,
                )
                .await,
            Err(detail.into())
        );

        let mut child = std::process::Command::new("sh")
            .args(["-c", "exec 3<>held; printf ready; exec sleep 15"])
            .current_dir(&target)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready");
        let initial = CloseRunRef::initial(attempt_id.clone());
        assert!(manager
            .retry_close_runtime_resources(initial.clone())
            .await
            .unwrap_err()
            .contains("external writer"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id=?1"
            )
            .bind(attempt_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );

        child.kill().unwrap();
        child.wait().unwrap();
        manager
            .retry_close_runtime_resources(initial)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id=?1"
            )
            .bind(attempt_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap(),
            2
        );
        assert!(!target.exists());
    }

    #[cfg(unix)]
    fn test_worktree_snapshot(
        identity: &WorktreeIdentity,
        path: &Path,
    ) -> phoenix_core::domain::close::CloseRetirementSnapshot {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(super::inspect_worktree_at(identity, path.to_path_buf()))
            .unwrap()
            .0
    }

    #[cfg(unix)]
    #[test]
    fn recorded_final_tombstone_resumes_after_failure_following_rename() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        initialize_repository(&target);
        std::fs::write(target.join("original"), "original\n").unwrap();
        run_git(&target, &["add", "original"]);
        run_git(&target, &["commit", "--quiet", "-m", "original"]);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        let recorded = std::sync::Mutex::new(None::<CloseWorktreeFinalTombstone>);

        let error = remove_identity_bound_directory(
            &target,
            None,
            identity.fingerprint().as_str(),
            |path| {
                observe_worktree_fingerprint(path)
                    .ok_or_else(|| "missing worktree identity".to_string())
            },
            |_| {},
            |root, (device, inode), object| {
                let mut recorded = recorded.lock().unwrap();
                if let Some((object_device, object_inode)) = object {
                    let binding = recorded.as_mut().expect("root bound before object");
                    binding.object_device = Some(object_device);
                    binding.object_inode = Some(object_inode);
                    Err("injected failure after final rename".to_string())
                } else {
                    *recorded = Some(CloseWorktreeFinalTombstone {
                        root: root.to_path_buf(),
                        device,
                        inode,
                        object_device: None,
                        object_inode: None,
                    });
                    Ok(())
                }
            },
            |_, _| Ok(()),
            "test directory",
        )
        .unwrap_err();
        assert!(error.contains("injected failure"));
        assert!(!target.exists());
        let recorded = recorded.into_inner().unwrap().unwrap();
        assert!(recorded.root.join("object/original").is_file());
        std::fs::write(
            recorded.root.join("object/original"),
            "changed after approval\n",
        )
        .unwrap();
        assert!(matches!(
            resume_final_worktree_tombstone(&recorded, &identity, &confirmed),
            FinalTombstoneRecovery::Residual(_)
        ));
        assert!(recorded.root.join("object/original").exists());
        std::fs::write(recorded.root.join("object/original"), "original\n").unwrap();
        std::fs::write(recorded.root.join("object/.gitignore"), "cache/\n").unwrap();
        std::fs::create_dir(recorded.root.join("object/cache")).unwrap();
        std::fs::write(recorded.root.join("object/cache/unique"), "unique").unwrap();
        assert!(matches!(
            resume_final_worktree_tombstone(&recorded, &identity, &confirmed),
            FinalTombstoneRecovery::Residual(_)
        ));
        assert!(recorded.root.join("object/cache/unique").exists());
        std::fs::remove_dir_all(recorded.root.join("object/cache")).unwrap();
        std::fs::remove_file(recorded.root.join("object/.gitignore")).unwrap();
        let recovery = resume_final_worktree_tombstone(&recorded, &identity, &confirmed);
        assert!(
            matches!(recovery, FinalTombstoneRecovery::Completed),
            "{recovery:?}"
        );
        assert!(!recorded.root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn recorded_final_tombstone_root_mismatch_preserves_both_roots() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        initialize_repository(&target);
        std::fs::write(target.join("original"), "original\n").unwrap();
        run_git(&target, &["add", "original"]);
        run_git(&target, &["commit", "--quiet", "-m", "original"]);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        let recorded = std::sync::Mutex::new(None::<CloseWorktreeFinalTombstone>);
        let _ = remove_identity_bound_directory(
            &target,
            None,
            identity.fingerprint().as_str(),
            |path| {
                observe_worktree_fingerprint(path)
                    .ok_or_else(|| "missing worktree identity".to_string())
            },
            |_| {},
            |root, (device, inode), object| {
                let mut recorded = recorded.lock().unwrap();
                if let Some((object_device, object_inode)) = object {
                    let binding = recorded.as_mut().expect("root bound before object");
                    binding.object_device = Some(object_device);
                    binding.object_inode = Some(object_inode);
                    Err("injected failure after final rename".to_string())
                } else {
                    *recorded = Some(CloseWorktreeFinalTombstone {
                        root: root.to_path_buf(),
                        device,
                        inode,
                        object_device: None,
                        object_inode: None,
                    });
                    Ok(())
                }
            },
            |_, _| Ok(()),
            "test directory",
        );
        let recorded = recorded.into_inner().unwrap().unwrap();
        let displaced = temp.path().join("recorded-root");
        std::fs::rename(&recorded.root, &displaced).unwrap();
        std::fs::create_dir(&recorded.root).unwrap();
        std::fs::write(recorded.root.join("replacement-marker"), "preserve\n").unwrap();

        let recovery = resume_final_worktree_tombstone(&recorded, &identity, &confirmed);
        assert!(matches!(
            recovery,
            FinalTombstoneRecovery::Residual(detail) if detail.contains("root was replaced")
        ));
        assert!(displaced.join("object/original").is_file());
        assert!(recorded.root.join("replacement-marker").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn recorded_final_tombstone_root_only_resumes_pre_rename() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        initialize_repository(&target);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        let root = temp.path().join("private-tombstone");
        std::fs::create_dir(&root).unwrap();
        let metadata = std::fs::symlink_metadata(&root).unwrap();
        let recorded = CloseWorktreeFinalTombstone {
            root,
            device: metadata.dev(),
            inode: metadata.ino(),
            object_device: None,
            object_inode: None,
        };

        let recovery = resume_final_worktree_tombstone(&recorded, &identity, &confirmed);
        assert!(
            matches!(recovery, FinalTombstoneRecovery::Completed),
            "{recovery:?}"
        );
        assert!(!target.exists());
        assert!(!recorded.root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn recorded_final_tombstone_root_only_preserves_unbound_object() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        initialize_repository(&target);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        let root = temp.path().join("private-tombstone");
        std::fs::create_dir(&root).unwrap();
        let metadata = std::fs::symlink_metadata(&root).unwrap();
        let recorded = CloseWorktreeFinalTombstone {
            root,
            device: metadata.dev(),
            inode: metadata.ino(),
            object_device: None,
            object_inode: None,
        };
        std::fs::rename(&target, recorded.root.join("object")).unwrap();

        assert!(matches!(
            resume_final_worktree_tombstone(&recorded, &identity, &confirmed),
            FinalTombstoneRecovery::Residual(detail) if detail.contains("unbound object remains")
        ));
        assert!(recorded.root.join("object").exists());
    }

    #[cfg(unix)]
    #[test]
    fn recorded_final_tombstone_root_only_converges_receipt_missing_after_completed_delete() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        initialize_repository(&target);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        let root = temp.path().join("private-tombstone");
        std::fs::create_dir(&root).unwrap();
        let metadata = std::fs::symlink_metadata(&root).unwrap();
        let recorded = CloseWorktreeFinalTombstone {
            root,
            device: metadata.dev(),
            inode: metadata.ino(),
            object_device: None,
            object_inode: None,
        };
        std::fs::remove_dir_all(&target).unwrap();
        std::fs::remove_dir(&recorded.root).unwrap();

        let recovery = resume_final_worktree_tombstone(&recorded, &identity, &confirmed);
        assert!(
            matches!(recovery, FinalTombstoneRecovery::Completed),
            "{recovery:?}"
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
                None,
                |_, _, _| Ok(()),
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
    async fn hidden_assume_unchanged_content_is_loss_and_preserves_real_index_flag() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(
            temp.path(),
            &["update-index", "--assume-unchanged", "tracked"],
        );
        std::fs::write(temp.path().join("tracked"), "hidden content\n").unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();

        assert!(losses.iter().any(|loss| matches!(loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"tracked"
        )));
        let flags = phoenix_core::git::command()
            .args(["ls-files", "-v", "--", "tracked"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(flags.stdout.starts_with(b"h "));
    }

    #[tokio::test]
    async fn hidden_skip_worktree_content_is_loss_and_clean_path_is_not() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        std::fs::write(temp.path().join("clean"), "clean\n").unwrap();
        run_git(temp.path(), &["add", "clean"]);
        run_git(temp.path(), &["commit", "--quiet", "-m", "add clean"]);
        run_git(
            temp.path(),
            &["update-index", "--skip-worktree", "tracked", "clean"],
        );
        std::fs::write(temp.path().join("tracked"), "hidden content\n").unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();

        assert!(losses.iter().any(|loss| matches!(loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"tracked"
        )));
        assert!(!losses.iter().any(|loss| matches!(loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"clean"
        )));
        let flags = phoenix_core::git::command()
            .args(["ls-files", "-v", "--", "tracked", "clean"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(flags
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .all(|line| line.starts_with(b"S ")));
    }

    #[tokio::test]
    async fn sparse_checkout_absent_skip_worktree_path_is_not_loss_but_present_modified_one_is() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        std::fs::create_dir_all(temp.path().join("sparse")).unwrap();
        std::fs::write(temp.path().join("sparse/kept.txt"), "kept\n").unwrap();
        std::fs::write(temp.path().join("sparse/hidden.txt"), "hidden\n").unwrap();
        run_git(
            temp.path(),
            &["add", "sparse/kept.txt", "sparse/hidden.txt"],
        );
        run_git(
            temp.path(),
            &["commit", "--quiet", "-m", "add sparse files"],
        );
        run_git(temp.path(), &["sparse-checkout", "init", "--no-cone"]);
        std::fs::write(
            temp.path().join(".git/info/sparse-checkout"),
            "/tracked\n/sparse/kept.txt\n",
        )
        .unwrap();
        run_git(temp.path(), &["read-tree", "-mu", "HEAD"]);

        assert!(!temp.path().join("sparse/hidden.txt").exists());
        let identity = inspection_identity(temp.path());
        let (first, first_losses) = inspect_worktree(&identity).await.unwrap();
        let (second, second_losses) = inspect_worktree(&identity).await.unwrap();
        assert!(first_losses.is_empty());
        assert!(second_losses.is_empty());
        assert_eq!(first, second);

        std::fs::write(
            temp.path().join("sparse/hidden.txt"),
            "modified while present\n",
        )
        .unwrap();
        let (_, present_losses) = inspect_worktree(&identity).await.unwrap();
        assert!(present_losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"sparse/hidden.txt"
        )));
    }

    #[tokio::test]
    async fn hidden_pathspec_magic_name_is_treated_literally() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let name = ":(exclude)hidden.txt";
        std::fs::write(temp.path().join(name), "original\n").unwrap();
        run_git(temp.path(), &["add", "--", name]);
        run_git(temp.path(), &["commit", "--quiet", "-m", "tracked"]);
        run_git(
            temp.path(),
            &["update-index", "--assume-unchanged", "--", name],
        );
        std::fs::write(temp.path().join(name), "changed\n").unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();

        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == name.as_bytes()
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hidden_assume_unchanged_executable_mode_is_loss() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(
            temp.path(),
            &["update-index", "--assume-unchanged", "tracked"],
        );
        let tracked = temp.path().join("tracked");
        let mut permissions = std::fs::metadata(&tracked).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        std::fs::set_permissions(&tracked, permissions).unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(losses.iter().any(|loss| matches!(loss,
            CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"tracked"
        )));
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

    #[cfg(unix)]
    #[tokio::test]
    async fn dirty_executable_bit_changes_invalidate_snapshot_without_content_change() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let tracked = temp.path().join("tracked");
        std::fs::write(&tracked, "unchanged dirty payload\n").unwrap();
        let identity = inspection_identity(temp.path());
        let (first, first_losses) = inspect_worktree(&identity).await.unwrap();

        let mut permissions = std::fs::metadata(&tracked).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        std::fs::set_permissions(&tracked, permissions).unwrap();
        let (second, second_losses) = inspect_worktree(&identity).await.unwrap();

        assert_eq!(first_losses, second_losses);
        assert_ne!(first, second);
    }

    #[test]
    fn detached_unreachable_commits_excludes_the_exact_stash_ref() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        std::fs::write(temp.path().join("tracked"), "stash me\n").unwrap();
        run_git(
            temp.path(),
            &["stash", "push", "--quiet", "-m", "close-test"],
        );
        run_git(
            temp.path(),
            &["checkout", "--quiet", "--detach", "refs/stash"],
        );
        let head = phoenix_core::git::command()
            .args(["rev-parse", "HEAD"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(head.status.success());

        assert!(
            super::detached_unreachable_commits(temp.path(), head.stdout.trim_ascii())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn detached_unreachable_commits_tolerates_an_absent_stash_ref() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let head = phoenix_core::git::command()
            .args(["rev-parse", "HEAD"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(head.status.success());

        assert!(super::detached_unreachable_commits(temp.path(), head.stdout.trim_ascii()).is_ok());
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
    async fn unborn_symbolic_head_has_no_detached_commit_loss() {
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init", "--quiet"]);

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .expect("unborn symbolic HEAD remains inspectable");

        assert!(!losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));
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
    async fn initialized_unmerged_gitlink_is_inspected_and_reported_as_loss() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child-unmerged-gitlink");
        let parent = temp.path().join("parent-unmerged-gitlink");
        initialize_repository(&child);
        initialize_repository(&parent);
        let output = phoenix_core::git::command()
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                child.to_str().unwrap(),
                "deps/child",
            ])
            .current_dir(&parent)
            .output()
            .unwrap();
        assert!(output.status.success());
        run_git(&parent, &["commit", "--quiet", "-am", "add submodule"]);

        let submodule = parent.join("deps/child");
        std::fs::write(submodule.join("nested-untracked"), "must be inventoried\n").unwrap();
        let oid = phoenix_core::git::command()
            .args(["rev-parse", "HEAD"])
            .current_dir(&submodule)
            .output()
            .unwrap();
        assert!(oid.status.success());
        let oid = std::str::from_utf8(oid.stdout.trim_ascii()).unwrap();
        run_git(&parent, &["update-index", "--remove", "deps/child"]);
        for stage in [1, 2, 3] {
            run_git(
                &parent,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("160000,{oid},{stage}"),
                    "deps/child",
                ],
            );
        }

        let (_, losses) = inspect_worktree(&inspection_identity(&parent))
            .await
            .unwrap();
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::InitializedSubmoduleState(path) if path.as_bytes() == b"deps/child"
        )));
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::UntrackedNonIgnoredPath(path)
                if path.as_bytes() == b"deps/child/nested-untracked"
        )));
    }

    #[tokio::test]
    async fn populated_gitlink_without_metadata_is_reported_as_loss() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child-missing-metadata");
        let parent = temp.path().join("parent-missing-metadata");
        initialize_repository(&child);
        initialize_repository(&parent);
        let output = phoenix_core::git::command()
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                child.to_str().unwrap(),
                "deps/child",
            ])
            .current_dir(&parent)
            .output()
            .unwrap();
        assert!(output.status.success());
        run_git(&parent, &["commit", "--quiet", "-am", "add submodule"]);
        std::fs::remove_file(parent.join("deps/child/.git")).unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(&parent))
            .await
            .unwrap();

        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::InitializedSubmoduleState(path)
                if path.as_bytes() == b"deps/child"
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
        let (confirmed, losses) = inspect_worktree(&identity).await.unwrap();
        assert!(losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));
        let common = super::exact_worktree_common_git_dir(&closing).unwrap();
        let administrative_dir = exact_worktree_administrative_dir(&closing, &common).unwrap();
        let outcome = quarantine_and_remove_exact_worktree(
            &identity,
            &confirmed,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            None,
            |_, _, _| Ok(()),
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_status_parser_returns_only_the_effective_uid() {
        assert_eq!(
            super::LinuxProcessEffectiveUid::parse_status(
                "Name:\ttest\nUid:\t1000\t1001\t1002\t1003\nGid:\t2000\t2001\t2002\t2003\n",
            ),
            Ok(super::LinuxProcessEffectiveUid(1001))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_status_parser_rejects_missing_malformed_and_ambiguous_credentials() {
        for status in [
            "Name:\ttest\n",
            "Uid:\t1000\t1000\t1000\n",
            "Uid:\t1000\t1000\t1000\t1000\t1000\n",
            "Uid:\t1000\tnot-a-uid\t1000\t1000\n",
            "Uid:\t1000\t1000\t1000\t1000\nUid:\t2000\t2000\t2000\t2000\n",
        ] {
            assert!(
                super::LinuxProcessEffectiveUid::parse_status(status).is_err(),
                "accepted ambiguous status: {status:?}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_nondumpable_process_helper() {
        if std::env::var_os("PHOENIX_NONDUMPABLE_PROCESS_HELPER").is_none() {
            return;
        }
        let cwd = std::env::var_os("PHOENIX_NONDUMPABLE_CWD").unwrap();
        std::env::set_current_dir(cwd).unwrap();
        // SAFETY: `prctl(PR_SET_DUMPABLE, 0)` has no pointer arguments.
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) }, 0);
        println!("ready");
        std::io::stdout().flush().unwrap();
        let _ = std::io::stdin().read(&mut [0_u8]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cwd_scan_treats_same_user_nondumpable_process_as_observational() {
        let temp = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::close_retirement::tests::linux_nondumpable_process_helper",
                "--nocapture",
            ])
            .env("PHOENIX_NONDUMPABLE_PROCESS_HELPER", "1")
            .env("PHOENIX_NONDUMPABLE_CWD", temp.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "helper exited before ready"
            );
            if line.trim() == "ready" {
                break;
            }
        }

        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&proc_root).unwrap();
        std::os::unix::fs::symlink(
            format!("/proc/{}", child.id()),
            proc_root.join(child.id().to_string()),
        )
        .unwrap();
        // SAFETY: `geteuid` has no preconditions.
        let effective_uid = unsafe { libc::geteuid() };
        let process = std::fs::read_dir(&proc_root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            super::linux_process_owner(&process, effective_uid, "working-directory").unwrap(),
            super::LinuxProcessOwner::Relevant,
            "same-user nondumpable process must be attributed from kernel credentials"
        );
        let scan = super::quarantine_has_process_cwd_in(temp.path(), &proc_root, effective_uid);

        drop(child.stdin.take());
        child.wait().unwrap();
        assert!(
            scan.is_ok(),
            "same-user nondumpable process unreadability must remain observational: {scan:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn descriptor_scan_skips_unreadable_links() {
        let worktree = Path::new("/quarantine/worktree");
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidData,
        ] {
            assert!(!super::linux_descriptor_target_is_within(
                Err(std::io::Error::from(kind)),
                worktree,
            ));
        }
        assert!(super::linux_descriptor_target_is_within(
            Ok(worktree.join("open-file")),
            worktree,
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn descriptor_scan_ignores_ambient_failures_and_finds_readable_writer() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        let ambient = proc_root.join("1273");
        let writer = proc_root.join("1274");
        std::fs::create_dir_all(&ambient).unwrap();
        std::fs::create_dir_all(writer.join("fd")).unwrap();
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::write(ambient.join("fd"), b"not a descriptor directory").unwrap();
        std::os::unix::fs::symlink(quarantine.join("open-file"), writer.join("fd/3")).unwrap();

        assert_eq!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::PositiveWriterFound,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn descriptor_scan_reports_no_evidence_for_unavailable_or_unrelated_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();

        assert_eq!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::NoPositiveEvidence,
        );

        let descriptors = proc_root.join("1273/fd");
        std::fs::create_dir_all(&descriptors).unwrap();
        std::os::unix::fs::symlink(temp.path().join("outside"), descriptors.join("3")).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), descriptors.join("4")).unwrap();
        std::fs::create_dir_all(proc_root.join("self/fd")).unwrap();
        std::os::unix::fs::symlink(quarantine.join("ignored"), proc_root.join("self/fd/5"))
            .unwrap();

        assert_eq!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::NoPositiveEvidence,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cwd_scan_ignores_ambient_unreadability_and_finds_readable_writer() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        let unreadable_status = proc_root.join("1");
        let unreadable_cwd = proc_root.join("2");
        let writer = proc_root.join("3");
        std::fs::create_dir_all(unreadable_status.join("status")).unwrap();
        std::fs::create_dir_all(&unreadable_cwd).unwrap();
        std::fs::create_dir_all(&writer).unwrap();
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::write(
            unreadable_cwd.join("status"),
            "Name:\ttest\nUid:\t1\t1\t1\t1\n",
        )
        .unwrap();
        std::fs::write(unreadable_cwd.join("cwd"), b"not a symlink").unwrap();
        std::fs::write(writer.join("status"), "Name:\twriter\nUid:\t1\t1\t1\t1\n").unwrap();
        std::os::unix::fs::symlink(quarantine.join("nested"), writer.join("cwd")).unwrap();

        assert!(super::quarantine_has_process_cwd_in(&quarantine, &proc_root, 1).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mapping_scan_ignores_ambient_unreadability_and_finds_readable_writer() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        let unreadable_status = proc_root.join("1");
        let unreadable_maps = proc_root.join("2");
        let writer = proc_root.join("3");
        std::fs::create_dir_all(unreadable_status.join("status")).unwrap();
        std::fs::create_dir_all(unreadable_maps.join("maps")).unwrap();
        std::fs::create_dir_all(&writer).unwrap();
        std::fs::create_dir(&quarantine).unwrap();
        for process in [&unreadable_maps, &writer] {
            std::fs::write(process.join("status"), "Name:\ttest\nUid:\t1\t1\t1\t1\n").unwrap();
        }
        std::fs::write(
            writer.join("maps"),
            format!(
                "00000000-00001000 rw-s 00000000 00:00 1 {}\n",
                quarantine.join("mapped-file").display()
            ),
        )
        .unwrap();

        assert!(super::quarantine_has_writable_mappings_in(&quarantine, &proc_root, 1).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cwd_and_mapping_scans_treat_unavailable_proc_inventory_as_no_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::write(&proc_root, b"not a proc directory").unwrap();

        assert!(!super::quarantine_has_process_cwd_in(&quarantine, &proc_root, 1).unwrap());
        assert!(!super::quarantine_has_writable_mappings_in(&quarantine, &proc_root, 1).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cwd_and_mapping_scans_keep_quarantine_canonicalization_authoritative() {
        let temp = tempfile::tempdir().unwrap();
        let missing_quarantine = temp.path().join("missing-quarantine");
        let proc_root = temp.path().join("proc");

        assert!(super::quarantine_has_process_cwd_in(&missing_quarantine, &proc_root, 1).is_err());
        assert!(
            super::quarantine_has_writable_mappings_in(&missing_quarantine, &proc_root, 1).is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mapping_scan_detects_writable_shared_mapping_after_descriptor_is_closed() {
        use std::os::fd::AsRawFd as _;

        let temp = tempfile::tempdir().unwrap();
        let mapped_file = temp.path().join("mapped");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&mapped_file)
            .unwrap();
        file.set_len(4096).unwrap();
        // SAFETY: the file is at least 4096 bytes, and the result is checked before use.
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        drop(file);

        assert_eq!(
            super::quarantine_has_open_descriptors(temp.path()).unwrap(),
            super::ExternalWriterEvidence::NoPositiveEvidence,
        );
        assert!(super::quarantine_has_writable_mappings(temp.path()).unwrap());

        // SAFETY: `mapping` is the successful result of the matching 4096-byte mmap call.
        assert_eq!(unsafe { libc::munmap(mapping, 4096) }, 0);
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
        let temp = tempfile::tempdir().unwrap();
        let closing = temp.path().join("closing");
        initialize_repository(&closing);
        let tracked = closing.join("tracked");
        std::fs::write(&tracked, "before\n").unwrap();
        run_git(&closing, &["add", "tracked"]);
        run_git(&closing, &["commit", "--quiet", "-m", "tracked"]);
        let identity = inspection_identity(&closing);
        let confirmed = inspect_worktree(&identity).await.unwrap().0;
        let mut descriptor = std::fs::OpenOptions::new()
            .append(true)
            .open(&tracked)
            .unwrap();

        let administrative_dir = closing.join(".git");
        let outcome = quarantine_and_remove_exact_worktree(
            &identity,
            &confirmed,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            None,
            |_, _, _| Ok(()),
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
    fn top_level_status_probe_disables_configured_fsmonitor() {
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

        let status = run_bounded_git_status_until(
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )
        .unwrap();

        assert!(status.status.success());
        assert!(
            !marker.exists(),
            "authoritative Close inspection must not invoke the configured fsmonitor hook"
        );
        assert!(gate.exists());
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
    fn porcelain_rename_preserves_source_deletion_but_copy_does_not() {
        let rename_losses = parse_status_losses(b"R  new-name.txt\0old-name.txt\0");
        assert_eq!(
            rename_losses,
            vec![
                CloseLossItem::StagedTrackedPath(GitPathIdentity::from_bytes(
                    b"new-name.txt".to_vec()
                )),
                CloseLossItem::StagedTrackedPath(GitPathIdentity::from_bytes(
                    b"old-name.txt".to_vec()
                )),
            ]
        );

        let copy_losses = parse_status_losses(b"C  copy-name.txt\0source-name.txt\0");
        assert_eq!(
            copy_losses,
            vec![CloseLossItem::StagedTrackedPath(
                GitPathIdentity::from_bytes(b"copy-name.txt".to_vec())
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
        let resource = super::opaque_resource(
            phoenix_core::domain::close::RetiredResourceKind::BrowserSession,
            "exact-browser-launch-and-profile".to_string(),
        );
        let process_epoch = CloseLeaseFailure::process_epoch(
            resource.kind(),
            vec![resource.clone()],
            "profile identity changed".to_string(),
        );
        let tmux = CloseLeaseFailure::Tmux {
            reason: phoenix_core::domain::close::RetirementFailureReason::IdentityNotProven,
            detail: "server token changed".to_string(),
        };

        assert!(matches!(
            process_epoch,
            CloseLeaseFailure::ProcessEpoch { resource: failed, .. } if failed == resource
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
