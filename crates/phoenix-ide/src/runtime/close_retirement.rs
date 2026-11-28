//! Exact-instance resource fences and receipts for Close retirement.

pub(crate) mod captured_loss_retry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriterInspectionGap {
    pub(crate) path: std::path::PathBuf,
    pub(crate) detector: CloseWriterDetector,
    pub(crate) reason: CloseWriterGap,
    pub(crate) pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetryAssessmentError {
    WriterInspectionGap(WriterInspectionGap),
    Other(String),
}

impl From<String> for RetryAssessmentError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl From<&str> for RetryAssessmentError {
    fn from(error: &str) -> Self {
        Self::Other(error.to_owned())
    }
}

impl std::fmt::Display for RetryAssessmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WriterInspectionGap(gap) => write!(
                formatter,
                "incomplete external-writer inspection at {}; no new run admitted",
                gap.path.display()
            ),
            Self::Other(error) => formatter.write_str(error),
        }
    }
}
mod native_retry;
pub(crate) use native_retry::{
    NativeInitialWorkScopePrecondition, NativeRetryNoLiveResourcesError,
};
pub(crate) mod native_initial;

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use sha2::{Digest, Sha256};

use phoenix_core::domain::close::{
    AbsenceBasis, CapturedWorktreeIdentity, CloseAttemptId, CloseAttemptScope,
    CloseCompletionOutcome, CloseExpectedRetirementResource, CloseLossItem,
    CloseOwnedResourceInventory, ClosePhase, CloseRetiredResource, CloseRetirementSnapshot,
    CloseRetirementTarget, CloseRunOrdinal, CloseRunRef, CloseRunStatus, CloseStopCertainty,
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
    BindCloseWorktreeFinalTombstoneObjectRequest, BindCloseWorktreeFinalTombstoneRequest,
    CaptureCloseRetirementInventoryRequest, CaptureCloseRetirementInventoryScopeRequest,
    CloseCleanupFailureAuthority, CloseCleanupFailureResource, CloseCleanupResourceDisposition,
    CloseProcessResourceKind, CloseProcessStepOutcome, CloseProcessStepSuccess,
    CloseRetirementDispatch, CloseSafeRetryEffect, CloseWorktreeFinalTombstone,
    FirstCloseRetirementDispatch, RecordCloseRetirementDispatchRequest,
    RecordCloseRetirementEvidenceRequest, RecordCloseWorktreeCleanupPlanRequest,
    ReplaceCloseInspectionRequest, ReplaceCloseInspectionScopeRequest,
    StoppedCloseOriginalRetirementProof, TerminalizeInitialCloseCleanupFailureRequest,
};
use crate::db::{
    CloseDescriptorAccess, CloseFileWriteAccess, CloseWriterAuthority, CloseWriterDetector,
    CloseWriterGap, CloseWriterObservation, CloseWriterProcess,
    RecordCloseWriterObservationRequest,
};

mod lease_custody;
pub(crate) use lease_custody::CloseResourceLeaseStore;

use lease_custody::LeaseRegistries;

pub(crate) enum GenericCloseRetryError {
    Admission(String),
    Continuation(String),
    Internal(String),
}

struct DispatchedWorktreeAbsence {
    snapshot: CloseRetirementSnapshot,
    replacement_generation: Option<String>,
}

/// Process-local capability retained from inventory sealing through per-resource
/// teardown. Durable inventory stores only the permit's stable instance identity.
pub(crate) struct CloseResourceLease {
    bash: BashRetirementPermit,
    tmux: TmuxRetirementPermit,
    terminal: TerminalRetirementPermit,
    browser: BrowserRetirementPermit,
    resources: Vec<RetiredResourceIdentity>,
    capture_evidence: Option<native_initial::capture_binding::CaptureEvidenceBinding>,
    native_initial_entered: Vec<RetiredResourceKind>,
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CloseRetirementInventoryPreparationError {
    BackendStorageOrUnattributed {
        detail: String,
    },
    CapturedScopeIdentity {
        scope: WorkScopeId,
        reason: RetirementFailureReason,
        detail: String,
    },
}

struct CloseRetirementInventoryScopeInput {
    scope: WorkScopeId,
    captured_worktree: Option<CapturedWorktreeIdentity>,
    acquired_resources: Vec<RetiredResourceIdentity>,
}

/// Read-only evidence for the exact source-owned initial inspection adapter.
pub(crate) struct NativeInitialInspection {
    pub(crate) request: ReplaceCloseInspectionRequest,
    pub(crate) empty_generation: Option<String>,
}

#[derive(Debug)]
pub(crate) enum NativeInitialInspectionFailure {
    Storage(String),
    Scope {
        scope: WorkScopeId,
        reason: RetirementFailureReason,
        detail: String,
    },
}

/// Collect evidence without acquiring leases, changing the registry, writing to `SQLite`,
/// or interpreting an absent worktree as a successful retirement.
pub(crate) async fn collect_native_initial_inspection(
    db: &phoenix_db::Database,
    attempt_id: CloseAttemptId,
) -> Result<NativeInitialInspection, NativeInitialInspectionFailure> {
    use NativeInitialInspectionFailure as Failure;

    let obligation = db
        .get_close_obligation(attempt_id.as_str())
        .await
        .map_err(|error| Failure::Storage(error.to_string()))?;
    if obligation.attempt_id() != &attempt_id
        || obligation.phase() != ClosePhase::AwaitingRetirementInspection
    {
        return Err(Failure::Storage(
            "initial inspection is not awaiting retirement inspection".into(),
        ));
    }
    let empty_generation = (obligation.snapshot().is_some())
        .then(|| format!("server_git_status_v2_retry_{}", uuid::Uuid::new_v4()));
    let scopes = db
        .list_close_attempt_scopes(attempt_id.as_str())
        .await
        .map_err(|error| Failure::Storage(error.to_string()))?;
    let mut requests = Vec::with_capacity(scopes.len());
    for captured in scopes {
        let scope = captured.scope;
        let Some(captured_worktree) = captured.captured_worktree else {
            continue;
        };
        match inspect_captured_close_worktree(&scope, captured_worktree, empty_generation.as_deref())
            .await
        {
            Ok(CapturedCloseWorktreeInspection::Present(inspection)) => requests.push(inspection),
            Ok(CapturedCloseWorktreeInspection::Absent(_)) => {
                return Err(Failure::Scope {
                    scope,
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail: CAPTURED_WORKTREE_ABSENT.into(),
                });
            }
            Err(failure) => {
                return Err(Failure::Scope {
                    scope,
                    reason: failure.reason,
                    detail: failure.detail,
                });
            }
        }
    }
    Ok(NativeInitialInspection {
        request: ReplaceCloseInspectionRequest {
            attempt_id,
            scopes: requests,
        },
        empty_generation,
    })
}

const CAPTURED_WORKTREE_ABSENT: &str = "captured worktree is absent before inspection";

struct CapturedCloseWorktreeInspectionFailure {
    reason: RetirementFailureReason,
    detail: String,
}

// Absence is an observation, not a retirement result or permission to resume.
enum CapturedCloseWorktreeInspection {
    Present(ReplaceCloseInspectionScopeRequest),
    Absent(WorktreeIdentity),
}

async fn inspect_captured_close_worktree(
    scope: &WorkScopeId,
    captured: CapturedWorktreeIdentity,
    inspection_generation: Option<&str>,
) -> Result<CapturedCloseWorktreeInspection, CapturedCloseWorktreeInspectionFailure> {
    let identity_failure = |detail| CapturedCloseWorktreeInspectionFailure {
        reason: RetirementFailureReason::IdentityNotProven,
        detail,
    };
    let repair_required = |detail| CapturedCloseWorktreeInspectionFailure {
        reason: RetirementFailureReason::ManualRepairRequired,
        detail,
    };
    let CapturedWorktreeIdentity::Resolved(identity) = captured else {
        return Err(identity_failure("captured worktree identity is unresolved".into()));
    };
    let path = worktree_path(&identity);
    let exists = |path: &Path| {
        path.try_exists().map_err(|error| {
            repair_required(format!(
                "cannot observe captured worktree at {}: {error}",
                path.display()
            ))
        })
    };
    let actual_path = if exists(&path)? {
        path
    } else {
        let quarantine = worktree_quarantine_path(&identity).map_err(identity_failure)?;
        if !exists(&quarantine)? {
            return Ok(CapturedCloseWorktreeInspection::Absent(identity));
        }
        quarantine
    };
    if observe_worktree_fingerprint(&actual_path).as_deref() != Some(identity.fingerprint().as_str())
    {
        return Err(identity_failure(
            "captured worktree administrative incarnation changed".into(),
        ));
    }
    let (snapshot, losses) = inspect_worktree_at(&identity, actual_path)
        .await
        .map_err(repair_required)?;
    let snapshot = rotate_inspection_generation(snapshot, inspection_generation)
        .map_err(repair_required)?;
    Ok(CapturedCloseWorktreeInspection::Present(
        ReplaceCloseInspectionScopeRequest {
            scope: scope.clone(),
            snapshot,
            losses,
        },
    ))
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

/// Read-only candidate assessment, not admission, dispatch, or completion authority.
#[derive(Debug)]
pub(crate) struct NativeCloseRetryAssessment {
    pub original: StoppedCloseOriginalRetirementProof,
    pub remaining_effects: Vec<CloseSafeRetryEffect>,
    pub safety_evidence: String,
}

struct CloseRetryLiveEvidence<'a> {
    scopes: &'a [CloseAttemptScope],
    expected: &'a [CloseExpectedRetirementResource],
    worktree_snapshots: &'a [(CloseRetirementTarget, CloseRetirementSnapshot)],
    retired: &'a [CloseRetiredResource],
}

#[allow(clippy::too_many_lines)]
fn native_close_retry_remaining_effects(
    run: &CloseRunRef,
    proof: &StoppedCloseOriginalRetirementProof,
    worktree_snapshots: &[(CloseRetirementTarget, CloseRetirementSnapshot)],
) -> Result<Vec<CloseSafeRetryEffect>, String> {
    if proof.run.run != *run || proof.run.status != CloseRunStatus::Stopped {
        return Err("native retry assessment requires the exact stopped run".into());
    }
    if proof.scopes.is_empty() || proof.resources.is_empty() {
        return Err("native retry assessment lacks original captured resources".into());
    }
    for (index, captured) in proof.scopes.iter().enumerate() {
        if captured.attempt_id != run.attempt_id
            || proof.scopes[..index]
                .iter()
                .any(|prior| prior.scope == captured.scope)
        {
            return Err(
                "native retry assessment has duplicate or mismatched captured scopes".into(),
            );
        }
        let scope_targets = proof
            .resources
            .iter()
            .filter(|target| target.scope == captured.scope);
        let work_scope =
            opaque_resource(RetiredResourceKind::WorkScope, captured.scope.to_string());
        if scope_targets
            .clone()
            .filter(|target| target.resource == work_scope)
            .count()
            != 1
        {
            return Err(
                "native retry assessment lacks independent exact WorkScope evidence".into(),
            );
        }
        let worktree_targets = scope_targets
            .filter(|target| target.resource.kind() == RetiredResourceKind::Worktree)
            .collect::<Vec<_>>();
        match &captured.captured_worktree {
            Some(CapturedWorktreeIdentity::Resolved(identity))
                if worktree_targets.len() == 1
                    && worktree_targets[0].resource.identity()
                        == &LossItemIdentity::Worktree(identity.clone()) => {}
            None if worktree_targets.is_empty() => {}
            _ => {
                return Err(
                    "native retry assessment has mismatched captured worktree evidence".into(),
                );
            }
        }
        let confirmed = worktree_snapshots
            .iter()
            .filter(|(target, _)| target.scope == captured.scope);
        if confirmed.count() != usize::from(captured.captured_worktree.is_some()) {
            return Err(
                "native retry assessment lacks a unique original worktree inspection".into(),
            );
        }
    }
    for (target, _) in worktree_snapshots {
        if !proof
            .scopes
            .iter()
            .any(|captured| captured.scope == target.scope)
        {
            return Err("native retry assessment has mismatched worktree snapshot evidence".into());
        }
    }
    for (index, target) in proof.resources.iter().enumerate() {
        if target.attempt_id != run.attempt_id
            || target.snapshot != proof.snapshot
            || !proof
                .scopes
                .iter()
                .any(|captured| captured.scope == target.scope)
            || proof.resources[..index]
                .iter()
                .any(|prior| prior.scope == target.scope && prior.resource == target.resource)
        {
            return Err(
                "native retry assessment has duplicate or mismatched original targets".into(),
            );
        }
        if target.resource.kind() == RetiredResourceKind::WorkScope
            && target.resource
                != opaque_resource(RetiredResourceKind::WorkScope, target.scope.to_string())
        {
            return Err("native retry assessment has a mismatched WorkScope identity".into());
        }
    }
    for outcome in &proof.outcomes {
        if outcome.attempt_id != run.attempt_id
            || outcome.snapshot != proof.snapshot
            || !proof
                .resources
                .iter()
                .any(|target| target.scope == outcome.scope && target.resource == outcome.resource)
        {
            return Err("native retry assessment has mismatched original outcome evidence".into());
        }
    }
    let mut remaining = Vec::new();
    for target in &proof.resources {
        let receipts = proof
            .outcomes
            .iter()
            .filter(|outcome| outcome.scope == target.scope && outcome.resource == target.resource)
            .collect::<Vec<_>>();
        let [receipt] = receipts.as_slice() else {
            return Err(
                "native retry assessment requires a unique original outcome for every resource"
                    .into(),
            );
        };
        match receipt.outcome {
            RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. } => {}
            RetirementOutcome::Residual { .. } => remaining.push(CloseCleanupFailureResource {
                scope: target.scope.clone(),
                resource: target.resource.clone(),
                disposition: CloseCleanupResourceDisposition::Failed,
            }),
        }
    }
    close_safe_retry_effects(&remaining)
}

fn close_safe_retry_effects(
    remaining_resources: &[CloseCleanupFailureResource],
) -> Result<Vec<CloseSafeRetryEffect>, String> {
    let mut plan = remaining_resources
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
    // Proof/child identity order is not dispatch order. Establish dependencies
    // before admission, which rejects work-scope teardown ahead of its resources.
    plan.sort_by_key(close_safe_retry_effect_rank);
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

#[derive(Debug)]
struct CloseProcessStepFailure {
    resources: Vec<RetiredResourceIdentity>,
    reason: String,
}

#[derive(Debug)]
struct CloseObservedProcessRetirement {
    identities: Vec<OpaqueIdentity>,
    outcome: CloseProcessStepOutcome,
}

#[derive(Debug)]
struct CloseObservedTmuxRetirement {
    resources: Vec<RetiredResourceIdentity>,
    outcome: RetirementOutcome,
}

#[async_trait::async_trait]
trait CloseProcessStepExecutor {
    async fn retire_bash(
        &mut self,
    ) -> Result<CloseObservedProcessRetirement, CloseProcessStepFailure>;
    async fn retire_tmux(
        &mut self,
    ) -> Result<CloseObservedTmuxRetirement, (RetirementFailureReason, String)>;
    async fn retire_terminal(
        &mut self,
    ) -> Result<CloseObservedProcessRetirement, CloseProcessStepFailure>;
    async fn retire_browser(
        &mut self,
    ) -> Result<CloseObservedProcessRetirement, CloseProcessStepFailure>;
}

#[derive(Debug, Default)]
struct CloseProcessStepIdentities {
    bash: Vec<String>,
    terminal: Vec<String>,
    browser: Vec<String>,
}

fn verify_observed_process_identities(
    observed: &[OpaqueIdentity],
    expected: &[String],
) -> Result<(), String> {
    let observed_set: BTreeSet<&str> = observed.iter().map(OpaqueIdentity::as_str).collect();
    if observed_set.len() != observed.len() {
        return Err("process retirement observed duplicate identities".to_string());
    }
    let expected_set: BTreeSet<&str> = expected.iter().map(String::as_str).collect();
    if expected_set.len() != expected.len() {
        return Err("process retirement expected duplicate identities".into());
    }
    if observed_set != expected_set {
        return Err(format!(
            "observed process identities {observed_set:?} do not match expected {expected_set:?}"
        ));
    }
    Ok(())
}

fn verify_process_observation_or_fail(
    observed: &CloseObservedProcessRetirement,
    expected: &[String],
    kind: RetiredResourceKind,
) -> Result<(), CloseLeaseFailure> {
    verify_observed_process_identities(&observed.identities, expected).map_err(|detail| {
        let captured = observed
            .identities
            .iter()
            .map(|identity| opaque_resource(kind, identity.as_str().to_string()))
            .collect();
        CloseLeaseFailure::process_epoch(kind, captured, detail)
    })
}

fn new_tmux_receipt_resources(
    observed: &[RetiredResourceIdentity],
    expected: &[RetiredResourceIdentity],
) -> Result<Vec<RetiredResourceIdentity>, String> {
    for (index, resource) in expected.iter().enumerate() {
        if resource.kind() != RetiredResourceKind::TmuxServer
            || expected[..index].contains(resource)
        {
            return Err("expected tmux targets must be distinct tmux resources".into());
        }
    }
    for (index, resource) in observed.iter().enumerate() {
        if observed[..index].contains(resource) {
            return Err("tmux retirement observed duplicate resources".to_string());
        }
    }
    if let Some(missing) = expected
        .iter()
        .find(|resource| !observed.contains(resource))
    {
        return Err(format!(
            "expected tmux resource {missing:?} was not carried by the observed retirement"
        ));
    }
    Ok(observed
        .iter()
        .filter(|resource| expected.contains(resource))
        .cloned()
        .collect())
}

async fn persist_close_process_step(
    db: &crate::db::Database,
    run: &CloseRunRef,
    scope: &WorkScopeId,
    resource_kind: CloseProcessResourceKind,
    observed: &CloseObservedProcessRetirement,
) -> Result<(), CloseLeaseFailure> {
    let observed_at_us = chrono::Utc::now().timestamp_micros();
    for identity in &observed.identities {
        db.record_close_process_step_success(&CloseProcessStepSuccess {
            run: run.clone(),
            scope: scope.clone(),
            resource_kind,
            identity: identity.clone(),
            outcome: observed.outcome,
            observed_at_us,
        })
        .await
        .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run_close_process_steps<E: CloseProcessStepExecutor>(
    db: &crate::db::Database,
    run: &CloseRunRef,
    snapshot: &CloseRetirementSnapshot,
    scope: &WorkScopeId,
    identities: &CloseProcessStepIdentities,
    tmux_resources: &[RetiredResourceIdentity],
    executor: &mut E,
) -> Result<(), CloseLeaseFailure> {
    let prior_process_successes = db
        .list_close_process_step_successes(run)
        .await
        .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;

    if !process_step_successes_cover(
        &prior_process_successes,
        scope,
        CloseProcessResourceKind::BashProcessGroup,
        &identities.bash,
    ) {
        let observed = executor.retire_bash().await.map_err(|failure| {
            CloseLeaseFailure::process_epoch(
                RetiredResourceKind::BashProcessGroup,
                failure.resources,
                failure.reason,
            )
        })?;
        verify_process_observation_or_fail(
            &observed,
            &identities.bash,
            RetiredResourceKind::BashProcessGroup,
        )?;
        persist_close_process_step(
            db,
            run,
            scope,
            CloseProcessResourceKind::BashProcessGroup,
            &observed,
        )
        .await?;
    }

    let prior_retirement_evidence = db
        .list_close_retirement_evidence(run.attempt_id.as_str())
        .await
        .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
    let tmux_already_complete = tmux_resources.iter().all(|resource| {
        prior_retirement_evidence.iter().any(|proof| {
            proof.scope == *scope
                && proof.resource == *resource
                && matches!(
                    proof.outcome,
                    RetirementOutcome::Retired | RetirementOutcome::AbsenceAdopted { .. }
                )
        })
    });
    if !tmux_already_complete {
        let observed = executor
            .retire_tmux()
            .await
            .map_err(|(reason, detail)| CloseLeaseFailure::Tmux { reason, detail })?;
        let receipts =
            new_tmux_receipt_resources(&observed.resources, tmux_resources).map_err(|detail| {
                CloseLeaseFailure::Tmux {
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail,
                }
            })?;
        for resource in receipts {
            db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: run.attempt_id.clone(),
                snapshot: snapshot.clone(),
                scope: scope.clone(),
                resource,
                outcome: observed.outcome.clone(),
                detail: Some("exact registry permit retirement".to_string()),
            })
            .await
            .map_err(|error| CloseLeaseFailure::Persistence(error.to_string()))?;
        }
    }

    if !process_step_successes_cover(
        &prior_process_successes,
        scope,
        CloseProcessResourceKind::PtySession,
        &identities.terminal,
    ) {
        let observed = executor.retire_terminal().await.map_err(|failure| {
            CloseLeaseFailure::process_epoch(
                RetiredResourceKind::PtySession,
                failure.resources,
                failure.reason,
            )
        })?;
        verify_process_observation_or_fail(
            &observed,
            &identities.terminal,
            RetiredResourceKind::PtySession,
        )?;
        persist_close_process_step(
            db,
            run,
            scope,
            CloseProcessResourceKind::PtySession,
            &observed,
        )
        .await?;
    }

    if !process_step_successes_cover(
        &prior_process_successes,
        scope,
        CloseProcessResourceKind::BrowserSession,
        &identities.browser,
    ) {
        let observed = executor.retire_browser().await.map_err(|failure| {
            CloseLeaseFailure::process_epoch(
                RetiredResourceKind::BrowserSession,
                failure.resources,
                failure.reason,
            )
        })?;
        verify_process_observation_or_fail(
            &observed,
            &identities.browser,
            RetiredResourceKind::BrowserSession,
        )?;
        persist_close_process_step(
            db,
            run,
            scope,
            CloseProcessResourceKind::BrowserSession,
            &observed,
        )
        .await?;
    }

    Ok(())
}

struct LiveCloseProcessStepExecutor<'a> {
    manager: &'a RuntimeManager,
    lease: &'a CloseResourceLease,
    browser_custody: phoenix_tools::browser::session::BrowserRetirementCustody,
}

#[async_trait::async_trait]
impl CloseProcessStepExecutor for LiveCloseProcessStepExecutor<'_> {
    async fn retire_bash(
        &mut self,
    ) -> Result<CloseObservedProcessRetirement, CloseProcessStepFailure> {
        let outcome = self
            .manager
            .bash_handles()
            .complete_retirement(&self.lease.bash)
            .await;
        let generation_is_stale = matches!(&outcome, BashRetirementOutcome::StaleGeneration(_));
        let failed_resources = self
            .lease
            .bash
            .exact_process_groups
            .iter()
            .filter(|target| {
                generation_is_stale
                    || outcome.report().kill_failures.iter().any(|(pid, _)| {
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
            .collect::<Vec<_>>();
        let outcome = require_absent(outcome).map_err(|reason| CloseProcessStepFailure {
            resources: failed_resources,
            reason,
        })?;
        Ok(CloseObservedProcessRetirement {
            identities: lease_process_identities(self.lease.bash.exact_process_groups.iter().map(
                phoenix_tools::bash::registry::BashRetirementTarget::stable_resource_identity,
            )),
            outcome,
        })
    }

    async fn retire_tmux(
        &mut self,
    ) -> Result<CloseObservedTmuxRetirement, (RetirementFailureReason, String)> {
        let outcome = self
            .manager
            .tmux_registry()
            .complete_retirement(&self.lease.tmux)
            .await
            .map_err(|error| {
                (
                    RetirementFailureReason::IdentityNotProven,
                    error.to_string(),
                )
            })?;
        let outcome = tmux_retirement_outcome(outcome)?;
        Ok(CloseObservedTmuxRetirement {
            resources: self
                .lease
                .resources
                .iter()
                .filter(|resource| resource.kind() == RetiredResourceKind::TmuxServer)
                .cloned()
                .collect(),
            outcome,
        })
    }

    async fn retire_terminal(
        &mut self,
    ) -> Result<CloseObservedProcessRetirement, CloseProcessStepFailure> {
        let outcome = require_terminal_absent(
            self.manager
                .terminals
                .complete_retirement(&self.lease.terminal)
                .await,
        )
        .map_err(|reason| CloseProcessStepFailure {
            resources: self
                .lease
                .resources
                .iter()
                .filter(|resource| resource.kind() == RetiredResourceKind::PtySession)
                .cloned()
                .collect(),
            reason,
        })?;
        Ok(CloseObservedProcessRetirement {
            identities: lease_process_identities(
                self.lease
                    .terminal
                    .instance
                    .iter()
                    .map(phoenix_terminal::session::TerminalInstanceIdentity::stable_identity),
            ),
            outcome,
        })
    }

    async fn retire_browser(
        &mut self,
    ) -> Result<CloseObservedProcessRetirement, CloseProcessStepFailure> {
        let outcome = require_browser_absent(
            self.manager
                .browser_sessions()
                .complete_retirement_with_custody(&self.lease.browser, self.browser_custody.clone())
                .await,
        )
        .map_err(|reason| CloseProcessStepFailure {
            resources: self
                .lease
                .resources
                .iter()
                .filter(|resource| resource.kind() == RetiredResourceKind::BrowserSession)
                .cloned()
                .collect(),
            reason,
        })?;
        Ok(CloseObservedProcessRetirement {
            identities: lease_process_identities(self.lease.browser.instances.iter().map(
                phoenix_tools::browser::session::BrowserSessionInstanceIdentity::stable_identity,
            )),
            outcome,
        })
    }
}

fn close_safe_retry_effect_rank(effect: &CloseSafeRetryEffect) -> u8 {
    match effect.resource.kind() {
        RetiredResourceKind::TmuxServer => 0,
        RetiredResourceKind::Worktree => 1,
        RetiredResourceKind::WorkScope => 2,
        _ => 3,
    }
}

pub(super) fn close_safe_retry_execution_order(plan: &[CloseSafeRetryEffect]) -> Vec<usize> {
    let mut indices = (0..plan.len()).collect::<Vec<_>>();
    indices.sort_by_key(|&index| close_safe_retry_effect_rank(&plan[index]));
    indices
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseRetirementInventoryPolicy {
    LegacyDurable,
    NativeInitial,
}

fn build_close_retirement_inventory_scope_request(
    attempt_id: CloseAttemptId,
    snapshot: CloseRetirementSnapshot,
    input: CloseRetirementInventoryScopeInput,
    policy: CloseRetirementInventoryPolicy,
) -> Result<CaptureCloseRetirementInventoryRequest, CloseRetirementInventoryPreparationError> {
    let worktree = match input.captured_worktree {
        None => None,
        Some(CapturedWorktreeIdentity::Resolved(identity)) => Some(identity),
        Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
            return Err(
                CloseRetirementInventoryPreparationError::CapturedScopeIdentity {
                    scope: input.scope.clone(),
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail: format!(
                        "scope {} has unresolved captured worktree identity",
                        input.scope
                    ),
                },
            );
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
    for resource in input.acquired_resources {
        let kind = resource.kind();
        if policy == CloseRetirementInventoryPolicy::LegacyDurable {
            if kind != RetiredResourceKind::TmuxServer {
                continue;
            }
            let LossItemIdentity::Opaque(identity) = resource.identity() else {
                return Err(
                    CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed {
                        detail: "registry resource identity was not opaque".to_string(),
                    },
                );
            };
            inventory.tmux_servers.insert(identity.clone());
            continue;
        }

        let target = match kind {
            RetiredResourceKind::BashProcessGroup => &mut inventory.bash_process_groups,
            RetiredResourceKind::TmuxServer => &mut inventory.tmux_servers,
            RetiredResourceKind::PtySession => &mut inventory.pty_sessions,
            RetiredResourceKind::BrowserSession => &mut inventory.browser_sessions,
            RetiredResourceKind::Worktree
            | RetiredResourceKind::WorkScope
            | RetiredResourceKind::EquivalentLiveResource => {
                return Err(
                    CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed {
                        detail: format!("unexpected registry resource kind {kind:?} in native initial inventory"),
                    },
                );
            }
        };
        let LossItemIdentity::Opaque(identity) = resource.identity() else {
            return Err(
                CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed {
                    detail: "registry resource identity was not opaque".to_string(),
                },
            );
        };
        target.insert(identity.clone());
    }
    Ok(CaptureCloseRetirementInventoryRequest {
        attempt_id,
        snapshot,
        scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
            scope: input.scope,
            inventory,
        }],
    })
}

impl RuntimeManager {
    pub(crate) fn native_initial_close_writer_probe(
        &self,
        receipt: &phoenix_db::native_close::NativeCloseReceipt,
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: RetiredResourceIdentity,
    ) -> CloseWriterProbe {
        CloseWriterProbe {
            persistence_uncertain: None,
            #[cfg(test)]
            clean_writer_fixture: self.clean_close_writer_fixture,
            db: self.db().clone(),
            runtime: tokio::runtime::Handle::current(),
            run: CloseRunRef::initial(receipt.admitted.attempt_id.clone()),
            scope,
            snapshot,
            resource,
        }
    }

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
                    continued_in_conv_id: None,
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

    /// Inspects and continues under the same attempt owner used by retirement and cancellation.
    pub(crate) async fn inspect_close_retirement(
        self: &Arc<Self>,
        attempt_id: CloseAttemptId,
    ) -> Result<(), String> {
        self.start_generic_close_worker(attempt_id, |manager, attempt_id| async move {
            manager.run_close_inspection(attempt_id).await
        })
        .await?
        .await
        .map_err(|error| format!("Close inspection worker failed: {error}"))?
    }

    pub(super) async fn run_close_inspection(&self, attempt_id: CloseAttemptId) -> Result<(), String> {
        let current = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        match current.phase() {
            ClosePhase::RetirementRequested => {
                return self.run_close_runtime_retirement(attempt_id).await;
            }
            ClosePhase::AwaitingRetirementInspection => {}
            _ => return Ok(()),
        }
        if let Some(DispatchedWorktreeAbsence {
            snapshot,
            replacement_generation: Some(generation),
        }) = self.persist_close_retirement_inspection(&attempt_id).await?
        {
            self.db()
                .resume_close_retirement_after_dispatched_absence(
                    &attempt_id,
                    &snapshot,
                    &generation,
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        let current = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        if current.phase() == ClosePhase::RetirementRequested {
            self.run_close_runtime_retirement(attempt_id).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn inspect_close_retirement_only(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<CloseRetirementSnapshot, String> {
        self.persist_close_retirement_inspection(&attempt_id).await?;
        self.db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
            .snapshot()
            .cloned()
            .ok_or_else(|| "server inspection did not persist aggregate snapshot".to_string())
    }

    #[allow(clippy::too_many_lines)]
    async fn persist_close_retirement_inspection(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<Option<DispatchedWorktreeAbsence>, String> {
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
                .acquire_close_resource_lease(attempt_id, captured.scope.clone())
                .await
            {
                return self
                    .route_close_attempt_to_repair(
                        attempt_id,
                        &captured.scope,
                        RetirementFailureReason::IdentityNotProven,
                        format!("Close resource admission fencing failed: {error}"),
                    )
                    .await;
            }
        }
        let mut requests = Vec::with_capacity(scopes.len());
        for captured in scopes {
            let scope = captured.scope;
            let Some(captured_worktree) = captured.captured_worktree else {
                continue;
            };
            let identity = match inspect_captured_close_worktree(
                &scope,
                captured_worktree,
                reinspection_generation.as_deref(),
            )
            .await
            {
                Ok(CapturedCloseWorktreeInspection::Present(inspection)) => {
                    requests.push(inspection);
                    continue;
                }
                Ok(CapturedCloseWorktreeInspection::Absent(identity)) => identity,
                Err(failure) => {
                    return self
                        .route_close_attempt_to_repair(
                            attempt_id,
                            &scope,
                            failure.reason,
                            failure.detail,
                        )
                        .await;
                }
            };
            let obligation = self
                .db()
                .get_close_obligation(attempt_id.as_str())
                .await
                .map_err(|error| error.to_string())?;
            if let Some(prior_snapshot) = obligation.snapshot().cloned() {
                let worktree = RetiredResourceIdentity::parse(
                    RetiredResourceKind::Worktree,
                    LossItemIdentity::Worktree(identity),
                )
                .map_err(|error| error.to_string())?;
                if self
                    .db()
                    .close_retirement_resource_was_dispatched(
                        attempt_id,
                        &scope,
                        &prior_snapshot,
                        &worktree,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                {
                    let replacement_generation = if obligation.phase()
                        == ClosePhase::AwaitingRetirementInspection
                    {
                        Some(reinspection_generation.ok_or_else(|| {
                            "dispatched absence retry lacks replacement generation".to_string()
                        })?)
                    } else {
                        None
                    };
                    return Ok(Some(DispatchedWorktreeAbsence {
                        snapshot: prior_snapshot,
                        replacement_generation,
                    }));
                }
            }
            return self
                .route_close_attempt_to_repair(
                    attempt_id,
                    &scope,
                    RetirementFailureReason::IdentityNotProven,
                    CAPTURED_WORKTREE_ABSENT,
                )
                .await;
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
        Ok(None)
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
        let lease_key = (attempt_id.as_str().to_string(), scope.clone());
        if leases.is_reopening(attempt_id.as_str()) {
            return Err("Close lease reopening is incomplete".to_string());
        }
        if let Some(lease) = leases.complete.get(&lease_key) {
            return Ok(lease.resources.clone());
        }
        if leases.has_acquisition(&lease_key) {
            return leases.finish_acquisition(&lease_key).await;
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
        let legacy_conversation_id = self
            .db()
            .get_work_scope_legacy_conversation_id(&scope)
            .await
            .map_err(|error| error.to_string())?;
        leases
            .acquire(
                lease_key,
                self.close_lease_registries(),
                legacy_worktree_path,
                legacy_conversation_id,
            )
            .await
    }

    fn close_lease_registries(&self) -> LeaseRegistries {
        LeaseRegistries {
            bash: self.bash_handles().clone(),
            tmux: self.tmux_registry().clone(),
            terminal: self.terminals.clone(),
            browser: self.browser_sessions().clone(),
        }
    }

    async fn discard_close_resource_leases(&self, attempt_id: &CloseAttemptId) {
        self.close_retirement_leases
            .lock()
            .await
            .discard_completed(attempt_id.as_str());
    }

    async fn cancel_close_resource_leases(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<(), String> {
        self.close_retirement_leases
            .lock()
            .await
            .cancel_attempt(attempt_id.as_str(), self.close_lease_registries())
            .await
    }

    pub(crate) async fn cancel_close_before_retirement(
        &self,
        attempt_id: &CloseAttemptId,
    ) -> Result<(), String> {
        let _execution = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
        let cancellation = self
            .db()
            .authorize_generic_close_cancellation(attempt_id)
            .await
            .map_err(|error| error.to_string())?;
        self.reopen_generic_close_cancellation_leases(&cancellation)
            .await?;
        self.db()
            .cancel_close_before_retirement(cancellation.attempt_id().as_str())
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn reopen_generic_close_cancellation_leases(
        &self,
        cancellation: &crate::db::GenericCloseCancellation,
    ) -> Result<(), String> {
        self.cancel_close_resource_leases(cancellation.attempt_id())
            .await
    }

    /// Builds the exact durable request from captured scopes and their original leases.
    /// This stage does not persist, classify, repair, or release custody.
    pub(crate) async fn prepare_close_retirement_inventory(
        &self,
        attempt_id: CloseAttemptId,
        snapshot: CloseRetirementSnapshot,
    ) -> Result<CaptureCloseRetirementInventoryRequest, CloseRetirementInventoryPreparationError>
    {
        self.prepare_close_retirement_inventory_with_policy(
            attempt_id,
            snapshot,
            CloseRetirementInventoryPolicy::LegacyDurable,
        )
        .await
    }

    pub(crate) async fn prepare_native_close_initial_retirement_inventory(
        &self,
        attempt_id: CloseAttemptId,
        snapshot: CloseRetirementSnapshot,
    ) -> Result<CaptureCloseRetirementInventoryRequest, CloseRetirementInventoryPreparationError>
    {
        self.prepare_close_retirement_inventory_with_policy(
            attempt_id,
            snapshot,
            CloseRetirementInventoryPolicy::NativeInitial,
        )
        .await
    }

    async fn prepare_close_retirement_inventory_with_policy(
        &self,
        attempt_id: CloseAttemptId,
        snapshot: CloseRetirementSnapshot,
        policy: CloseRetirementInventoryPolicy,
    ) -> Result<CaptureCloseRetirementInventoryRequest, CloseRetirementInventoryPreparationError>
    {
        let scopes = self
            .db()
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .map_err(|error| {
                CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed {
                    detail: error.to_string(),
                }
            })?;
        let mut requests = Vec::with_capacity(scopes.len());
        let mut native_scopes = Vec::new();
        for captured in scopes {
            if policy == CloseRetirementInventoryPolicy::NativeInitial {
                native_scopes.push(captured.clone());
            }
            let resources = self
                .acquire_close_resource_lease(&attempt_id, captured.scope.clone())
                .await
                .map_err(|detail| {
                    CloseRetirementInventoryPreparationError::CapturedScopeIdentity {
                        scope: captured.scope.clone(),
                        reason: RetirementFailureReason::IdentityNotProven,
                        detail,
                    }
                })?;
            let request = build_close_retirement_inventory_scope_request(
                attempt_id.clone(),
                snapshot.clone(),
                CloseRetirementInventoryScopeInput {
                    scope: captured.scope,
                    captured_worktree: captured.captured_worktree,
                    acquired_resources: resources,
                },
                policy,
            )?;
            requests.extend(request.scopes);
        }
        let request = CaptureCloseRetirementInventoryRequest {
            attempt_id,
            snapshot,
            scopes: requests,
        };
        if policy == CloseRetirementInventoryPolicy::NativeInitial {
            let evidence = native_initial::capture_binding::read_capture_evidence(
                self.db(),
                &request,
                &native_scopes,
            )
            .await
            .map_err(|detail| {
                CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed { detail }
            })?;
            self.close_retirement_leases
                .lock()
                .await
                .bind_capture_evidence(request.attempt_id.as_str(), &evidence)
                .map_err(|detail| {
                    CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed {
                        detail,
                    }
                })?;
        }
        Ok(request)
    }

    /// Acquires all scope fences, then seals the exact server-owned inventory.
    pub(crate) async fn capture_close_retirement_inventory(
        &self,
        attempt_id: CloseAttemptId,
        snapshot: CloseRetirementSnapshot,
    ) -> Result<(), String> {
        let request = match self
            .prepare_close_retirement_inventory(attempt_id.clone(), snapshot)
            .await
        {
            Ok(request) => request,
            Err(CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed {
                detail,
            }) => return Err(detail),
            Err(CloseRetirementInventoryPreparationError::CapturedScopeIdentity {
                scope,
                reason,
                detail,
            }) => {
                return self
                    .route_close_attempt_to_repair(&attempt_id, &scope, reason, detail)
                    .await;
            }
        };
        match self.db().capture_close_retirement_inventory(request).await {
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

    /// Captures the first-pass inventory if incomplete, then lists exactly the
    /// unresolved retirement targets. Performs no resource effects.
    async fn prepare_close_retirement_targets(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
    ) -> Result<Vec<CloseExpectedRetirementResource>, String> {
        if !self
            .db()
            .close_retirement_inventory_is_complete(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?
        {
            self.capture_close_retirement_inventory(attempt_id.clone(), snapshot.clone())
                .await?;
        }
        self.db()
            .list_close_expected_retirement_resources(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())
    }

    /// Retires exactly the unresolved inventory targets, independently of the waiter.
    pub(crate) async fn retire_close_runtime_resources(
        self: &Arc<Self>,
        attempt_id: CloseAttemptId,
    ) -> Result<(), String> {
        self.start_generic_close_worker(attempt_id, |manager, attempt_id| async move {
            manager.run_close_runtime_retirement(attempt_id).await
        })
            .await?
            .await
            .map_err(|error| format!("Close retirement worker failed: {error}"))?
    }

    pub(crate) async fn retry_generic_close_retirement(
        self: &Arc<Self>,
        attempt_id: CloseAttemptId,
    ) -> Result<(), GenericCloseRetryError> {
        self.start_generic_close_worker(attempt_id, |manager, attempt_id| async move {
            manager.run_generic_close_retry(attempt_id).await
        })
        .await
        .map_err(GenericCloseRetryError::Admission)?
        .await
        .map_err(|error| GenericCloseRetryError::Internal(format!("Close Retry worker failed: {error}")))?
    }

    async fn run_generic_close_retry(
        &self,
        attempt_id: CloseAttemptId,
    ) -> Result<(), GenericCloseRetryError> {
        self.db()
            .retry_close_retirement(&attempt_id)
            .await
            .map_err(|error| GenericCloseRetryError::Admission(error.to_string()))?;
        let Err(error) = self.run_close_inspection(attempt_id.clone()).await else {
            return Ok(());
        };
        let current = self.db().get_close_obligation(attempt_id.as_str()).await
            .map_err(|error| GenericCloseRetryError::Internal(error.to_string()))?;
        if current.phase() == ClosePhase::RetirementRequested {
            let scope = self.db().list_close_attempt_scopes(attempt_id.as_str()).await
                .map_err(|error| GenericCloseRetryError::Internal(error.to_string()))?
                .into_iter().next()
                .ok_or_else(|| GenericCloseRetryError::Internal("Close retry has no captured scope".to_string()))?
                .scope;
            let _ = self.route_close_attempt_to_repair::<()>(
                &attempt_id,
                &scope,
                RetirementFailureReason::ManualRepairRequired,
                error.clone(),
            ).await;
        }
        Err(GenericCloseRetryError::Continuation(error))
    }

    #[cfg(test)]
    pub(super) async fn start_paused_generic_close_retirement(
        self: &Arc<Self>,
        attempt_id: CloseAttemptId,
        resume: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<tokio::task::JoinHandle<Result<(), String>>, String> {
        self.start_generic_close_worker(attempt_id, |manager, attempt_id| async move {
            resume.await.map_err(|error| error.to_string())?;
            manager.run_close_runtime_retirement(attempt_id).await
        })
        .await
    }

    pub(crate) async fn start_generic_close_settlement(
        self: &Arc<Self>,
        settlement: crate::db::GenericCloseSettlement,
    ) -> Result<tokio::task::JoinHandle<Result<bool, String>>, String> {
        let attempt_id = settlement.obligation().attempt_id().clone();
        self.start_generic_close_worker(attempt_id, |manager, _| async move {
            manager.settle_close_obligation(settlement).await
        })
        .await
    }

    async fn start_generic_close_worker<F, Fut, T>(
        self: &Arc<Self>,
        attempt_id: CloseAttemptId,
        operation: F,
    ) -> Result<tokio::task::JoinHandle<T>, String>
    where
        F: FnOnce(Arc<Self>, CloseAttemptId) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send,
        T: Send + 'static,
    {
        let execution = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
        let owner = self
            .acquire_local_authority_pass()
            .map_err(|()| "local authority closed before Close retirement".to_string())?;
        let manager = Arc::clone(self);
        Ok(tokio::spawn(async move {
            let _execution = execution;
            let _owner = owner;
            operation(manager, attempt_id).await
        }))
    }

    #[allow(clippy::too_many_lines)]
    async fn run_close_runtime_retirement(&self, attempt_id: CloseAttemptId) -> Result<(), String> {
        let run = CloseRunRef::initial(attempt_id.clone());
        let obligation = self
            .db()
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| error.to_string())?;
        if obligation.phase() != ClosePhase::RetirementRequested {
            return Ok(());
        }
        self.db()
            .require_no_retained_legacy_creation_for_retirement(&attempt_id)
            .await
            .map_err(|error| error.to_string())?;
        let snapshot = obligation
            .snapshot()
            .cloned()
            .ok_or_else(|| "retirement requested without an inspection snapshot".to_string())?;
        let targets = self
            .prepare_close_retirement_targets(&attempt_id, &snapshot)
            .await?;
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
                .complete
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
                .complete
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
                                .complete_retirement(permit.as_ref())
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

    /// Assesses an exact stopped native operation without admitting or executing a successor.
    pub(crate) async fn assess_native_close_retry(
        &self,
        failed_run: &CloseRunRef,
    ) -> Result<NativeCloseRetryAssessment, RetryAssessmentError> {
        self.assess_native_close_retry_with_writer_probe(
            failed_run,
            |original, quarantine, tombstone, clean_writer_fixture| {
                #[cfg(not(test))]
                let _ = clean_writer_fixture;
                probe_retry_worktree_writers(
                    original,
                    quarantine,
                    tombstone,
                    #[cfg(test)]
                    clean_writer_fixture,
                )
            },
        )
        .await
    }

    async fn assess_native_close_retry_with_writer_probe<F>(
        &self,
        failed_run: &CloseRunRef,
        mut writer_probe: F,
    ) -> Result<NativeCloseRetryAssessment, RetryAssessmentError>
    where
        F: FnMut(
            &Path,
            &Path,
            Option<&CloseWorktreeFinalTombstone>,
            bool,
        ) -> Result<(), RetryAssessmentError>,
    {
        let context = self
            .db()
            .read_native_close_retry_pre_admission_context(failed_run)
            .await
            .map_err(|error| error.to_string())?;
        let original = context.original;
        let worktree_snapshots = context.worktree_snapshots;
        let remaining_effects =
            native_close_retry_remaining_effects(failed_run, &original, &worktree_snapshots)?;
        let safety_evidence = self
            .validate_close_safe_retry_with_evidence(
                &failed_run.attempt_id,
                &original.snapshot,
                &remaining_effects,
                CloseRetryLiveEvidence {
                    scopes: &original.scopes,
                    expected: &original.resources,
                    worktree_snapshots: &worktree_snapshots,
                    retired: &original.outcomes,
                },
                &mut writer_probe,
            )
            .await?;
        Ok(NativeCloseRetryAssessment {
            original,
            remaining_effects,
            safety_evidence,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn validate_close_safe_retry_with_evidence(
        &self,
        attempt_id: &CloseAttemptId,
        snapshot: &CloseRetirementSnapshot,
        plan: &[CloseSafeRetryEffect],
        evidence: CloseRetryLiveEvidence<'_>,
        writer_probe: &mut impl FnMut(
            &Path,
            &Path,
            Option<&CloseWorktreeFinalTombstone>,
            bool,
        ) -> Result<(), RetryAssessmentError>,
    ) -> Result<String, RetryAssessmentError> {
        let CloseRetryLiveEvidence {
            scopes,
            expected,
            worktree_snapshots,
            retired,
        } = evidence;
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
        for captured in scopes {
            let scope = &captured.scope;
            if self
                .db()
                .work_scope_has_unresolved_product_ownership(scope)
                .await
                .map_err(|error| error.to_string())?
            {
                return Err(format!("scope {scope} has unresolved ownership").into());
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
                    )
                    .into());
                }
            }
            match &captured.captured_worktree {
                Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                    return Err(format!("scope {scope} has unresolved worktree identity").into());
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
                    writer_probe(
                        &path,
                        &quarantine,
                        tombstone.as_ref(),
                        #[cfg(test)]
                        self.clean_close_writer_fixture,
                        #[cfg(not(test))]
                        false,
                    )?;
                    if let Some(tombstone) = &tombstone {
                        let object = tombstone.root.join("object");
                        if object.try_exists().map_err(|error| error.to_string())? {
                            let confirmed = worktree_snapshots
                                .iter()
                                .find(|(target, _)| target.scope == *scope)
                                .ok_or_else(|| {
                                    format!("scope {scope} lacks a confirmed worktree inspection")
                                })?;
                            verify_retained_worktree(identity, object, &confirmed.1).await?;
                        }
                    }
                    let active = path.try_exists().map_err(|error| error.to_string())?;
                    let quarantined = quarantine.try_exists().map_err(|error| error.to_string())?;
                    if active && quarantined {
                        return Err(
                            format!("scope {scope} has ambiguous worktree locations").into()
                        );
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
                            )
                            .into());
                        }
                        if planned
                            .iter()
                            .any(|effect| effect.resource.kind() == RetiredResourceKind::Worktree)
                        {
                            let confirmed = worktree_snapshots
                                .iter()
                                .find(|(target, _)| target.scope == *scope)
                                .ok_or_else(|| {
                                    format!("scope {scope} lacks a confirmed worktree inspection")
                                })?;
                            ensure_no_ignored_content(&path)?;
                            let (fresh, losses) = inspect_worktree_at(identity, path).await?;
                            if fresh.fingerprint() != confirmed.1.fingerprint()
                                || !losses.is_empty()
                            {
                                return Err(format!(
                                    "scope {scope} worktree is not unchanged and reconstructible"
                                )
                                .into());
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
                        });
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
                            return Err(
                                format!(
                                    "scope {scope} absent worktree has no exact prior proof or cleanup authority"
                                )
                                .into(),
                            );
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
                            )
                            .into());
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
                                    )
                                    .into());
                                }
                                observations.push(format!(
                                    "{scope}: dispatched exact tmux {} absent",
                                    value.as_str()
                                ));
                            }
                            _ => {
                                return Err(format!(
                                    "scope {scope} tmux identity differs or is ambiguous"
                                )
                                .into());
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
                            )
                            .into());
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
                self.persist_close_retirement_inspection(attempt_id).await?;
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
                            let writer_probe = CloseWriterProbe {
                                persistence_uncertain: None,
                                #[cfg(test)]
                                clean_writer_fixture: self.clean_close_writer_fixture,
                                db: self.db().clone(),
                                runtime: tokio::runtime::Handle::current(),
                                run: run.clone(),
                                scope: scope.clone(),
                                snapshot: snapshot.clone(),
                                resource: target.resource.clone(),
                            };
                            let recovery = tokio::task::spawn_blocking(move || {
                                match resume_final_worktree_tombstone(
                                    cleanup_plan
                                        .final_tombstone
                                        .as_ref()
                                        .expect("filtered above"),
                                    &identity,
                                    &confirmed_snapshot,
                                    writer_probe,
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
                            let writer_probe = CloseWriterProbe {
                                persistence_uncertain: None,
                                #[cfg(test)]
                                clean_writer_fixture: self.clean_close_writer_fixture,
                                db: self.db().clone(),
                                runtime: tokio::runtime::Handle::current(),
                                run: run.clone(),
                                scope: scope.clone(),
                                snapshot: snapshot.clone(),
                                resource: target.resource.clone(),
                            };
                            let recovery = tokio::task::spawn_blocking(move || {
                                if let Some(tombstone) = &cleanup_plan.final_tombstone {
                                    match resume_final_worktree_tombstone(
                                        tombstone,
                                        &identity,
                                        &confirmed_snapshot,
                                        writer_probe,
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
                            let preparation_identity = identity.clone();
                            let prepared = tokio::task::spawn_blocking(move || {
                                prepare_generic_worktree_removal(&preparation_identity)
                            })
                            .await
                            .map_err(|error| format!("worktree preparation task failed: {error}"))
                            .and_then(std::convert::identity);
                            let (prepared, administrative_dir, incarnation) = match prepared {
                                Ok(prepared) => prepared,
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
                            let is_original = prepared.inspection_path == worktree_path(identity);
                            if is_original {
                                let (fresh, _) = match inspect_worktree_at_until(
                                    identity,
                                    prepared.inspection_path.clone(),
                                    prepared.deadline,
                                )
                                .await
                                {
                                    Ok(inspection) => inspection,
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
                                if fresh.fingerprint() != confirmed.snapshot.fingerprint() {
                                    drop(prepared);
                                    self.db()
                                        .return_close_attempt_to_reinspection(attempt_id)
                                        .await
                                        .map_err(|error| error.to_string())?;
                                    self.persist_close_retirement_inspection(attempt_id).await?;
                                    return Err("worktree changed before dispatch; fresh confirmation is required".to_string());
                                }
                            }
                            let cleanup_plan = match existing_cleanup_plan {
                                Some(plan)
                                    if plan.administrative_dir == administrative_dir
                                        && plan.administrative_dir_incarnation == incarnation =>
                                {
                                    plan
                                }
                                Some(_) => {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            "prepared worktree no longer matches the durable cleanup plan",
                                        )
                                        .await;
                                }
                                None if is_original => crate::db::CloseWorktreeCleanupPlan {
                                    administrative_dir,
                                    administrative_dir_incarnation: incarnation,
                                    final_tombstone: None,
                                },
                                None => {
                                    return self
                                        .record_close_cleanup_failure(
                                            run,
                                            snapshot,
                                            &scope,
                                            target.resource.clone(),
                                            RetirementFailureReason::IdentityNotProven,
                                            "quarantined worktree lacks an exact durable cleanup plan",
                                        )
                                        .await;
                                }
                            };
                            let removal_entry = if is_original {
                                match self
                                    .db()
                                    .claim_generic_original_worktree_dispatch(
                                        RecordCloseWorktreeCleanupPlanRequest {
                                            attempt_id: attempt_id.clone(),
                                            scope: scope.clone(),
                                            snapshot: snapshot.clone(),
                                            resource: target.resource.clone(),
                                            administrative_dir: cleanup_plan.administrative_dir.clone(),
                                            administrative_dir_incarnation: cleanup_plan
                                                .administrative_dir_incarnation
                                                .clone(),
                                        },
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?
                                {
                                    CloseRetirementDispatch::First(first) => {
                                        WorktreeRemovalEntry::GenericOriginal(first)
                                    }
                                    CloseRetirementDispatch::Replay => {
                                        return self
                                            .record_close_cleanup_failure(
                                                run,
                                                snapshot,
                                                &scope,
                                                target.resource.clone(),
                                                RetirementFailureReason::ManualRepairRequired,
                                                "original worktree dispatch already exists without a completed removal; refusing to replay the effect",
                                            )
                                            .await;
                                    }
                                }
                            } else {
                                WorktreeRemovalEntry::RetainedQuarantine
                            };
                            let confirmed_snapshot = confirmed.snapshot.clone();
                            let db = self.db().clone();
                            let attempt = attempt_id.clone();
                            let scope_for_tombstone = scope.clone();
                            let resource_for_tombstone = target.resource.clone();
                            let snapshot_for_tombstone = snapshot.clone();
                            let runtime = tokio::runtime::Handle::current();
                            let persistence_runtime = runtime.clone();
                            let writer_probe = CloseWriterProbe {
                                persistence_uncertain: None,
                                #[cfg(test)]
                                clean_writer_fixture: self.clean_close_writer_fixture,
                                db: self.db().clone(),
                                runtime: runtime.clone(),
                                run: run.clone(),
                                scope: scope.clone(),
                                snapshot: snapshot.clone(),
                                resource: target.resource.clone(),
                            };
                            let final_removal = tokio::task::spawn_blocking(move || {
                                inspect_and_remove_prepared_worktree_with_hook_and_plan_until(
                                    &runtime,
                                    prepared,
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
                                    |_| {},
                                    CloseWorktreeEffects::production(writer_probe, removal_entry)?,
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
        for ((lease_attempt, scope), lease) in &leases.complete {
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

    async fn route_close_attempt_to_repair<T>(
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

    /// Completes one live lease.
    async fn complete_close_resource_lease(
        &self,
        run: &CloseRunRef,
        snapshot: &CloseRetirementSnapshot,
        scope: &WorkScopeId,
        expected: &[RetiredResourceIdentity],
    ) -> Result<(), CloseLeaseFailure> {
        let key = (run.attempt_id.as_str().to_string(), scope.clone());
        // Keep both the store guard and the original entry in place across every
        // await. The lease owns the sole process-epoch permits; removing it here
        // would let cancellation drop that custody before any reinsertion.
        let leases = self.close_retirement_leases.lock().await;
        #[cfg(test)]
        let completion_hook = leases.completion_hook.clone();
        let Some(lease) = leases.complete.get(&key) else {
            return Err(CloseLeaseFailure::Unavailable);
        };
        #[cfg(test)]
        if let Some(hook) = completion_hook {
            // This must stay after lease lookup, before the first awaited step,
            // so cancellation exercises custody rather than only lock admission.
            hook.entered.notify_one();
            hook.release.notified().await;
        }
        async {
            if expected.iter().any(|resource| {
                resource.kind() != RetiredResourceKind::TmuxServer
                    || !lease.resources.contains(resource)
            }) {
                return Err(CloseLeaseFailure::Tmux {
                    reason: RetirementFailureReason::IdentityNotProven,
                    detail: "live Close lease differs from sealed unresolved inventory".to_string(),
                });
            }
            let identities = CloseProcessStepIdentities {
                bash: lease
                    .bash
                    .exact_process_groups
                    .iter()
                    .map(phoenix_tools::bash::registry::BashRetirementTarget::stable_resource_identity)
                    .collect(),
                terminal: lease
                    .terminal
                    .instance
                    .iter()
                    .map(phoenix_terminal::session::TerminalInstanceIdentity::stable_identity)
                    .collect(),
                browser: lease
                    .browser
                    .instances
                    .iter()
                    .map(phoenix_tools::browser::session::BrowserSessionInstanceIdentity::stable_identity)
                    .collect(),
            };
            let mut executor = LiveCloseProcessStepExecutor {
                manager: self,
                lease,
                // Legacy execution does not transfer native source/target custody.
                browser_custody: phoenix_tools::browser::session::BrowserRetirementCustody::new(
                    std::sync::Arc::new(()),
                ),
            };
            run_close_process_steps(
                self.db(),
                run,
                snapshot,
                scope,
                &identities,
                expected,
                &mut executor,
            )
            .await
        }
        .await
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
    // One compatibility budget covers the complete top-level worktree inspection.
    inspect_worktree_at_until(
        identity,
        path,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .await
}

async fn inspect_worktree_at_until(
    identity: &WorktreeIdentity,
    path: PathBuf,
    deadline: std::time::Instant,
) -> Result<(CloseRetirementSnapshot, Vec<CloseLossItem>), String> {
    if std::time::Instant::now() >= deadline {
        return Err("worktree inspection exceeded its deadline".to_string());
    }
    let identity = identity.clone();
    tokio::task::spawn_blocking(move || {
        inspect_worktree_at_with_policy_until(
            &identity,
            &path,
            deadline,
            WorktreeInspectionPolicy::Legacy,
            &mut run_bounded_spawned_git,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorktreeInspectionPolicy {
    Legacy,
    ReadOnlyClean,
}

/// Synchronous inspection for a future native worker. Call outside `SQLite` writer
/// admission: only the supplied Git leaf may enter spawn admission. Filesystem
/// checks are cooperative with the same absolute deadline, not interruptible IO.
/// Until native scratch-index mutation has its own authority, refuse hidden
/// suppression, every gitlink (including uninitialized ones), detached HEAD and
/// dirty status. Successful results use the full legacy observation builder, not
/// a status-only clean proof or a separate snapshot generation.
/// Ignored content retains legacy snapshot semantics; deletion still needs the
/// separate ignored-content guard and its own exact mutation authority.
fn inspect_worktree_at_with_git_leaf_until(
    identity: &WorktreeIdentity,
    path: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(CloseRetirementSnapshot, Vec<CloseLossItem>), String> {
    inspect_worktree_at_with_policy_until(
        identity,
        path,
        deadline,
        WorktreeInspectionPolicy::ReadOnlyClean,
        git_leaf,
    )
}

#[derive(Debug)]
enum InspectedHead {
    Resolved {
        symbolic: Option<Vec<u8>>,
        oid: Vec<u8>,
    },
    Unborn {
        symbolic: Vec<u8>,
    },
}

fn inspect_head_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    operation: &str,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<InspectedHead, String> {
    let symbolic = run_bounded_git_command_with_leaf_until(
        repository,
        &["symbolic-ref", "-q", "HEAD"],
        None,
        deadline,
        operation,
        git_leaf,
    )?;
    let symbolic_ref = if symbolic.status.success() {
        Some(symbolic.stdout.trim_ascii().to_vec())
    } else if symbolic.status.code() == Some(1) {
        None
    } else {
        return Err(format!(
            "cannot determine whether HEAD is detached: {}",
            String::from_utf8_lossy(&symbolic.stderr).trim()
        ));
    };
    let head = run_bounded_git_command_with_leaf_until(
        repository,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        None,
        deadline,
        operation,
        git_leaf,
    )?;
    if head.status.success() {
        let oid = head.stdout.trim_ascii().to_vec();
        GitOidIdentity::parse_hex(String::from_utf8_lossy(&oid).trim())
            .map_err(|error| format!("malformed inspected HEAD object ID: {error}"))?;
        return Ok(InspectedHead::Resolved {
            symbolic: symbolic_ref,
            oid,
        });
    }
    if head.status.code() != Some(1) || !head.stdout.is_empty() || !head.stderr.is_empty() {
        return Err("cannot prove HEAD is unresolved".to_string());
    }
    let Some(symbolic_ref) = symbolic_ref else {
        return Err("detached HEAD is unresolved".to_string());
    };
    let reference = std::str::from_utf8(&symbolic_ref)
        .map_err(|error| format!("malformed symbolic HEAD reference: {error}"))?;
    if !reference.starts_with("refs/heads/") || reference.trim() != reference {
        return Err("symbolic HEAD does not name a branch ref".to_string());
    }
    let valid_ref = run_bounded_git_command_with_leaf_until(
        repository,
        &["check-ref-format", reference],
        None,
        deadline,
        operation,
        git_leaf,
    )?;
    if !valid_ref.status.success() {
        return Err("symbolic HEAD branch ref is malformed".to_string());
    }
    verify_unborn_head_with_leaf_until(
        repository,
        reference,
        &symbolic_ref,
        deadline,
        operation,
        git_leaf,
    )?;
    Ok(InspectedHead::Unborn {
        symbolic: symbolic_ref,
    })
}

fn verify_unborn_head_with_leaf_until(
    repository: &Path,
    reference: &str,
    symbolic_ref: &[u8],
    deadline: std::time::Instant,
    operation: &str,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(), String> {
    let mut show_ref_leaf =
        |mut command: std::process::Command, leaf_deadline, leaf_operation: &str| {
            command.env("LC_ALL", "C");
            git_leaf(command, leaf_deadline, leaf_operation)
        };
    let exact_ref = run_bounded_git_command_with_leaf_until(
        repository,
        &["show-ref", "--exists", reference],
        None,
        deadline,
        operation,
        &mut show_ref_leaf,
    )?;
    if exact_ref.status.code() != Some(2)
        || !exact_ref.stdout.is_empty()
        || exact_ref.stderr != b"error: reference does not exist\n"
    {
        return Err("cannot prove symbolic HEAD branch is absent".to_string());
    }
    let symbolic_again = run_bounded_git_command_with_leaf_until(
        repository,
        &["symbolic-ref", "-q", "HEAD"],
        None,
        deadline,
        operation,
        git_leaf,
    )?;
    if !symbolic_again.status.success()
        || symbolic_again.stdout.trim_ascii() != symbolic_ref
        || !symbolic_again.stderr.is_empty()
    {
        return Err("symbolic HEAD changed during unborn inspection".to_string());
    }
    let head_again = run_bounded_git_command_with_leaf_until(
        repository,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        None,
        deadline,
        operation,
        git_leaf,
    )?;
    if head_again.status.code() != Some(1)
        || !head_again.stdout.is_empty()
        || !head_again.stderr.is_empty()
    {
        return Err("HEAD changed during unborn inspection".to_string());
    }
    Ok(())
}

fn inspect_worktree_at_with_policy_until(
    identity: &WorktreeIdentity,
    path: &Path,
    deadline: std::time::Instant,
    policy: WorktreeInspectionPolicy,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(CloseRetirementSnapshot, Vec<CloseLossItem>), String> {
    if std::time::Instant::now() >= deadline {
        return Err("worktree inspection exceeded its deadline".to_string());
    }
    if observe_worktree_fingerprint(path).as_deref() != Some(identity.fingerprint().as_str()) {
        return Err("captured worktree administrative incarnation changed".to_string());
    }
    let output = run_bounded_git_status_with_leaf_until(path, deadline, git_leaf)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut observation = canonical_status_observation(&output.stdout);
    let ignored_paths = observe_ignored_paths_with_leaf_until(path, deadline, git_leaf)?;
    append_ignored_paths(&mut observation, &ignored_paths);
    if policy == WorktreeInspectionPolicy::ReadOnlyClean && !observation.is_empty() {
        return Err("native worktree inspection does not yet support dirty status".to_string());
    }
    let mut losses = parse_status_losses(&output.stdout);
    losses.extend(
        ignored_paths
            .iter()
            .cloned()
            .map(CloseLossItem::IgnoredPath),
    );
    losses.sort_by_key(|loss| (loss.category().as_str(), loss.identity().value()));
    losses.dedup();
    let hidden_dirty = match policy {
        WorktreeInspectionPolicy::Legacy => {
            observe_hidden_worktree_changes_with_leaf_until(path, deadline, git_leaf)?
        }
        WorktreeInspectionPolicy::ReadOnlyClean => {
            // Unlike the legacy hidden-change observer, this probe does not copy
            // or mutate an index, even temporarily. Refuse suppression itself,
            // rather than treating a clean porcelain result as sufficient proof.
            let flags = run_bounded_git_command_with_leaf_until(
                path,
                &["ls-files", "-v", "-z", "--"],
                None,
                deadline,
                "hidden-index inspection",
                git_leaf,
            )?;
            if !flags.status.success() {
                return Err(format!(
                    "cannot inspect hidden index flags: {}",
                    String::from_utf8_lossy(&flags.stderr).trim()
                ));
            }
            // `-v` lowercases any assume-unchanged tag, not only `H`.
            if flags.stdout.split(|byte| *byte == 0).any(|row| {
                row.first()
                    .is_some_and(|flag| *flag == b'S' || flag.is_ascii_lowercase())
            }) {
                return Err(
                    "native worktree inspection does not yet support hidden index suppression"
                        .to_string(),
                );
            }
            Vec::new()
        }
    };
    for path in hidden_dirty {
        observation.extend_from_slice(b"HIDDEN_UNSTAGED\\0");
        observation.extend_from_slice(path.as_bytes());
        observation.push(0);
        losses.push(CloseLossItem::UnstagedTrackedPath(path));
    }
    losses.sort_by_key(|loss| (loss.category().as_str(), loss.identity().value()));
    losses.dedup();
    let content_observation =
        observe_dirty_content_with_leaf_until(path, &losses, deadline, git_leaf)?;
    observation.extend_from_slice(&content_observation);
    observe_detached_head_and_submodules_with_leaf_until(
        path,
        &mut observation,
        &mut losses,
        deadline,
        policy,
        git_leaf,
    )?;
    let snapshot = snapshot_for(&observation);
    if std::time::Instant::now() >= deadline {
        return Err("worktree inspection exceeded its deadline".to_string());
    }
    Ok((snapshot, losses))
}

fn observe_detached_head_and_submodules_with_leaf_until(
    path: &Path,
    observation: &mut Vec<u8>,
    losses: &mut Vec<CloseLossItem>,
    deadline: std::time::Instant,
    policy: WorktreeInspectionPolicy,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(), String> {
    if std::time::Instant::now() >= deadline {
        return Err("detached head inspection exceeded its deadline".to_string());
    }
    let observed = {
        if std::time::Instant::now() >= deadline {
            return Err("detached head inspection exceeded its deadline".to_string());
        }
        let mut visited = std::collections::HashSet::new();
        visited.insert(path.canonicalize().map_err(|error| error.to_string())?);
        let head =
            inspect_head_with_leaf_until(path, deadline, "detached head inspection", git_leaf)?;
        let mut result = Vec::new();
        let (symbolic, head_oid) = match head {
            InspectedHead::Resolved { symbolic, oid } => (symbolic, Some(oid)),
            InspectedHead::Unborn { symbolic } => (Some(symbolic), None),
        };
        if let Some(symbolic) = symbolic.as_ref() {
            result.push((b"HEAD_SYMBOLIC".to_vec(), symbolic.clone()));
        }
        if let Some(head_oid) = head_oid.as_ref() {
            result.push((b"HEAD".to_vec(), head_oid.clone()));
        }
        let evidence = detached_reachability_evidence_with_leaf_until(
            path,
            head_oid.as_deref(),
            deadline,
            git_leaf,
        )?;
        if symbolic.is_none() {
            result.push((b"DETACHED".to_vec(), evidence.clone()));
        }
        result.push((b"DETACHED_HISTORY".to_vec(), evidence));
        match policy {
            WorktreeInspectionPolicy::Legacy => {
                observe_initialized_submodules_with_leaf_until(
                    path,
                    &[],
                    &mut result,
                    deadline,
                    &mut visited,
                    git_leaf,
                )?;
            }
            WorktreeInspectionPolicy::ReadOnlyClean => {
                let gitlinks = observe_gitlink_index_with_leaf_until(
                    path,
                    &[],
                    &mut result,
                    deadline,
                    git_leaf,
                )?;
                if !gitlinks.is_empty() {
                    return Err(
                        "native worktree inspection does not yet support gitlinks".to_string()
                    );
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("detached head inspection exceeded its deadline".to_string());
        }
        result
    };
    append_detached_and_submodule_observation(observed, observation, losses)?;
    if std::time::Instant::now() >= deadline {
        return Err("detached head inspection exceeded its deadline".to_string());
    }
    Ok(())
}

fn append_detached_and_submodule_observation(
    observed: WorktreeObservation,
    observation: &mut Vec<u8>,
    losses: &mut Vec<CloseLossItem>,
) -> Result<(), String> {
    let head_oid = observed
        .iter()
        .find_map(|(tag, value)| (tag == b"HEAD").then_some(value.clone()));
    for (tag, value) in observed {
        observation.extend_from_slice(&tag);
        observation.push(0);
        observation.extend_from_slice(&value);
        observation.push(0);
        if tag == b"DETACHED" && head_oid.is_none() {
            return Err("detached worktree has no resolved HEAD".to_string());
        }
        if tag == b"DETACHED_HISTORY" {
            for oid in value
                .split(|byte| *byte == b'\n')
                .filter(|oid| !oid.is_empty())
            {
                observation.extend_from_slice(b"DETACHED_UNREACHABLE\0");
                observation.extend_from_slice(oid);
                observation.push(0);
                losses.push(CloseLossItem::DetachedUnreachableCommit(
                    GitOidIdentity::parse_hex(String::from_utf8_lossy(oid).trim())
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
                b"ignored" => CloseLossItem::IgnoredPath(path),
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

type WorktreeObservation = Vec<(Vec<u8>, Vec<u8>)>;

// Retained for direct compatibility assertions in the unit tests only.
#[cfg(test)]
fn detached_unreachable_commits(
    repository: &Path,
    head_oid: &[u8],
) -> Result<Vec<Vec<u8>>, String> {
    detached_unreachable_commits_until(
        repository,
        head_oid,
        std::time::Instant::now() + std::time::Duration::from_secs(5),
    )
}

#[cfg(test)]
fn detached_unreachable_commits_until(
    repository: &Path,
    head_oid: &[u8],
    deadline: std::time::Instant,
) -> Result<Vec<Vec<u8>>, String> {
    detached_unreachable_commits_with_leaf_until(
        repository,
        Some(head_oid),
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn detached_unreachable_commits_with_leaf_until(
    repository: &Path,
    head_oid: Option<&[u8]>,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<Vec<u8>>, String> {
    if std::time::Instant::now() >= deadline {
        return Err("detached commit reachability exceeded its deadline".to_string());
    }
    let mut roots = Vec::new();
    if let Some(head_oid) = head_oid {
        let head_text = std::str::from_utf8(head_oid).map_err(|error| error.to_string())?;
        GitOidIdentity::parse_hex(head_text.trim())
            .map_err(|error| format!("malformed inspected HEAD object ID: {error}"))?;
        roots.push(head_text.to_string());
    }
    // Only the target worktree's HEAD reflog contributes candidate history.
    // Other worktree HEADs, custom refs, and reflogs are not durable roots.
    roots.extend(target_head_reflog_roots_with_leaf_until(
        repository, deadline, git_leaf,
    )?);
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    if std::time::Instant::now() >= deadline {
        return Err("detached commit reachability exceeded its deadline".to_string());
    }

    // A failed exact lookup means the legitimate refs/stash may be absent. Other
    // failures are inspection uncertainty, not proof that no stash exists.
    let stash = run_bounded_git_command_with_leaf_until(
        repository,
        &["show-ref", "--verify", "--quiet", "refs/stash"],
        None,
        deadline,
        "detached commit reachability",
        git_leaf,
    )?;
    let stash_exists = if stash.status.success() {
        true
    } else if stash.status.code() == Some(1) && stash.stdout.is_empty() && stash.stderr.is_empty() {
        false
    } else {
        return Err(String::from_utf8_lossy(&stash.stderr).trim().to_string());
    };
    let mut arguments = vec!["rev-list"];
    arguments.extend(roots.iter().map(String::as_str));
    arguments.extend(["--not", "--branches", "--remotes", "--tags"]);
    if stash_exists {
        arguments.push("refs/stash");
    }
    let output = run_bounded_git_command_with_leaf_until(
        repository,
        &arguments,
        None,
        deadline,
        "detached commit reachability",
        git_leaf,
    )?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut unreachable = Vec::new();
    for oid in output.stdout.split(|byte| *byte == b'\n') {
        if oid.is_empty() {
            continue;
        }
        let text = std::str::from_utf8(oid).map_err(|error| error.to_string())?;
        GitOidIdentity::parse_hex(text)
            .map_err(|error| format!("malformed detached commit object ID: {error}"))?;
        unreachable.push(oid.to_vec());
    }
    Ok(unreachable)
}

fn ensure_files_ref_storage_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(), String> {
    let backend = run_bounded_git_command_with_leaf_until(
        repository,
        &["config", "--get", "extensions.refStorage"],
        None,
        deadline,
        "Git ref backend inspection",
        git_leaf,
    )?;
    if backend.status.success() {
        if backend.stdout.trim_ascii() == b"files" {
            return Ok(());
        }
        return Err(format!(
            "unsupported Git ref storage backend: {}",
            String::from_utf8_lossy(backend.stdout.trim_ascii()).trim()
        ));
    }
    if backend.status.code() == Some(1) {
        return Ok(());
    }
    Err(format!(
        "cannot inspect Git ref storage backend: {}",
        String::from_utf8_lossy(&backend.stderr).trim()
    ))
}

fn target_head_reflog_roots_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<String>, String> {
    ensure_files_ref_storage_with_leaf_until(repository, deadline, git_leaf)?;
    let path = run_bounded_git_command_with_leaf_until(
        repository,
        &["rev-parse", "--git-path", "logs/HEAD"],
        None,
        deadline,
        "detached commit reachability",
        git_leaf,
    )?;
    if !path.status.success() {
        return Err(String::from_utf8_lossy(&path.stderr).trim().to_string());
    }
    let reflog_path = path_buf_from_git_bytes(path.stdout.trim_ascii());
    let reflog_path = if reflog_path.is_absolute() {
        reflog_path
    } else {
        repository.join(reflog_path)
    };
    if std::time::Instant::now() >= deadline {
        return Err("detached commit reachability exceeded its deadline".to_string());
    }
    let reflog = match read_bounded_regular_file_nofollow(&reflog_path, 16 * 1024 * 1024) {
        Ok(reflog) => reflog,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot safely read target HEAD reflog: {error}")),
    };
    if std::time::Instant::now() >= deadline {
        return Err("detached commit reachability exceeded its deadline".to_string());
    }
    parse_target_head_reflog_roots(&reflog, deadline)
}

fn read_bounded_regular_file_nofollow(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    #[cfg(not(unix))]
    {
        let _ = (path, limit);
        return Err(std::io::Error::other(
            "safe no-follow file opening is unavailable on this platform",
        ));
    }
    #[cfg(unix)]
    {
        use std::{
            ffi::CString,
            io::Read as _,
            os::{fd::FromRawFd as _, unix::ffi::OsStrExt as _},
        };

        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("target HEAD reflog has no parent directory"))?;
        let basename = path
            .file_name()
            .ok_or_else(|| std::io::Error::other("target HEAD reflog has no basename"))?;
        let parent = CString::new(parent.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::other("target HEAD reflog parent contains NUL"))?;
        let basename = CString::new(basename.as_bytes())
            .map_err(|_| std::io::Error::other("target HEAD reflog basename contains NUL"))?;
        let parent_fd = unsafe {
            libc::open(
                parent.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
            )
        };
        if parent_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let parent_file = unsafe { std::fs::File::from_raw_fd(parent_fd) };
        let file_fd = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&parent_file),
                basename.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if file_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let file = unsafe { std::fs::File::from_raw_fd(file_fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > limit {
            return Err(std::io::Error::other(
                "target HEAD reflog is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(std::io::Error::other(
                "target HEAD reflog exceeded its capture limit",
            ));
        }
        Ok(bytes)
    }
}

fn parse_target_head_reflog_roots(
    reflog: &[u8],
    deadline: std::time::Instant,
) -> Result<Vec<String>, String> {
    const MAX_ROOTS: usize = 1024;
    let mut roots = BTreeSet::new();
    for record in reflog
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if std::time::Instant::now() >= deadline {
            return Err("detached commit reachability exceeded its deadline".to_string());
        }
        let fields = record.splitn(3, |byte| *byte == b' ').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err("malformed target HEAD reflog record".to_string());
        }
        for oid in &fields[..2] {
            let oid = std::str::from_utf8(oid).map_err(|error| error.to_string())?;
            GitOidIdentity::parse_hex(oid)
                .map_err(|error| format!("malformed target HEAD reflog object ID: {error}"))?;
            if !oid.bytes().all(|byte| byte == b'0') && !roots.contains(oid) {
                if roots.len() == MAX_ROOTS {
                    return Err(
                        "target HEAD reflog exceeds the 1024 distinct-root limit".to_string()
                    );
                }
                roots.insert(oid.to_string());
            }
        }
    }
    Ok(roots.into_iter().collect())
}

#[cfg(test)]
fn detached_reachability_evidence_until(
    repository: &Path,
    head_oid: &[u8],
    deadline: std::time::Instant,
) -> Result<Vec<u8>, String> {
    detached_reachability_evidence_with_leaf_until(
        repository,
        Some(head_oid),
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn detached_reachability_evidence_with_leaf_until(
    repository: &Path,
    head_oid: Option<&[u8]>,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<u8>, String> {
    if std::time::Instant::now() >= deadline {
        return Err("detached reachability evidence exceeded its deadline".to_string());
    }
    let unreachable =
        detached_unreachable_commits_with_leaf_until(repository, head_oid, deadline, git_leaf)?;
    let mut evidence = Vec::new();
    for oid in unreachable {
        evidence.extend_from_slice(&oid);
        evidence.push(b'\n');
    }
    Ok(evidence)
}

#[allow(clippy::too_many_lines)]
fn git_directory_for_worktree_until(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<PathBuf, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let dot_git = repository.join(".git");
    check_directory_deletion_deadline(Some(deadline))?;
    let metadata = std::fs::symlink_metadata(&dot_git);
    check_directory_deletion_deadline(Some(deadline))?;
    let metadata = metadata.map_err(|error| {
        format!(
            "cannot inspect Git metadata at {}: {error}",
            dot_git.display()
        )
    })?;
    if metadata.is_dir() {
        check_directory_deletion_deadline(Some(deadline))?;
        let resolved = dot_git.canonicalize();
        check_directory_deletion_deadline(Some(deadline))?;
        return resolved.map_err(|error| format!("cannot resolve Git directory: {error}"));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Git metadata at {} is not a file or directory",
            dot_git.display()
        ));
    }
    check_directory_deletion_deadline(Some(deadline))?;
    let pointer = std::fs::read(&dot_git);
    check_directory_deletion_deadline(Some(deadline))?;
    let pointer = pointer
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
        pointer.clone()
    } else {
        repository.join(&pointer)
    };
    check_directory_deletion_deadline(Some(deadline))?;
    let resolved = candidate.canonicalize();
    check_directory_deletion_deadline(Some(deadline))?;
    if let Ok(candidate) = resolved {
        return Ok(candidate);
    }

    let mut parent_repository = None;
    for ancestor in repository.ancestors().skip(1) {
        check_directory_deletion_deadline(Some(deadline))?;
        let exists = ancestor.join(".git").exists();
        check_directory_deletion_deadline(Some(deadline))?;
        if exists {
            parent_repository = Some(ancestor);
            break;
        }
    }
    check_directory_deletion_deadline(Some(deadline))?;
    let parent_repository = parent_repository.ok_or_else(|| {
        format!(
            "cannot resolve moved Git directory for {}",
            repository.display()
        )
    })?;
    let parent_git_dir = git_directory_for_worktree_until(parent_repository, deadline)?;
    let components = pointer.components().collect::<Vec<_>>();
    let mut candidates = std::collections::BTreeSet::new();
    for (index, component) in components.iter().enumerate() {
        check_directory_deletion_deadline(Some(deadline))?;
        if component.as_os_str() != std::ffi::OsStr::new("modules") {
            continue;
        }
        let mut candidate = parent_git_dir.join("modules");
        for component in &components[index + 1..] {
            check_directory_deletion_deadline(Some(deadline))?;
            if let std::path::Component::Normal(component) = component {
                candidate.push(component);
            } else {
                candidate.clear();
                break;
            }
        }
        if candidate.as_os_str().is_empty() {
            continue;
        }
        check_directory_deletion_deadline(Some(deadline))?;
        let resolved = candidate.canonicalize();
        check_directory_deletion_deadline(Some(deadline))?;
        if let Ok(candidate) = resolved {
            candidates.insert(candidate);
        }
    }
    check_directory_deletion_deadline(Some(deadline))?;
    if candidates.len() != 1 {
        return Err(format!(
            "cannot resolve the exact moved Git directory for {} from its pre-move gitdir binding",
            repository.display()
        ));
    }
    Ok(candidates
        .pop_first()
        .expect("exactly one candidate exists"))
}

fn capture_git_directory_bindings_until(
    root: &Path,
    deadline: std::time::Instant,
) -> Result<std::collections::BTreeMap<PathBuf, PathBuf>, String> {
    capture_git_directory_bindings_with_leaf_until(root, deadline, &mut run_bounded_spawned_git)
}

fn capture_git_directory_bindings_with_leaf_until(
    root: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::collections::BTreeMap<PathBuf, PathBuf>, String> {
    fn capture(
        repository: &Path,
        relative: &Path,
        deadline: std::time::Instant,
        bindings: &mut std::collections::BTreeMap<PathBuf, PathBuf>,
        git_leaf: &mut WorktreeGitLeaf<'_>,
    ) -> Result<(), String> {
        check_directory_deletion_deadline(Some(deadline))?;
        bindings.insert(
            relative.to_path_buf(),
            git_directory_for_worktree_until(repository, deadline)?,
        );
        let (_, gitlinks) = index_gitlinks_with_leaf_until(repository, deadline, git_leaf)?;
        for gitlink in gitlinks {
            check_directory_deletion_deadline(Some(deadline))?;
            let path = path_buf_from_git_bytes(gitlink.path.as_bytes());
            let submodule = repository.join(&path);
            if !submodule.join(".git").exists() {
                continue;
            }
            capture(
                &submodule,
                &relative.join(path),
                deadline,
                bindings,
                git_leaf,
            )?;
        }
        Ok(())
    }

    check_directory_deletion_deadline(Some(deadline))?;
    let mut bindings = std::collections::BTreeMap::new();
    capture(root, Path::new(""), deadline, &mut bindings, git_leaf)?;
    check_directory_deletion_deadline(Some(deadline))?;
    Ok(bindings)
}

fn verify_git_directory_bindings_until(
    root: &Path,
    expected: &std::collections::BTreeMap<PathBuf, PathBuf>,
    deadline: std::time::Instant,
) -> Result<(), String> {
    check_directory_deletion_deadline(Some(deadline))?;
    for (relative, expected_git_dir) in expected {
        check_directory_deletion_deadline(Some(deadline))?;
        let observed = git_directory_for_worktree_until(&root.join(relative), deadline)?;
        check_directory_deletion_deadline(Some(deadline))?;
        if &observed != expected_git_dir {
            return Err(format!(
                "Git administrative binding changed for {}",
                relative.display()
            ));
        }
    }
    Ok(())
}

fn bind_git_command_to_worktree_until(
    command: &mut std::process::Command,
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<(), String> {
    command
        .env(
            "GIT_DIR",
            git_directory_for_worktree_until(repository, deadline)?,
        )
        .env("GIT_WORK_TREE", repository)
        .current_dir(repository);
    Ok(())
}

/// Executes one prepared Git command, not an enclosing inspection. A native
/// callback owns spawn admission, child supervision and output collection under
/// its original effect context. It may return only after confirmed writer release
/// and completed child/pipe custody. Unknown release or unresolved completion must
/// retain the calling stack without returning or unwinding (including scratch
/// index owners held by enclosing helpers).
/// Helpers call this outside any `SQLite` writer callback and never refresh the
/// supplied absolute deadline. Legacy callers use `run_bounded_spawned_git`.
type WorktreeGitLeaf<'a> = dyn FnMut(std::process::Command, std::time::Instant, &str) -> Result<std::process::Output, String>
    + 'a;

#[cfg(test)]
fn run_bounded_git_status_until(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<std::process::Output, String> {
    run_bounded_git_status_with_leaf_until(repository, deadline, &mut run_bounded_spawned_git)
}

fn run_bounded_git_status_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::process::Output, String> {
    if std::time::Instant::now() >= deadline {
        return Err("Git status inspection exceeded its deadline".to_string());
    }
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree_until(&mut command, repository, deadline)?;
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
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
    execute_worktree_git_leaf_until(command, deadline, "Git status inspection", git_leaf)
}

// This read-only registry query starts from captured Git metadata, not a
// worktree. Never route it through the worktree-only .git pointer resolver.
fn list_registered_worktrees_in_common_directory_until(
    common_git_dir: &Path,
    deadline: std::time::Instant,
) -> Result<std::process::Output, String> {
    list_registered_worktrees_in_common_directory_with_leaf_until(
        common_git_dir,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn list_registered_worktrees_in_common_directory_with_leaf_until(
    common_git_dir: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::process::Output, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let mut command = phoenix_core::git::command();
    command
        .current_dir(common_git_dir)
        .env("GIT_DIR", common_git_dir)
        .env_remove("GIT_WORK_TREE")
        .args(["worktree", "list", "--porcelain", "-z"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    execute_worktree_git_leaf_until(
        command,
        deadline,
        "exact Worktree registration inspection",
        git_leaf,
    )
}

fn run_bounded_git_command_with_leaf_until(
    repository: &Path,
    arguments: &[&str],
    index: Option<&Path>,
    deadline: std::time::Instant,
    operation: &str,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::process::Output, String> {
    if std::time::Instant::now() >= deadline {
        return Err(format!("{operation} exceeded its deadline"));
    }
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree_until(&mut command, repository, deadline)?;
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
    execute_worktree_git_leaf_until(command, deadline, operation, git_leaf)
}

#[cfg(test)]
fn run_bounded_git_paths_until(
    repository: &Path,
    arguments: &[&str],
    paths: &[Vec<u8>],
    literal_pathspec: bool,
    index: &Path,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    run_bounded_git_paths_with_leaf_until(
        repository,
        arguments,
        paths,
        literal_pathspec,
        index,
        deadline,
        operation,
        &mut run_bounded_spawned_git,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_bounded_git_paths_with_leaf_until(
    repository: &Path,
    arguments: &[&str],
    paths: &[Vec<u8>],
    literal_pathspec: bool,
    index: &Path,
    deadline: std::time::Instant,
    operation: &str,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::process::Output, String> {
    // `Command::args` accepts path OsStrings, preserving bytes on Unix.
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree_until(&mut command, repository, deadline)?;
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
    execute_worktree_git_leaf_until(command, deadline, operation, git_leaf)
}

fn execute_worktree_git_leaf_until(
    command: std::process::Command,
    deadline: std::time::Instant,
    operation: &str,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::process::Output, String> {
    if std::time::Instant::now() >= deadline {
        return Err(format!("{operation} exceeded its deadline"));
    }
    git_leaf(command, deadline, operation)
}

fn run_bounded_spawned_git(
    mut command: std::process::Command,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    if std::time::Instant::now() >= deadline {
        return Err(format!("{operation} exceeded its deadline"));
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    wait_bounded_child_output(child, deadline, operation)
}

fn wait_bounded_child_output(
    mut child: std::process::Child,
    deadline: std::time::Instant,
    operation: &str,
) -> Result<std::process::Output, String> {
    let Some(stdout) = child.stdout.take() else {
        terminate_bounded_child(&mut child);
        return Err("Git stdout was not piped".into());
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_bounded_child(&mut child);
        return Err("Git stderr was not piped".into());
    };
    let stdout_reader = std::thread::spawn(move || read_git_pipe(stdout, None));
    let stderr_reader = std::thread::spawn(move || read_git_pipe(stderr, None));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status))
                if stdout_reader.is_finished()
                    && stderr_reader.is_finished()
                    && std::time::Instant::now() < deadline =>
            {
                break status;
            }
            Ok(_) => {}
            Err(error) => {
                terminate_bounded_child(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error.to_string());
            }
        }
        if std::time::Instant::now() >= deadline {
            terminate_bounded_child(&mut child);
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

fn read_git_pipe(
    pipe: impl std::io::Read,
    limit: Option<usize>,
) -> Result<Vec<u8>, std::io::Error> {
    use std::io::Read as _;
    let mut reader = std::io::BufReader::new(pipe);
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    let mut overflow = false;
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        let remaining = limit.unwrap_or(usize::MAX).saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..n.min(remaining)]);
        overflow |= n > remaining;
    }
    if overflow {
        return Err(std::io::Error::other(
            "Git output exceeded its capture limit",
        ));
    }
    Ok(bytes)
}

fn terminate_bounded_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        if let Ok(process_group) = i32::try_from(child.id()) {
            libc::kill(-process_group, libc::SIGKILL);
        }
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

struct IndexedGitlink {
    path: GitPathIdentity,
    stage_zero_oid: Option<Vec<u8>>,
    has_unmerged_stage: bool,
}

type IndexedGitlinks = (Vec<u8>, Vec<IndexedGitlink>);

#[cfg(test)]
fn index_gitlinks(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<IndexedGitlinks, String> {
    index_gitlinks_with_leaf_until(repository, deadline, &mut run_bounded_spawned_git)
}

fn index_gitlinks_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<IndexedGitlinks, String> {
    let output = run_bounded_git_command_with_leaf_until(
        repository,
        &["ls-files", "--stage", "-z", "--"],
        None,
        deadline,
        "gitlink inspection",
        git_leaf,
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

fn observe_gitlink_index_with_leaf_until(
    repository: &Path,
    relative_prefix: &[u8],
    observation: &mut WorktreeObservation,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<IndexedGitlink>, String> {
    let (index_observation, gitlinks) =
        index_gitlinks_with_leaf_until(repository, deadline, git_leaf)?;
    observation.push((
        [b"SUBMODULE_GITLINK_INDEX\0".as_slice(), relative_prefix].concat(),
        index_observation,
    ));
    Ok(gitlinks)
}

#[allow(clippy::too_many_lines)]
fn observe_initialized_submodules_with_leaf_until(
    repository: &Path,
    relative_prefix: &[u8],
    observation: &mut WorktreeObservation,
    deadline: std::time::Instant,
    visited: &mut std::collections::HashSet<PathBuf>,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(), String> {
    if std::time::Instant::now() >= deadline {
        return Err("Git submodule inspection exceeded its aggregate deadline".to_string());
    }
    let gitlinks = observe_gitlink_index_with_leaf_until(
        repository,
        relative_prefix,
        observation,
        deadline,
        git_leaf,
    )?;
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

        let status = run_bounded_git_status_with_leaf_until(&submodule_path, deadline, git_leaf)?;
        if !status.status.success() {
            return Err(format!(
                "cannot inspect initialized submodule {}: {}",
                String::from_utf8_lossy(&relative_path),
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
        let ignored_paths =
            observe_ignored_paths_with_leaf_until(&submodule_path, deadline, git_leaf)?;
        let mut submodule_losses = parse_status_losses(&status.stdout);
        submodule_losses.extend(
            ignored_paths
                .iter()
                .cloned()
                .map(CloseLossItem::IgnoredPath),
        );
        for hidden in
            observe_hidden_worktree_changes_with_leaf_until(&submodule_path, deadline, git_leaf)?
        {
            let loss = CloseLossItem::UnstagedTrackedPath(hidden);
            if !submodule_losses.contains(&loss) {
                submodule_losses.push(loss);
            }
        }
        let mut submodule_status = canonical_status_observation(&status.stdout);
        append_ignored_paths(&mut submodule_status, &ignored_paths);
        submodule_status.extend_from_slice(&observe_dirty_content_with_leaf_until(
            &submodule_path,
            &submodule_losses,
            deadline,
            git_leaf,
        )?);
        observation.push((
            [b"SUBMODULE_STATUS\0".as_slice(), relative_path.as_slice()].concat(),
            submodule_status,
        ));

        let submodule_head = inspect_head_with_leaf_until(
            &submodule_path,
            deadline,
            "Git submodule HEAD inspection",
            git_leaf,
        )
        .map_err(|detail| {
            format!(
                "cannot inspect initialized submodule gitlink {}: {detail}",
                String::from_utf8_lossy(&relative_path)
            )
        })?;
        let (symbolic_head, submodule_head_oid) = match submodule_head {
            InspectedHead::Resolved { symbolic, oid } => (symbolic, Some(oid)),
            InspectedHead::Unborn { symbolic } => (Some(symbolic), None),
        };
        if let Some(symbolic_head) = symbolic_head.as_ref() {
            observation.push((
                [
                    b"SUBMODULE_HEAD_SYMBOLIC\0".as_slice(),
                    relative_path.as_slice(),
                ]
                .concat(),
                symbolic_head.clone(),
            ));
        }
        if let Some(submodule_head_oid) = submodule_head_oid.as_ref() {
            observation.push((
                [b"SUBMODULE_GITLINK\0".as_slice(), relative_path.as_slice()].concat(),
                [
                    gitlink.stage_zero_oid.as_deref().unwrap_or_default(),
                    b"\0",
                    submodule_head_oid,
                ]
                .concat(),
            ));
        }
        let submodule_history = detached_reachability_evidence_with_leaf_until(
            &submodule_path,
            submodule_head_oid.as_deref(),
            deadline,
            git_leaf,
        )?;
        observation.push((
            [
                b"SUBMODULE_DETACHED_HISTORY\0".as_slice(),
                relative_path.as_slice(),
            ]
            .concat(),
            submodule_history.clone(),
        ));
        let gitlink_changed = gitlink.stage_zero_oid.as_deref() != submodule_head_oid.as_deref();
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
        for oid in submodule_history
            .split(|byte| *byte == b'\n')
            .filter(|oid| !oid.is_empty())
        {
            observation.push((
                [b"SUBMODULE_LOSS\0detached\0".as_slice(), oid].concat(),
                Vec::new(),
            ));
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
                CloseLossItem::IgnoredPath(path) => (b"ignored".as_slice(), path),
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
        for nested_path in &ignored_paths {
            let full_path = join_git_paths(&relative_path, nested_path.as_bytes())?;
            observation.push((
                [
                    b"SUBMODULE_LOSS\0ignored\0".as_slice(),
                    full_path.as_slice(),
                ]
                .concat(),
                Vec::new(),
            ));
        }
        observe_initialized_submodules_with_leaf_until(
            &submodule_path,
            &relative_path,
            observation,
            deadline,
            visited,
            git_leaf,
        )?;
    }
    if std::time::Instant::now() >= deadline {
        return Err("Git submodule inspection exceeded its aggregate deadline".to_string());
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

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn inspect_and_remove_exact_worktree<B>(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    final_tombstone: Option<&CloseWorktreeFinalTombstone>,
    effects: CloseWorktreeEffects,
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
        effects,
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
        tests::injected_close_worktree_effects(|_| Ok(FsmonitorStop::NotRunning)),
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
    effects: CloseWorktreeEffects,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    inspect_and_remove_exact_worktree_with_hook_and_plan_until(
        runtime,
        identity,
        confirmed_snapshot,
        administrative_dir,
        administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
        after_quarantine,
        effects,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
}

// This owns a physical lock, not native Close destructive authority. Preparation
// may probe Git/create its lock file but performs no teardown. The caller retains
// custody until the consuming graph returns, including non-preemptible filesystem
// operations and completion of its blocking worker.
struct PreparedPhysicalWorktreeRemoval {
    identity: WorktreeIdentity,
    inspection_path: PathBuf,
    common_git_dir: PathBuf,
    common_git_dir_incarnation: String,
    deadline: std::time::Instant,
    repository_lock: RepositoryMutationLock,
}

fn prepare_physical_worktree_removal_until(
    identity: &WorktreeIdentity,
    deadline: std::time::Instant,
) -> Result<Option<PreparedPhysicalWorktreeRemoval>, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let path = worktree_path(identity);
    let quarantine = worktree_quarantine_path(identity)?;
    if !path
        .try_exists()
        .map_err(|error| format!("cannot observe captured worktree path: {error}"))?
        && !quarantine
            .try_exists()
            .map_err(|error| format!("cannot observe quarantined worktree path: {error}"))?
    {
        return Ok(None);
    }
    let inspection_path = if path.exists() { path } else { quarantine };
    let common_git_dir = exact_worktree_common_git_dir_until(&inspection_path, deadline)?;
    let common_git_dir_incarnation = observe_administrative_dir_incarnation(&common_git_dir)?;
    check_directory_deletion_deadline(Some(deadline))?;
    let repository_lock =
        RepositoryMutationLock::acquire_common_directory_until(&common_git_dir, deadline)
            .map_err(|(message, _)| message)?;
    check_directory_deletion_deadline(Some(deadline))?;
    if observe_administrative_dir_incarnation(&common_git_dir)? != common_git_dir_incarnation {
        return Err("worktree repository incarnation changed while acquiring its lock".to_string());
    }
    check_directory_deletion_deadline(Some(deadline))?;
    Ok(Some(PreparedPhysicalWorktreeRemoval {
        identity: identity.clone(),
        inspection_path,
        common_git_dir,
        common_git_dir_incarnation,
        deadline,
        repository_lock,
    }))
}

fn prepare_generic_worktree_removal(
    identity: &WorktreeIdentity,
) -> Result<(PreparedPhysicalWorktreeRemoval, PathBuf, String), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let prepared = prepare_physical_worktree_removal_until(identity, deadline)?
        .ok_or_else(|| "captured worktree disappeared before dispatch".to_string())?;
    let original = worktree_path(identity);
    let quarantine = worktree_quarantine_path(identity)?;
    let original_exists = original.try_exists().map_err(|error| error.to_string())?;
    let quarantine_exists = quarantine.try_exists().map_err(|error| error.to_string())?;
    let exact_path = match (original_exists, quarantine_exists) {
        (true, false) => original,
        (false, true) => quarantine,
        _ => return Err("worktree custody changed while preparing its removal".to_string()),
    };
    if prepared.inspection_path != exact_path
        || observe_worktree_fingerprint(&exact_path).as_deref()
            != Some(identity.fingerprint().as_str())
    {
        return Err("prepared worktree no longer matches captured custody".to_string());
    }
    check_directory_deletion_deadline(Some(deadline))?;
    let administrative_dir = exact_worktree_administrative_dir(&exact_path, &prepared.common_git_dir)?;
    validate_cleanup_plan_registration_incarnation(identity, &administrative_dir)?;
    let incarnation = observe_administrative_dir_incarnation(&administrative_dir)?;
    Ok((prepared, administrative_dir, incarnation))
}

#[allow(clippy::too_many_arguments)]
fn inspect_and_remove_exact_worktree_with_hook_and_plan_until<F, B>(
    runtime: &tokio::runtime::Handle,
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    final_tombstone: Option<&CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    effects: CloseWorktreeEffects,
    deadline: std::time::Instant,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    let Some(prepared) = prepare_physical_worktree_removal_until(identity, deadline)? else {
        return Ok(ExactWorktreeRemoval::Residual {
            detail: format!(
                "captured worktree and quarantine are absent; refusing to delete unverified Git registration {}",
                administrative_dir.display()
            ),
        });
    };
    inspect_and_remove_prepared_worktree_with_hook_and_plan_until(
        runtime,
        prepared,
        confirmed_snapshot,
        administrative_dir,
        administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
        after_quarantine,
        effects,
    )
}

#[allow(clippy::too_many_arguments)]
fn inspect_and_remove_prepared_worktree_with_hook_and_plan_until<F, B>(
    runtime: &tokio::runtime::Handle,
    prepared: PreparedPhysicalWorktreeRemoval,
    confirmed_snapshot: &CloseRetirementSnapshot,
    administrative_dir: &Path,
    administrative_dir_incarnation: &str,
    final_tombstone: Option<&CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    effects: CloseWorktreeEffects,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    let PreparedPhysicalWorktreeRemoval {
        identity,
        inspection_path,
        common_git_dir,
        common_git_dir_incarnation,
        deadline,
        repository_lock,
    } = prepared;
    check_directory_deletion_deadline(Some(deadline))?;
    if !inspection_path
        .try_exists()
        .map_err(|error| format!("cannot observe prepared worktree path: {error}"))?
    {
        return Err("prepared worktree path disappeared before removal".to_string());
    }
    if exact_worktree_common_git_dir_until(&inspection_path, deadline)? != common_git_dir
        || observe_administrative_dir_incarnation(&common_git_dir)? != common_git_dir_incarnation
    {
        return Err("prepared worktree repository binding changed before removal".to_string());
    }
    check_directory_deletion_deadline(Some(deadline))?;
    let (fresh_snapshot, _) = runtime.block_on(inspect_worktree_at_until(
        &identity,
        inspection_path.clone(),
        deadline,
    ))?;
    if fresh_snapshot.fingerprint() != confirmed_snapshot.fingerprint() {
        return Ok(ExactWorktreeRemoval::ReinspectionRequired {
            detail: "worktree changed after Close inspection confirmation; fresh confirmation is required"
                .to_string(),
        });
    }
    ensure_no_ignored_content_until(&inspection_path, deadline)?;
    runtime.block_on(quarantine_and_remove_exact_worktree(
        &identity,
        confirmed_snapshot,
        administrative_dir.to_path_buf(),
        administrative_dir_incarnation.to_string(),
        final_tombstone.cloned(),
        bind_tombstone,
        after_quarantine,
        effects,
        deadline,
        repository_lock,
    ))
}

fn path_is_within(candidate: &Path, directory: &Path) -> bool {
    candidate == directory || candidate.starts_with(directory)
}

fn exact_worktree_common_git_dir(worktree: &Path) -> Result<PathBuf, String> {
    exact_worktree_common_git_dir_until(
        worktree,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
    )
}

fn exact_worktree_common_git_dir_until(
    worktree: &Path,
    deadline: std::time::Instant,
) -> Result<PathBuf, String> {
    exact_worktree_common_git_dir_with_leaf_until(worktree, deadline, &mut run_bounded_spawned_git)
}

fn exact_worktree_common_git_dir_with_leaf_until(
    worktree: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<PathBuf, String> {
    let output = run_bounded_git_command_with_leaf_until(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        None,
        deadline,
        "Git common-directory inspection",
        git_leaf,
    )?;
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
    destination_directory: libc::c_int,
    destination: *const libc::c_char,
) -> libc::c_int {
    // SAFETY: caller supplies valid descriptors and NUL-terminated names.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory,
            source,
            destination_directory,
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
    destination_directory: libc::c_int,
    destination: *const libc::c_char,
) -> libc::c_int {
    // SAFETY: caller supplies valid descriptors and NUL-terminated names.
    unsafe {
        libc::renameatx_np(
            directory,
            source,
            destination_directory,
            destination,
            libc::RENAME_EXCL,
        )
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
unsafe fn renameat_exclusive(
    _directory: libc::c_int,
    _source: *const libc::c_char,
    _destination_directory: libc::c_int,
    _destination: *const libc::c_char,
) -> libc::c_int {
    -1
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeletionEntryIdentity {
    device: u64,
    inode: u64,
    kind: libc::mode_t,
}

#[cfg(unix)]
impl DeletionEntryIdentity {
    fn from_stat(stat: &libc::stat) -> Self {
        Self {
            // Preserve the signed macOS dev_t conversion used by slot identities.
            #[cfg(target_os = "macos")]
            device: i64::from(stat.st_dev).cast_unsigned(),
            #[cfg(not(target_os = "macos"))]
            device: stat.st_dev,
            inode: stat.st_ino,
            kind: stat.st_mode & libc::S_IFMT,
        }
    }

    fn pair(self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

#[cfg(unix)]
fn deletion_descriptor_stat(descriptor: &std::os::fd::OwnedFd) -> Result<libc::stat, String> {
    use std::os::fd::AsRawFd as _;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: descriptor is retained and stat is writable.
    if unsafe { libc::fstat(descriptor.as_raw_fd(), stat.as_mut_ptr()) } < 0 {
        return Err(format!(
            "cannot identify retained deletion descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fstat initialized stat.
    Ok(unsafe { stat.assume_init() })
}

#[cfg(unix)]
fn deletion_slot_stat(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::CStr,
) -> Result<libc::stat, String> {
    use std::os::fd::AsRawFd as _;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: parent/name are retained; no symlink is followed.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(format!(
            "cannot revalidate deletion slot: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fstatat initialized stat.
    Ok(unsafe { stat.assume_init() })
}

#[cfg(unix)]
fn open_deletion_parent(path: &Path) -> Result<std::sync::Arc<std::os::fd::OwnedFd>, String> {
    use std::os::fd::FromRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "deletion parent contains NUL")?;
    // SAFETY: path is a valid C string; reject a symlink at the parent slot.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "cannot retain deletion parent: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fd is newly owned.
    Ok(std::sync::Arc::new(unsafe {
        std::os::fd::OwnedFd::from_raw_fd(fd)
    }))
}

/// Pins BOTH the entry and its parent, not merely the parent descriptor. Each
/// effect rechecks the no-follow slot identity after admission waits. This is
/// still check-then-unlink, not an atomic compare-and-unlink primitive.
#[cfg(unix)]
#[derive(Clone)]
struct OwnedDeletionSlot {
    parent: std::sync::Arc<std::os::fd::OwnedFd>,
    parent_identity: DeletionEntryIdentity,
    name: std::ffi::CString,
    retained: std::sync::Arc<std::os::fd::OwnedFd>,
    identity: DeletionEntryIdentity,
}

// Keep the owning slot borrowed through the synchronous scan: no dup or raw-fd lease.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn deletion_descriptor_device(stat: &libc::stat) -> u64 {
    #[cfg(target_os = "macos")]
    {
        // libproc exposes the same dev_t bits unsigned, unlike fstat's signed dev_t.
        u64::from(stat.st_dev.cast_unsigned())
    }
    #[cfg(target_os = "linux")]
    {
        stat.st_dev
    }
}

struct SelfDeletionDescriptorBorrow<'a> {
    #[cfg(unix)]
    witness: &'a OwnedDeletionSlot,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pid: libc::pid_t,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fd: libc::c_int,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    device: u64,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    inode: u64,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    kind: libc::mode_t,
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    _borrow: std::marker::PhantomData<&'a ()>,
}

#[cfg(unix)]
impl OwnedDeletionSlot {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn self_descriptor_borrow(&self) -> Result<SelfDeletionDescriptorBorrow<'_>, String> {
        use std::os::fd::AsRawFd as _;
        self.validate()?;
        let stat = deletion_descriptor_stat(&self.retained)?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(
                "self-descriptor exemption requires the live owned directory object".into(),
            );
        }
        Ok(SelfDeletionDescriptorBorrow {
            witness: self,
            pid: unsafe { libc::getpid() },
            fd: self.retained.as_raw_fd(),
            device: deletion_descriptor_device(&stat),
            inode: stat.st_ino,
            kind: stat.st_mode & libc::S_IFMT,
        })
    }

    fn capture(
        parent: &std::sync::Arc<std::os::fd::OwnedFd>,
        name: &std::ffi::CStr,
        expected: DeletionEntryIdentity,
    ) -> Result<Self, String> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        let flags = if expected.kind == libc::S_IFDIR {
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW
        } else {
            // Retain symlinks themselves and avoid opening FIFOs/devices for I/O.
            #[cfg(target_os = "linux")]
            {
                libc::O_PATH | libc::O_NOFOLLOW
            }
            #[cfg(target_os = "macos")]
            {
                libc::O_EVTONLY | libc::O_SYMLINK
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                return Err("retained deletion slots unsupported on this Unix platform".into());
            }
        };
        // SAFETY: parent/name are live; flags do not follow the entry symlink.
        let fd =
            unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(format!(
                "cannot retain deletion slot: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: fd is newly owned.
        let retained = std::sync::Arc::new(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
        let witness = Self {
            parent: std::sync::Arc::clone(parent),
            parent_identity: DeletionEntryIdentity::from_stat(&deletion_descriptor_stat(parent)?),
            name: name.to_owned(),
            retained,
            identity: expected,
        };
        witness.validate()?;
        Ok(witness)
    }

    fn validate(&self) -> Result<(), String> {
        if DeletionEntryIdentity::from_stat(&deletion_descriptor_stat(&self.parent)?)
            != self.parent_identity
            || DeletionEntryIdentity::from_stat(&deletion_descriptor_stat(&self.retained)?)
                != self.identity
            || DeletionEntryIdentity::from_stat(&deletion_slot_stat(&self.parent, &self.name)?)
                != self.identity
        {
            return Err("identity-bound deletion slot was replaced; replacement preserved".into());
        }
        Ok(())
    }

    fn unlink(self, deadline: Option<std::time::Instant>) -> Result<(), String> {
        use std::os::fd::AsRawFd as _;
        check_directory_deletion_deadline(deadline)?;
        self.validate()?;
        let flags = if self.identity.kind == libc::S_IFDIR {
            libc::AT_REMOVEDIR
        } else {
            0
        };
        // SAFETY: self retains descriptor/name/identity through this syscall.
        if unsafe { libc::unlinkat(self.parent.as_raw_fd(), self.name.as_ptr(), flags) } < 0 {
            return Err(format!(
                "cannot unlink identity-bound deletion slot: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// Owned final relocation leaf. The destination is the EXACT retained root FD,
/// not a path re-resolved after admission; exclusive rename never overwrites an
/// existing object. Source and root slot checks are intentionally non-atomic.
#[cfg(unix)]
struct OwnedWorktreeRenameLeaf {
    source_path: PathBuf,
    source: OwnedDeletionSlot,
    root: OwnedDeletionSlot,
}

#[cfg(unix)]
impl OwnedWorktreeRenameLeaf {
    fn rename(self, deadline: Option<std::time::Instant>) -> Result<std::io::Result<()>, String> {
        use std::os::fd::AsRawFd as _;
        check_directory_deletion_deadline(deadline)?;
        self.source.validate()?;
        self.root.validate()?;
        let object = std::ffi::CString::new("object").expect("static name");
        // SAFETY: self retains both verified descriptors and names. The kernel
        // supplies non-overwrite semantics; the source identity check is separate.
        if unsafe {
            renameat_exclusive(
                self.source.parent.as_raw_fd(),
                self.source.name.as_ptr(),
                self.root.retained.as_raw_fd(),
                object.as_ptr(),
            )
        } < 0
        {
            return Ok(Err(std::io::Error::last_os_error()));
        }
        let moved =
            DeletionEntryIdentity::from_stat(&deletion_slot_stat(&self.root.retained, &object)?);
        if moved != self.source.identity {
            return Err("final tombstone object differs from owned rename source".into());
        }
        Ok(Ok(()))
    }
}

/// Mandatory per-syscall admission for shared worktree deletion leaves.
///
/// Implementations execute the owned effect synchronously to completion or refuse
/// it without calling it; effects must not outlive this call. An executor error
/// (including an unknown outcome after execution) stops traversal without retry
/// or compensation. Native callers reenter their original effect context here.
/// Preparation and durable custody callbacks stay outside it; descriptor/slot
/// identity revalidation belongs inside each admitted mutation callback.
trait WorktreeMutationExecutor {
    fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static;
}

/// Compatibility only: legacy callers retain their existing direct execution.
struct DirectWorktreeMutationExecutor;

impl WorktreeMutationExecutor for DirectWorktreeMutationExecutor {
    fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        effect()
    }
}

#[cfg(unix)]
#[allow(
    dead_code,
    reason = "legacy direct-execution signature retained for compatibility"
)]
fn bind_deletion_entry_to_private_slot(
    directory: libc::c_int,
    name: &std::ffi::CStr,
    expected: &libc::stat,
) -> Result<(std::ffi::CString, libc::stat), String> {
    use std::os::fd::FromRawFd as _;

    // Duplicate before constructing any 'static mutation closure. Invalid legacy
    // descriptors produce an OS error rather than an invalid BorrowedFd.
    // SAFETY: fcntl validates the input descriptor and returns a new owned fd.
    let duplicate = unsafe { libc::fcntl(directory, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(format!(
            "cannot duplicate deletion descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: duplicate is newly owned on success above.
    let directory = unsafe { std::os::fd::OwnedFd::from_raw_fd(duplicate) };
    bind_deletion_entry_to_private_slot_with_executor(
        &std::sync::Arc::new(directory),
        name,
        expected,
        None,
        &mut DirectWorktreeMutationExecutor,
    )
}

#[cfg(unix)]
fn bind_deletion_entry_to_private_slot_with_executor<E: WorktreeMutationExecutor>(
    directory: &std::sync::Arc<std::os::fd::OwnedFd>,
    name: &std::ffi::CStr,
    expected: &libc::stat,
    deadline: Option<std::time::Instant>,
    executor: &mut E,
) -> Result<(std::ffi::CString, libc::stat), String> {
    use std::os::fd::AsRawFd as _;

    let slot = std::ffi::CString::new(format!(
        ".phoenix-delete-entry-{}",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("generated deletion slot contains no NUL");
    let effect_directory = std::sync::Arc::clone(directory);
    let effect_name = name.to_owned();
    let effect_slot = slot.clone();
    executor.execute(move || {
        check_directory_deletion_deadline(deadline)?;
        // SAFETY: the effect owns the descriptor and C strings. Exclusive
        // rename cannot overwrite another entry at the private destination.
        if unsafe {
            renameat_exclusive(
                effect_directory.as_raw_fd(),
                effect_name.as_ptr(),
                effect_directory.as_raw_fd(),
                effect_slot.as_ptr(),
            )
        } < 0
        {
            return Err(format!(
                "cannot bind deletion entry to private slot: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    })?;

    let mut moved = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: descriptor and slot are valid; moved points to writable storage.
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
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
fn remove_directory_contents_at_with_hook<H>(
    directory: &std::os::fd::OwnedFd,
    after_inspection: &mut H,
) -> Result<(), String>
where
    H: FnMut(libc::c_int, &std::ffi::CStr),
{
    // Preserve existing unbounded callers; deadline-aware callers supply their own
    // captured absolute deadline to the shared implementation below.
    remove_directory_contents_at_with_hook_and_deadline(directory, after_inspection, None)
}

fn check_directory_deletion_deadline(deadline: Option<std::time::Instant>) -> Result<(), String> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err("descriptor-bound directory deletion exceeded its deadline".to_string());
    }
    Ok(())
}

#[cfg(unix)]
#[allow(clippy::too_many_lines)]
fn remove_directory_contents_at_with_hook_and_deadline<H>(
    directory: &std::os::fd::OwnedFd,
    after_inspection: &mut H,
    deadline: Option<std::time::Instant>,
) -> Result<(), String>
where
    H: FnMut(libc::c_int, &std::ffi::CStr),
{
    check_directory_deletion_deadline(deadline)?;
    let directory = directory
        .try_clone()
        .map_err(|error| format!("cannot duplicate deletion descriptor: {error}"))?;
    remove_directory_contents_at_with_hook_and_deadline_with_executor(
        &std::sync::Arc::new(directory),
        after_inspection,
        deadline,
        &mut DirectWorktreeMutationExecutor,
    )
}

#[cfg(unix)]
#[allow(clippy::too_many_lines)]
fn remove_directory_contents_at_with_hook_and_deadline_with_executor<H, E>(
    directory: &std::sync::Arc<std::os::fd::OwnedFd>,
    after_inspection: &mut H,
    deadline: Option<std::time::Instant>,
    executor: &mut E,
) -> Result<(), String>
where
    H: FnMut(libc::c_int, &std::ffi::CStr),
    E: WorktreeMutationExecutor,
{
    use std::ffi::CStr;
    use std::os::fd::AsRawFd as _;

    // These are cooperative checks, not a hard wall-clock bound on in-flight
    // syscalls. Destructive custody stays with this synchronous walk until return;
    // expiry is not proof of removal and may leave an identity-bound private slot.
    check_directory_deletion_deadline(deadline)?;
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
            check_directory_deletion_deadline(deadline)?;
            // SAFETY: stream remains valid until closed below. errno is reset so
            // a null result can distinguish end-of-directory from an error.
            unsafe { *errno_location() = 0 };
            // SAFETY: stream is a valid DIR pointer owned by this function.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                return if error.raw_os_error() == Some(0) {
                    check_directory_deletion_deadline(deadline)
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
            check_directory_deletion_deadline(deadline)?;
            let (slot, moved) = bind_deletion_entry_to_private_slot_with_executor(
                directory, name, &metadata, deadline, executor,
            )?;
            let witness = OwnedDeletionSlot::capture(
                directory,
                &slot,
                DeletionEntryIdentity::from_stat(&moved),
            )?;
            if witness.identity.kind == libc::S_IFDIR {
                remove_directory_contents_at_with_hook_and_deadline_with_executor(
                    &witness.retained,
                    after_inspection,
                    deadline,
                    executor,
                )?;
            }
            check_directory_deletion_deadline(deadline)?;
            let effect_witness = witness.clone();
            executor.execute(move || effect_witness.unlink(deadline))?;
        }
    })();
    // SAFETY: stream is the valid DIR pointer returned by fdopendir.
    unsafe { libc::closedir(stream) };
    result?;
    check_directory_deletion_deadline(deadline)
}

#[cfg(test)]
fn reserve_private_tombstone(root: &Path, description: &str) -> Result<(), String> {
    reserve_private_tombstone_with_executor(
        root,
        description,
        None,
        &mut DirectWorktreeMutationExecutor,
    )
}

fn reserve_private_tombstone_with_executor<E: WorktreeMutationExecutor>(
    root: &Path,
    description: &str,
    deadline: Option<std::time::Instant>,
    executor: &mut E,
) -> Result<(), String> {
    let root = root.to_path_buf();
    let description = description.to_string();
    executor.execute(move || {
        check_directory_deletion_deadline(deadline)?;
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
    })
}

#[cfg(unix)]
fn validate_private_tombstone_root(
    is_directory: bool,
    owner: u32,
    mode: u64,
) -> Result<(), String> {
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let effective_uid = unsafe { libc::geteuid() };
    if !is_directory || owner != effective_uid || mode & 0o077 != 0 {
        return Err("private final tombstone must be an effective-user-owned directory with owner-only permissions".to_string());
    }
    Ok(())
}

#[cfg(test)]
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
    bind_root: B,
    after_identity_observation: A,
    description: &str,
) -> Result<(), String>
where
    F: FnOnce(&Path),
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String>,
    A: FnOnce(&Path, &Path) -> Result<(), String>,
    O: Fn(&Path) -> Result<String, String>,
{
    remove_identity_bound_directory_with_deadline(
        deletion_target,
        tombstone_root,
        expected_identity,
        observe_identity,
        before_final_move,
        bind_root,
        after_identity_observation,
        description,
        None,
    )
}

#[allow(
    dead_code,
    reason = "legacy direct-execution signature retained for compatibility"
)]
fn remove_empty_tombstone_before_deadline(tombstone: &Path, deadline: Option<std::time::Instant>) {
    let _ = remove_empty_tombstone_before_deadline_with_executor(
        tombstone,
        deadline,
        &mut DirectWorktreeMutationExecutor,
    );
}

fn remove_empty_tombstone_before_deadline_with_executor<E: WorktreeMutationExecutor>(
    tombstone: &Path,
    deadline: Option<std::time::Instant>,
    executor: &mut E,
) -> Result<(), String> {
    check_directory_deletion_deadline(deadline)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let parent = open_deletion_parent(tombstone.parent().ok_or("tombstone has no parent")?)?;
        let name = std::ffi::CString::new(
            tombstone
                .file_name()
                .ok_or("tombstone has no name")?
                .as_bytes(),
        )
        .map_err(|_| "tombstone name contains NUL")?;
        let stat = deletion_slot_stat(&parent, &name)?;
        let identity = DeletionEntryIdentity::from_stat(&stat);
        if identity.kind != libc::S_IFDIR {
            return Err("empty tombstone cleanup requires a no-follow directory".into());
        }
        let witness = OwnedDeletionSlot::capture(&parent, &name, identity)?;
        let effect_witness = witness.clone();
        executor.execute(move || effect_witness.unlink(deadline))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (tombstone, executor);
        return Err("identity-bound deletion is unsupported on this platform".into());
    }
    Ok(())
}

// This helper accepts a budget but does not establish original physical authority or ownership.
#[allow(clippy::too_many_arguments)]
fn remove_identity_bound_directory_with_deadline<F, B, A, O>(
    deletion_target: &Path,
    tombstone_root: Option<&Path>,
    expected_identity: &str,
    observe_identity: O,
    before_final_move: F,
    bind_root: B,
    after_identity_observation: A,
    description: &str,
    deadline: Option<std::time::Instant>,
) -> Result<(), String>
where
    F: FnOnce(&Path),
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String>,
    A: FnOnce(&Path, &Path) -> Result<(), String>,
    O: Fn(&Path) -> Result<String, String>,
{
    remove_identity_bound_directory_with_deadline_with_executor(
        deletion_target,
        tombstone_root,
        expected_identity,
        observe_identity,
        before_final_move,
        bind_root,
        after_identity_observation,
        description,
        deadline,
        &mut DirectWorktreeMutationExecutor,
    )
}

// Admission surrounds individual mutations, never recursive traversal or durable
// bind_root callbacks: those callbacks may themselves need a database writer.
trait WorktreeDeletionDriver: WorktreeMutationExecutor {
    fn allows_existing_root(&self) -> bool {
        true
    }

    fn cleanup_after_failure(&self) -> bool {
        true
    }

    /// The physical driver adds immutable registration checks inside the same
    /// admitted closure as the rename, after any wait for source-entry authority.
    #[cfg(unix)]
    fn rename_to_tombstone(
        &mut self,
        leaf: OwnedWorktreeRenameLeaf,
        deadline: Option<std::time::Instant>,
    ) -> Result<std::io::Result<()>, String> {
        self.execute(move || leaf.rename(deadline))
    }

    fn before_move(&mut self, source: &Path) -> Result<(), String>;
    fn bind_root(
        &mut self,
        root: &Path,
        root_identity: (u64, u64),
        object_identity: Option<(u64, u64)>,
    ) -> Result<(), String>;
    fn observe_identity(&mut self, object: &Path) -> Result<String, String>;
    fn after_move(
        &mut self,
        root: &Path,
        object: &Path,
        self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
    ) -> Result<(), String>;
}

struct CallbackWorktreeDeletionDriver<'a, F, B, A, O, E> {
    before: Option<F>,
    bind: B,
    after: Option<A>,
    observe: O,
    executor: &'a mut E,
}

impl<F, B, A, O, E: WorktreeMutationExecutor> WorktreeMutationExecutor
    for CallbackWorktreeDeletionDriver<'_, F, B, A, O, E>
{
    fn execute<T, C>(&mut self, effect: C) -> Result<T, String>
    where
        T: Send + 'static,
        C: FnOnce() -> Result<T, String> + Send + 'static,
    {
        self.executor.execute(effect)
    }
}

impl<F, B, A, O, E> WorktreeDeletionDriver for CallbackWorktreeDeletionDriver<'_, F, B, A, O, E>
where
    F: FnOnce(&Path),
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String>,
    A: FnOnce(&Path, &Path) -> Result<(), String>,
    O: Fn(&Path) -> Result<String, String>,
    E: WorktreeMutationExecutor,
{
    fn before_move(&mut self, source: &Path) -> Result<(), String> {
        self.before.take().expect("one final move")(source);
        Ok(())
    }

    fn bind_root(
        &mut self,
        root: &Path,
        identity: (u64, u64),
        object: Option<(u64, u64)>,
    ) -> Result<(), String> {
        (self.bind)(root, identity, object)
    }

    fn observe_identity(&mut self, object: &Path) -> Result<String, String> {
        (self.observe)(object)
    }

    fn after_move(
        &mut self,
        root: &Path,
        object: &Path,
        _self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
    ) -> Result<(), String> {
        self.after.take().expect("one final move")(root, object)
    }
}

#[allow(clippy::too_many_arguments)]
fn remove_identity_bound_directory_with_deadline_with_executor<F, B, A, O, E>(
    deletion_target: &Path,
    tombstone_root: Option<&Path>,
    expected_identity: &str,
    observe_identity: O,
    before_final_move: F,
    bind_root: B,
    after_identity_observation: A,
    description: &str,
    deadline: Option<std::time::Instant>,
    executor: &mut E,
) -> Result<(), String>
where
    F: FnOnce(&Path),
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String>,
    A: FnOnce(&Path, &Path) -> Result<(), String>,
    O: Fn(&Path) -> Result<String, String>,
    E: WorktreeMutationExecutor,
{
    remove_identity_bound_directory_with_driver(
        deletion_target,
        tombstone_root,
        expected_identity,
        description,
        deadline,
        &mut CallbackWorktreeDeletionDriver {
            before: Some(before_final_move),
            bind: bind_root,
            after: Some(after_identity_observation),
            observe: observe_identity,
            executor,
        },
    )
}

#[cfg(not(unix))]
fn remove_identity_bound_directory_with_driver<D: WorktreeDeletionDriver>(
    _deletion_target: &Path,
    _tombstone_root: Option<&Path>,
    _expected_identity: &str,
    description: &str,
    deadline: Option<std::time::Instant>,
    _executor: &mut D,
) -> Result<(), String> {
    check_directory_deletion_deadline(deadline)?;
    Err(format!(
        "identity-bound {description} deletion is unsupported on this platform"
    ))
}

#[cfg(unix)]
#[allow(
    clippy::too_many_lines,
    clippy::cast_sign_loss,
    clippy::unnecessary_cast
)]
fn remove_identity_bound_directory_with_driver<D: WorktreeDeletionDriver>(
    deletion_target: &Path,
    tombstone_root: Option<&Path>,
    expected_identity: &str,
    description: &str,
    deadline: Option<std::time::Instant>,
    executor: &mut D,
) -> Result<(), String> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    check_directory_deletion_deadline(deadline)?;
    executor.before_move(deletion_target)?;
    let fresh_root = tombstone_root.is_none() || !executor.allows_existing_root();
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
    check_directory_deletion_deadline(deadline)?;
    let parent_descriptor = open_deletion_parent(parent)?;
    let source_name = std::ffi::CString::new(
        deletion_target
            .file_name()
            .ok_or("missing deletion leaf")?
            .as_bytes(),
    )
    .map_err(|_| "deletion source name contains NUL")?;
    let source_identity =
        DeletionEntryIdentity::from_stat(&deletion_slot_stat(&parent_descriptor, &source_name)?);
    if source_identity.kind != libc::S_IFDIR {
        return Err("final deletion source is not a no-follow directory".into());
    }
    let source_witness =
        OwnedDeletionSlot::capture(&parent_descriptor, &source_name, source_identity)?;
    let root_name = std::ffi::CString::new(
        tombstone_root
            .file_name()
            .ok_or("missing tombstone leaf")?
            .as_bytes(),
    )
    .map_err(|_| "tombstone name contains NUL")?;
    // Creation and descriptor binding complete in one admitted callback, before
    // the durable root bind or any move. Never reopen this reservation by path.
    let root_witness = if fresh_root || !tombstone_root.exists() {
        let parent_descriptor = std::sync::Arc::clone(&parent_descriptor);
        executor.execute(move || {
            check_directory_deletion_deadline(deadline)?;
            // SAFETY: descriptor/name are owned; mkdirat never replaces an entry.
            if unsafe { libc::mkdirat(parent_descriptor.as_raw_fd(), root_name.as_ptr(), 0o700) }
                < 0
            {
                return Err(format!(
                    "cannot reserve private tombstone: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let stat = deletion_slot_stat(&parent_descriptor, &root_name)?;
            OwnedDeletionSlot::capture(
                &parent_descriptor,
                &root_name,
                DeletionEntryIdentity::from_stat(&stat),
            )
        })?
    } else {
        let stat = deletion_slot_stat(&parent_descriptor, &root_name)?;
        OwnedDeletionSlot::capture(
            &parent_descriptor,
            &root_name,
            DeletionEntryIdentity::from_stat(&stat),
        )?
    };
    let root_stat = deletion_descriptor_stat(&root_witness.retained)?;
    validate_private_tombstone_root(
        root_witness.identity.kind == libc::S_IFDIR,
        root_stat.st_uid,
        u64::from(root_stat.st_mode),
    )?;
    let tombstone = tombstone_root.join("object");
    let verified_root = root_witness.identity.pair();
    check_directory_deletion_deadline(deadline)?;
    if let Err(error) = executor.bind_root(&tombstone_root, verified_root, None) {
        if !executor.cleanup_after_failure() {
            return Err(error);
        }
        let cleanup = root_witness.clone();
        executor.execute(move || cleanup.unlink(deadline))?;
        return Err(error);
    }
    check_directory_deletion_deadline(deadline)?;
    // Keep syscall failure distinct from executor refusal: refusal must return
    // immediately, without even a compensating tombstone cleanup admission.
    let rename_result = executor.rename_to_tombstone(
        OwnedWorktreeRenameLeaf {
            source_path: deletion_target.to_path_buf(),
            source: source_witness.clone(),
            root: root_witness.clone(),
        },
        deadline,
    )?;
    if let Err(error) = rename_result {
        // Legacy callers may clean their empty reservation. Native Close stops
        // at the first failed syscall and retains the bound root as evidence.
        if executor.cleanup_after_failure() {
            let cleanup = root_witness.clone();
            executor.execute(move || cleanup.unlink(deadline))?;
        }
        return Err(format!(
            "cannot move {description} into private final tombstone: {error}"
        ));
    }
    check_directory_deletion_deadline(deadline)?;
    let object_witness = OwnedDeletionSlot {
        parent: std::sync::Arc::clone(&root_witness.retained),
        parent_identity: root_witness.identity,
        name: std::ffi::CString::new("object").expect("static name"),
        retained: std::sync::Arc::clone(&source_witness.retained),
        identity: source_witness.identity,
    };
    object_witness.validate()?;
    let verified_object = object_witness.identity.pair();
    check_directory_deletion_deadline(deadline)?;
    executor.bind_root(&tombstone_root, verified_root, Some(verified_object))?;
    check_directory_deletion_deadline(deadline)?;
    if executor.observe_identity(&tombstone)? != expected_identity {
        return Err(format!(
            "{description} identity changed before final deletion; replacement preserved at {}",
            tombstone.display()
        ));
    }
    check_directory_deletion_deadline(deadline)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let self_descriptor = Some(object_witness.self_descriptor_borrow()?);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let self_descriptor: Option<SelfDeletionDescriptorBorrow<'_>> = None;
    executor.after_move(&tombstone_root, &tombstone, self_descriptor.as_ref())?;
    check_directory_deletion_deadline(deadline)?;
    root_witness.validate()?;
    object_witness.validate()?;
    remove_directory_contents_at_with_hook_and_deadline_with_executor(
        &object_witness.retained,
        &mut |_, _| {},
        deadline,
        executor,
    )?;
    // Keep the original descriptors alive through release classification of each
    // admitted leaf, including final reservation removal through its parent FD.
    check_directory_deletion_deadline(deadline)?;
    let object_effect = object_witness.clone();
    executor.execute(move || object_effect.unlink(deadline))?;
    check_directory_deletion_deadline(deadline)?;
    let root_effect = root_witness.clone();
    executor.execute(move || root_effect.unlink(deadline))?;
    check_directory_deletion_deadline(deadline)
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
fn resume_final_worktree_tombstone(
    tombstone: &CloseWorktreeFinalTombstone,
    identity: &WorktreeIdentity,
    confirmed: &CloseRetirementSnapshot,
    probe: CloseWriterProbe,
) -> FinalTombstoneRecovery {
    resume_final_worktree_tombstone_with_writer_probe(tombstone, identity, confirmed, move |path| {
        Ok(probe
            .observe(path, quarantine_has_external_writer)?
            .blocks_retirement())
    })
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
fn resume_final_worktree_tombstone_with_writer_probe<F>(
    tombstone: &CloseWorktreeFinalTombstone,
    identity: &WorktreeIdentity,
    confirmed: &CloseRetirementSnapshot,
    writer_probe: F,
) -> FinalTombstoneRecovery
where
    F: FnOnce(&Path) -> Result<bool, String>,
{
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
    if let Err(detail) = validate_private_tombstone_root(
        root_stat.st_mode & libc::S_IFMT == libc::S_IFDIR,
        root_stat.st_uid,
        u64::from(root_stat.st_mode),
    ) {
        return FinalTombstoneRecovery::Residual(detail);
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
        Ok(true) => match writer_probe(&object) {
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
    _probe: CloseWriterProbe,
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

fn administrative_dir_final_tombstone_path(quarantine: &Path) -> PathBuf {
    let mut path = quarantine.as_os_str().to_os_string();
    path.push("-delete");
    PathBuf::from(path)
}

fn fresh_administrative_dir_final_tombstone_path(quarantine: &Path) -> PathBuf {
    let mut path = administrative_dir_final_tombstone_path(quarantine).into_os_string();
    path.push(format!("-{}", uuid::Uuid::new_v4().simple()));
    PathBuf::from(path)
}

fn reject_administrative_final_tombstone_residuals(
    quarantine: &Path,
    deadline: Option<std::time::Instant>,
) -> Result<(), String> {
    let legacy = administrative_dir_final_tombstone_path(quarantine);
    check_directory_deletion_deadline(deadline)?;
    let legacy_metadata = std::fs::symlink_metadata(&legacy);
    check_directory_deletion_deadline(deadline)?;
    match legacy_metadata {
        Ok(_) => {
            return Err(format!(
                "worktree administrative cleanup has an unresolved final tombstone at {}",
                legacy.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect administrative final tombstone: {error}"
            ))
        }
    }
    let parent = legacy
        .parent()
        .ok_or_else(|| "administrative final tombstone has no parent".to_string())?;
    let mut prefix = legacy
        .file_name()
        .ok_or_else(|| "administrative final tombstone has no name".to_string())?
        .to_os_string();
    prefix.push("-");
    check_directory_deletion_deadline(deadline)?;
    let entries = std::fs::read_dir(parent);
    check_directory_deletion_deadline(deadline)?;
    let mut entries = match entries {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            check_directory_deletion_deadline(deadline)?;
            let metadata = std::fs::symlink_metadata(parent);
            check_directory_deletion_deadline(deadline)?;
            return match metadata {
                // An absent parent cannot contain a legacy or randomized residual.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                _ => Err("cannot enumerate administrative final tombstones: parent is present or inaccessible".to_string()),
            };
        }
        Err(error) => {
            return Err(format!(
                "cannot enumerate administrative final tombstones: {error}"
            ))
        }
    };
    loop {
        check_directory_deletion_deadline(deadline)?;
        let entry = entries.next();
        check_directory_deletion_deadline(deadline)?;
        let Some(entry) = entry else {
            return Ok(());
        };
        let entry = entry.map_err(|error| {
            format!("cannot read administrative final tombstone entry: {error}")
        })?;
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(prefix.as_encoded_bytes())
        {
            return Err(format!(
                "worktree administrative cleanup has an unresolved final tombstone at {}",
                entry.path().display()
            ));
        }
    }
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

fn remove_exact_worktree_administrative_dir_until(
    administrative_dir: &Path,
    expected_incarnation: &str,
    deadline: std::time::Instant,
) -> Result<(), String> {
    remove_exact_worktree_administrative_dir_with_hook_and_deadline(
        administrative_dir,
        expected_incarnation,
        |_| {},
        Some(deadline),
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
    remove_exact_worktree_administrative_dir_with_hook_and_deadline(
        administrative_dir,
        expected_incarnation,
        before_final_move,
        None,
    )
}

fn remove_exact_worktree_administrative_dir_with_hook_and_deadline<F>(
    administrative_dir: &Path,
    expected_incarnation: &str,
    before_final_move: F,
    deadline: Option<std::time::Instant>,
) -> Result<(), String>
where
    F: FnOnce(&Path),
{
    remove_exact_worktree_administrative_dir_with_hook_and_deadline_with_executor(
        administrative_dir,
        expected_incarnation,
        before_final_move,
        deadline,
        &mut DirectWorktreeMutationExecutor,
    )
}

fn remove_exact_worktree_administrative_dir_with_hook_and_deadline_with_executor<F, E>(
    administrative_dir: &Path,
    expected_incarnation: &str,
    before_final_move: F,
    deadline: Option<std::time::Instant>,
    executor: &mut E,
) -> Result<(), String>
where
    F: FnOnce(&Path),
    E: WorktreeMutationExecutor,
{
    check_directory_deletion_deadline(deadline)?;
    let quarantine = administrative_dir_quarantine_path(administrative_dir, expected_incarnation)?;
    reject_administrative_final_tombstone_residuals(&quarantine, deadline)?;
    let source_exists = administrative_dir
        .try_exists()
        .map_err(|error| error.to_string())?;
    let quarantine_exists = quarantine.try_exists().map_err(|error| error.to_string())?;
    check_directory_deletion_deadline(deadline)?;
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
            check_directory_deletion_deadline(deadline)?;
            let source = administrative_dir.to_path_buf();
            let destination = quarantine.clone();
            executor.execute(move || {
                check_directory_deletion_deadline(deadline)?;
                std::fs::rename(source, destination).map_err(|error| {
                    format!("cannot quarantine exact worktree administrative directory: {error}")
                })
            })?;
            quarantine
        }
    };
    check_directory_deletion_deadline(deadline)?;
    if observe_administrative_dir_incarnation(&deletion_target)? != expected_incarnation {
        return Err(
            "quarantined worktree administrative-directory incarnation changed".to_string(),
        );
    }
    check_directory_deletion_deadline(deadline)?;
    let final_tombstone = fresh_administrative_dir_final_tombstone_path(&deletion_target);
    reserve_private_tombstone_with_executor(
        &final_tombstone,
        "retired worktree administrative directory",
        deadline,
        executor,
    )?;
    remove_identity_bound_directory_with_deadline_with_executor(
        &deletion_target,
        Some(&final_tombstone),
        expected_incarnation,
        observe_administrative_dir_incarnation,
        before_final_move,
        |_, _, _| Ok(()),
        |_, _| Ok(()),
        "retired worktree administrative directory",
        deadline,
        executor,
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
        // Exact C-locale diagnostics from Git's fsmonitor-ipc.c and
        // builtin/fsmonitor--daemon.c (#else !HAVE_FSMONITOR_DAEMON_BACKEND).
        // Unsupported Git builds (including Linux) have no applicable daemon.
        && matches!(
            output.stderr.strip_suffix(b"\n").unwrap_or(&output.stderr),
            b"fatal: fsmonitor--daemon is not running"
                | b"fatal: fsmonitor--daemon not supported on this platform"
        )
    {
        return Ok(FsmonitorStop::NotRunning);
    }
    Err(format!(
        "fsmonitor stop failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

#[cfg(test)]
fn stop_bound_fsmonitor_daemons_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<FsmonitorStop, String> {
    stop_bound_fsmonitor_daemons_with_admission_until(path, deadline, None)
}

fn stop_bound_fsmonitor_daemons_with_admission_until(
    path: &Path,
    deadline: std::time::Instant,
    first_submodule_spawn_admission: Option<SubmoduleSpawnAdmissionEffect>,
) -> Result<FsmonitorStop, String> {
    if std::time::Instant::now() >= deadline {
        return Err("submodule fsmonitor shutdown exceeded its deadline".to_string());
    }
    let arguments = [
        "submodule",
        "foreach",
        "--quiet",
        "--recursive",
        r#"output=$(git fsmonitor--daemon stop 2>&1); result=$?; if [ "$result" -eq 0 ] || { [ "$result" -eq 128 ] && { [ "$output" = 'fatal: fsmonitor--daemon is not running' ] || [ "$output" = 'fatal: fsmonitor--daemon not supported on this platform' ]; }; }; then exit 0; fi; printf '%s\n' "$output" >&2; exit "$result""#,
    ];
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    bind_git_command_to_worktree_until(&mut command, path, deadline)?;
    command
        .args(arguments)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    if std::time::Instant::now() >= deadline {
        return Err(
            "Close teardown deadline elapsed before recursive fsmonitor shutdown spawn".to_string(),
        );
    }
    let child = match first_submodule_spawn_admission {
        Some(admit_spawn) => admit_spawn(&mut command, deadline)?,
        None => command.spawn().map_err(|error| error.to_string())?,
    };
    let submodules = wait_bounded_child_output(child, deadline, "submodule fsmonitor shutdown")?;
    if !submodules.status.success() {
        return Err(format!(
            "submodule fsmonitor shutdown failed ({}): {}",
            submodules.status,
            String::from_utf8_lossy(&submodules.stderr).trim()
        ));
    }
    stop_bound_fsmonitor_with_leaf_until(path, deadline, &mut run_bounded_spawned_git)
}

/// Stops only this bound repository's daemon. Native callers must enumerate
/// initialized submodules and invoke this leaf for each one; the legacy recursive
/// `submodule foreach` helper above launches descendants outside leaf admission.
fn stop_bound_fsmonitor_with_leaf_until(
    path: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<FsmonitorStop, String> {
    let worktree = run_bounded_git_command_with_leaf_until(
        path,
        &["fsmonitor--daemon", "stop"],
        None,
        deadline,
        "worktree fsmonitor shutdown",
        git_leaf,
    )?;
    classify_fsmonitor_stop(&worktree)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ExternalWriterEvidence {
    Observed(CloseWriterObservation),
    NoPositiveEvidence,
}

impl ExternalWriterEvidence {
    fn blocks_retirement(&self) -> bool {
        matches!(self, Self::Observed(_))
    }

    fn is_positive(&self) -> bool {
        matches!(
            self,
            Self::Observed(CloseWriterObservation::Authority { .. })
        )
    }

    // Preserve one representative unknown unless verified authority supersedes it.
    // Only the final observation is persisted, so it cannot conflict with the
    // immutable per-detector evidence key for this run.
    fn remember_gap(&mut self, evidence: Self) {
        if matches!(self, Self::NoPositiveEvidence)
            && matches!(
                evidence,
                Self::Observed(CloseWriterObservation::InspectionGap { .. })
            )
        {
            *self = evidence;
        }
    }
}

#[derive(Debug)]
enum WriterScanError {
    Gap(CloseWriterGap),
    Stopped(String),
}

impl From<CloseWriterGap> for WriterScanError {
    fn from(gap: CloseWriterGap) -> Self {
        Self::Gap(gap)
    }
}

fn writer_scan_check(deadline: std::time::Instant) -> Result<(), WriterScanError> {
    check_directory_deletion_deadline(Some(deadline)).map_err(WriterScanError::Stopped)
}

/// A failed entry must not mask an independent, well-formed match. The
/// process scan still has to revalidate the incarnation of any matched entry.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn scan_writer_entries<T, M>(
    entries: impl IntoIterator<Item = T>,
    deadline: std::time::Instant,
    mut inspect: impl FnMut(T) -> Result<Option<M>, WriterScanError>,
) -> Result<Option<M>, WriterScanError> {
    let mut entries = entries.into_iter();
    let mut gap = None;
    loop {
        writer_scan_check(deadline)?;
        let entry = entries.next();
        writer_scan_check(deadline)?;
        let Some(entry) = entry else {
            break;
        };
        let result = inspect(entry);
        writer_scan_check(deadline)?;
        match result {
            Ok(Some(authority)) => return Ok(Some(authority)),
            Ok(None) => {}
            Err(WriterScanError::Gap(reason)) => {
                gap.get_or_insert(reason);
            }
            Err(error @ WriterScanError::Stopped(_)) => return Err(error),
        }
    }
    gap.map_or(Ok(None), |reason| Err(WriterScanError::Gap(reason)))
}

fn writer_gap(
    detector: CloseWriterDetector,
    pid: Option<u32>,
    reason: CloseWriterGap,
) -> ExternalWriterEvidence {
    tracing::debug!(
        detector = detector.as_str(),
        pid,
        reason = reason.as_str(),
        "Close external writer inspection incomplete"
    );
    ExternalWriterEvidence::Observed(CloseWriterObservation::InspectionGap {
        detector,
        pid,
        reason,
    })
}

#[derive(Clone)]
pub(crate) struct CloseWriterProbe {
    // Native execution must not classify an untyped writer-observation commit
    // error as a proven terminal failure.
    persistence_uncertain: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    clean_writer_fixture: bool,
    db: crate::db::Database,
    runtime: tokio::runtime::Handle,
    run: CloseRunRef,
    scope: WorkScopeId,
    snapshot: CloseRetirementSnapshot,
    resource: RetiredResourceIdentity,
}

impl CloseWriterProbe {
    fn observe_until<F>(
        &self,
        path: &Path,
        deadline: std::time::Instant,
        production: F,
    ) -> Result<ExternalWriterEvidence, String>
    where
        F: FnOnce(&Path, std::time::Instant) -> Result<ExternalWriterEvidence, String>,
    {
        let evidence = self.observe_and_retain_until(path, deadline, production)?;
        if let ExternalWriterEvidence::Observed(CloseWriterObservation::InspectionGap {
            detector,
            pid,
            reason,
        }) = &evidence
        {
            return Err(format!(
                "Close writer inspection is incomplete; retained typed inspection gap: detector={detector:?}, pid={pid:?}, reason={reason:?}"
            ));
        }
        Ok(evidence)
    }

    fn observe_and_retain_until<F>(
        &self,
        path: &Path,
        deadline: std::time::Instant,
        production: F,
    ) -> Result<ExternalWriterEvidence, String>
    where
        F: FnOnce(&Path, std::time::Instant) -> Result<ExternalWriterEvidence, String>,
    {
        check_directory_deletion_deadline(Some(deadline))?;
        #[cfg(test)]
        if self.clean_writer_fixture {
            return Ok(ExternalWriterEvidence::NoPositiveEvidence);
        }
        let evidence = production(path, deadline)?;
        check_directory_deletion_deadline(Some(deadline))?;
        self.retain_until(evidence, deadline)
    }

    fn observe(
        &self,
        path: &Path,
        production: fn(&Path) -> Result<ExternalWriterEvidence, String>,
    ) -> Result<ExternalWriterEvidence, String> {
        self.observe_until(
            path,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
            |path, _deadline| production(path),
        )
    }

    fn retain_until(
        &self,
        evidence: ExternalWriterEvidence,
        deadline: std::time::Instant,
    ) -> Result<ExternalWriterEvidence, String> {
        check_directory_deletion_deadline(Some(deadline))?;
        if let ExternalWriterEvidence::Observed(observation) = &evidence {
            self.runtime
                .block_on(self.db.record_close_writer_observation(
                    &RecordCloseWriterObservationRequest {
                        run: self.run.clone(),
                        scope: self.scope.clone(),
                        snapshot: self.snapshot.clone(),
                        resource: self.resource.clone(),
                        observation: observation.clone(),
                    },
                ))
                .map_err(|error| {
                    if let Some(uncertain) = &self.persistence_uncertain {
                        uncertain.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    format!("cannot persist Close writer observation; target preserved: {error}")
                })?;
            check_directory_deletion_deadline(Some(deadline))?;
        }
        check_directory_deletion_deadline(Some(deadline))?;
        Ok(evidence)
    }
}

/// A single retry target's observed external-writer status, as seen by the
/// admission boundary. `Absent` means the target path does not exist, so no
/// writer can hold it; `Present` carries the writer evidence for an existing
/// target. Separating observation from the admission decision lets the decision
/// be exercised without touching the filesystem or `/proc`.
#[derive(Clone, Debug, Eq, PartialEq)]
enum RetryTargetObservation {
    Absent,
    Present(ExternalWriterEvidence),
}

fn probe_retry_worktree_writers_with<F>(
    original: &Path,
    quarantine: &Path,
    tombstone: Option<&CloseWorktreeFinalTombstone>,
    mut observe: F,
) -> Result<(), RetryAssessmentError>
where
    F: FnMut(&Path) -> Result<RetryTargetObservation, String>,
{
    let object = tombstone.map(|record| record.root.join("object"));
    for path in [Some(original), Some(quarantine), object.as_deref()]
        .into_iter()
        .flatten()
    {
        match observe(path)? {
            RetryTargetObservation::Present(ExternalWriterEvidence::Observed(
                CloseWriterObservation::InspectionGap {
                    detector,
                    pid,
                    reason,
                },
            )) => {
                return Err(RetryAssessmentError::WriterInspectionGap(
                    WriterInspectionGap {
                        path: path.to_path_buf(),
                        detector,
                        reason,
                        pid,
                    },
                ))
            }
            RetryTargetObservation::Present(ExternalWriterEvidence::Observed(
                CloseWriterObservation::Authority { .. },
            )) => {
                return Err(format!(
                    "external writer holds exact retry target {}; no new run admitted",
                    path.display()
                )
                .into())
            }
            RetryTargetObservation::Absent
            | RetryTargetObservation::Present(ExternalWriterEvidence::NoPositiveEvidence) => {}
        }
    }
    Ok(())
}

fn probe_retry_worktree_writers(
    original: &Path,
    quarantine: &Path,
    tombstone: Option<&CloseWorktreeFinalTombstone>,
    #[cfg(test)] clean_writer_fixture: bool,
) -> Result<(), RetryAssessmentError> {
    probe_retry_worktree_writers_with(original, quarantine, tombstone, |path| {
        if !path
            .try_exists()
            .map_err(|error| format!("cannot probe retry writer target: {error}"))?
        {
            return Ok(RetryTargetObservation::Absent);
        }
        // Match the existing per-manager deletion-checkpoint fixture. The
        // runner's ambient /proc access is not evidence about this isolated
        // Git fixture; production and non-opted-in tests retain the detector.
        #[cfg(test)]
        if clean_writer_fixture {
            return Ok(RetryTargetObservation::Present(
                ExternalWriterEvidence::NoPositiveEvidence,
            ));
        }
        Ok(RetryTargetObservation::Present(
            quarantine_has_external_writer(path)?,
        ))
    })
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

fn quarantine_has_external_writer_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_external_writer_until_with_self_descriptor(path, deadline, None)
}

fn quarantine_has_external_writer_until_with_self_descriptor(
    path: &Path,
    deadline: std::time::Instant,
    self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<ExternalWriterEvidence, String> {
    scan_writer_detectors(
        deadline,
        || {
            quarantine_has_open_descriptors_until_with_self_descriptor(
                path,
                deadline,
                self_descriptor,
            )
        },
        || quarantine_has_process_cwd_until(path, deadline),
        || quarantine_has_writable_mappings_until(path, deadline),
    )
}

fn scan_writer_detectors(
    deadline: std::time::Instant,
    mut descriptor: impl FnMut() -> Result<ExternalWriterEvidence, String>,
    mut cwd: impl FnMut() -> Result<ExternalWriterEvidence, String>,
    mut mapping: impl FnMut() -> Result<ExternalWriterEvidence, String>,
) -> Result<ExternalWriterEvidence, String> {
    type Probe<'a> = dyn FnMut() -> Result<ExternalWriterEvidence, String> + 'a;
    let mut incomplete = ExternalWriterEvidence::NoPositiveEvidence;
    let probes: [&mut Probe<'_>; 3] = [&mut descriptor, &mut cwd, &mut mapping];
    for probe in probes {
        check_directory_deletion_deadline(Some(deadline))?;
        let evidence = probe()?;
        check_directory_deletion_deadline(Some(deadline))?;
        if evidence.is_positive() {
            return Ok(evidence);
        }
        incomplete.remember_gap(evidence);
    }
    Ok(incomplete)
}

fn quarantine_has_external_writer(path: &Path) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_external_writer_until(
        path,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
}

#[cfg(all(test, target_os = "linux"))]
fn quarantine_has_writable_mappings_in(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_writable_mappings_in_until(
        path,
        proc_root,
        effective_uid,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
}

#[cfg(target_os = "linux")]
fn quarantine_has_writable_mappings_in_until(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    linux_writer_scan_until(
        path,
        proc_root,
        effective_uid,
        CloseWriterDetector::LinuxMapping,
        deadline,
        |process, canonical| {
            writer_scan_check(deadline)?;
            let mappings_result = std::fs::read(process.join("maps"));
            writer_scan_check(deadline)?;
            let mappings = mappings_result.map_err(|_| CloseWriterGap::Unavailable)?;
            if mappings.is_empty() {
                return Ok(None);
            }
            let mappings = mappings.strip_suffix(b"\n").unwrap_or(&mappings);
            scan_writer_entries(mappings.split(|byte| *byte == b'\n'), deadline, |mapping| {
                use std::os::unix::ffi::OsStrExt;

                let mut remainder = mapping;
                let mut permissions = &[][..];
                for index in 0..5 {
                    remainder = remainder.trim_ascii_start();
                    if remainder.is_empty() {
                        return Err(CloseWriterGap::Malformed.into());
                    }
                    let end = remainder
                        .iter()
                        .position(u8::is_ascii_whitespace)
                        .unwrap_or(remainder.len());
                    if index == 1 {
                        permissions = &remainder[..end];
                    }
                    remainder = &remainder[end..];
                }
                let mapped_path =
                    PathBuf::from(std::ffi::OsStr::from_bytes(remainder.trim_ascii_start()));
                if permissions.len() != 4 {
                    return Err(CloseWriterGap::Malformed.into());
                }
                if permissions[1] == b'w'
                    && permissions[3] == b's'
                    && path_is_within(&mapped_path, canonical)
                {
                    return Ok(Some((
                        mapped_path,
                        CloseWriterAuthority::WritableSharedMapping,
                    )));
                }
                Ok(None)
            })
        },
    )
}

#[cfg(target_os = "macos")]
fn quarantine_has_writable_mappings_mac_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
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
    mac_writer_scan_until(
        path,
        CloseWriterDetector::MacMapping,
        deadline,
        |pid, canonical| {
            let mut address = 0_u64;
            let mut incomplete = None;
            loop {
                writer_scan_check(deadline)?;
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
                writer_scan_check(deadline)?;
                if bytes == 0
                    && address > 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL)
                {
                    return incomplete.map_or(Ok(None), |reason| Err(WriterScanError::Gap(reason)));
                }
                if bytes == 0 {
                    return Err(CloseWriterGap::Unavailable.into());
                }
                if usize::try_from(bytes).ok() != Some(size_of::<ProcRegionWithPathInfo>()) {
                    return Err(CloseWriterGap::Malformed.into());
                }
                // SAFETY: the complete C structure was initialized by the successful call.
                let info = unsafe { info.assume_init() };
                let next = info
                    .region
                    .address
                    .checked_add(info.region.size)
                    .ok_or(CloseWriterGap::Malformed)?;
                if next <= address {
                    return Err(CloseWriterGap::Malformed.into());
                }
                address = next;
                let path_bytes = info.vnode.vip_path.as_flattened();
                // SAFETY: reinterpret the initialized, bounded C character buffer as bytes.
                let path_bytes = unsafe {
                    std::slice::from_raw_parts(path_bytes.as_ptr().cast::<u8>(), path_bytes.len())
                };
                let Ok(mapped_path) = CStr::from_bytes_until_nul(path_bytes) else {
                    incomplete = Some(CloseWriterGap::Malformed);
                    continue;
                };
                let mapped_path =
                    PathBuf::from(std::ffi::OsStr::from_bytes(mapped_path.to_bytes()));
                if mac_mapping_is_writer(info.region.protection, info.region.share_mode)
                    && path_is_within(&mapped_path, canonical)
                {
                    return Ok(Some((
                        mapped_path,
                        CloseWriterAuthority::WritableSharedMapping,
                    )));
                }
            }
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_writable_mappings(_path: &Path) -> Result<ExternalWriterEvidence, String> {
    Err("writable memory-mapping inspection is unsupported on this platform".to_string())
}

#[cfg(all(test, target_os = "linux"))]
fn quarantine_has_process_cwd_in(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_process_cwd_in_until(
        path,
        proc_root,
        effective_uid,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
}

#[cfg(target_os = "linux")]
fn quarantine_has_process_cwd_in_until(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    linux_writer_scan_until(
        path,
        proc_root,
        effective_uid,
        CloseWriterDetector::LinuxCwd,
        deadline,
        |process, canonical| {
            writer_scan_check(deadline)?;
            let cwd_result = std::fs::read_link(process.join("cwd"));
            writer_scan_check(deadline)?;
            let cwd = cwd_result.map_err(|_| CloseWriterGap::Unavailable)?;
            let repeated_result = std::fs::read_link(process.join("cwd"));
            writer_scan_check(deadline)?;
            let repeated = repeated_result.map_err(|_| CloseWriterGap::Unavailable)?;
            if cwd != repeated {
                return Err(CloseWriterGap::ProcessChanged.into());
            }
            Ok(path_is_within(&cwd, canonical)
                .then_some((cwd, CloseWriterAuthority::WorkingDirectory)))
        },
    )
}

#[cfg(target_os = "macos")]
fn macos_uid_pids_until(
    effective_uid: libc::uid_t,
    deadline: std::time::Instant,
) -> Result<Vec<i32>, WriterScanError> {
    macos_uid_pids_with_until(
        effective_uid,
        |selector, uid, buffer, buffer_bytes| unsafe {
            libc::proc_listpids(selector, uid, buffer, buffer_bytes)
        },
        deadline,
    )
}

#[cfg(test)]
fn macos_uid_pids_with(
    effective_uid: libc::uid_t,
    mut list: impl FnMut(u32, u32, *mut libc::c_void, i32) -> i32,
) -> Option<Vec<i32>> {
    macos_uid_pids_with_until(
        effective_uid,
        &mut list,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .ok()
}

#[cfg(any(target_os = "macos", test))]
fn macos_uid_pids_with_until(
    effective_uid: libc::uid_t,
    mut list: impl FnMut(u32, u32, *mut libc::c_void, i32) -> i32,
    deadline: std::time::Instant,
) -> Result<Vec<i32>, WriterScanError> {
    use std::mem::size_of;

    // XNU's PROC_UID_ONLY selector restricts the inventory before per-PID
    // inspection, avoiding EPERM on unrelated users' live processes.
    const PROC_UID_ONLY: u32 = 4;
    let mut capacity = 4096_usize;
    loop {
        writer_scan_check(deadline)?;
        let mut pids = vec![0_i32; capacity];
        let capacity_bytes = capacity
            .checked_mul(size_of::<i32>())
            .ok_or(WriterScanError::Gap(CloseWriterGap::Unavailable))?;
        let capacity_bytes_i32 = i32::try_from(capacity_bytes)
            .map_err(|_| WriterScanError::Gap(CloseWriterGap::Unavailable))?;
        writer_scan_check(deadline)?;
        let pid_bytes = list(
            PROC_UID_ONLY,
            effective_uid,
            pids.as_mut_ptr().cast(),
            capacity_bytes_i32,
        );
        writer_scan_check(deadline)?;
        if pid_bytes <= 0 {
            return Err(WriterScanError::Gap(CloseWriterGap::Unavailable));
        }
        let pid_bytes = usize::try_from(pid_bytes)
            .map_err(|_| WriterScanError::Gap(CloseWriterGap::Malformed))?;
        if pid_bytes < capacity_bytes {
            if !pid_bytes.is_multiple_of(size_of::<i32>()) {
                return Err(WriterScanError::Gap(CloseWriterGap::Malformed));
            }
            pids.truncate(pid_bytes / size_of::<i32>());
            return Ok(pids);
        }
        capacity = capacity
            .checked_mul(2)
            .filter(|capacity| *capacity <= 1_048_576)
            .ok_or(WriterScanError::Gap(CloseWriterGap::Unavailable))?;
    }
}

#[cfg(target_os = "macos")]
fn quarantine_has_process_cwd_mac_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};
    use std::os::unix::ffi::OsStrExt as _;

    mac_writer_scan_until(
        path,
        CloseWriterDetector::MacCwd,
        deadline,
        |pid, canonical| {
            writer_scan_check(deadline)?;
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
            writer_scan_check(deadline)?;
            if bytes
                != i32::try_from(size_of::<libc::proc_vnodepathinfo>())
                    .expect("vnode path info size fits i32")
            {
                return Err(CloseWriterGap::Unavailable.into());
            }
            // SAFETY: the complete C structure was initialized above.
            let info = unsafe { info.assume_init() };
            let cwd_bytes = info.pvi_cdir.vip_path.as_flattened();
            // SAFETY: reinterpret the initialized bounded C path buffer as bytes.
            let cwd_bytes = unsafe {
                std::slice::from_raw_parts(cwd_bytes.as_ptr().cast::<u8>(), cwd_bytes.len())
            };
            let cwd =
                CStr::from_bytes_until_nul(cwd_bytes).map_err(|_| CloseWriterGap::Malformed)?;
            let cwd = PathBuf::from(std::ffi::OsStr::from_bytes(cwd.to_bytes()));
            Ok(path_is_within(&cwd, canonical)
                .then_some((cwd, CloseWriterAuthority::WorkingDirectory)))
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_process_cwd(_path: &Path) -> Result<ExternalWriterEvidence, String> {
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
    Zombie,
    Vanished,
}

#[cfg(target_os = "linux")]
fn linux_process_is_zombie(status: &str) -> Result<bool, String> {
    let mut fields = status
        .lines()
        .filter_map(|line| line.strip_prefix("State:"));
    let state = fields.next().ok_or("status State field is missing")?;
    if fields.next().is_some() {
        return Err("status State field is ambiguous".into());
    }
    match state.split_whitespace().next() {
        Some("Z") => Ok(true),
        Some("R" | "S" | "D" | "T" | "t" | "X" | "x" | "K" | "W" | "P" | "I") => Ok(false),
        _ => Err("status State field is malformed or unsupported".into()),
    }
}

#[cfg(target_os = "linux")]
fn linux_writer_io_diagnostic(operation: &str, path: &Path, error: &std::io::Error) {
    tracing::debug!(
        operation,
        path = %path.display(),
        error_kind = ?error.kind(),
        raw_os_error = ?error.raw_os_error(),
        "Linux Close writer scan I/O failed"
    );
}

#[cfg(target_os = "linux")]
fn linux_zombie_has_no_sibling_threads_until(
    process: &Path,
    status: &str,
    deadline: std::time::Instant,
) -> Result<(), WriterScanError> {
    writer_scan_check(deadline)?;
    let validate = |sample: &str| -> Result<(), WriterScanError> {
        let mut counts = sample
            .lines()
            .filter_map(|line| line.strip_prefix("Threads:"));
        if counts.next().map(str::trim) != Some("1")
            || counts.next().is_some()
            || !linux_process_is_zombie(sample).map_err(|error| {
                tracing::debug!(operation = "zombie_state_parse", path = %process.display(), %error);
                CloseWriterGap::Unavailable
            })?
        {
            tracing::debug!(operation = "zombie_single_thread_validate", path = %process.display());
            return Err(CloseWriterGap::Unavailable.into());
        }
        Ok(())
    };
    validate(status)?;
    // A zombie group leader can coexist with live sibling threads. Only exclude
    // the single-task group, never infer absence from the leader's state alone.
    writer_scan_check(deadline)?;
    let inventory = std::fs::read_dir(process.join("task"));
    writer_scan_check(deadline)?;
    let mut tasks = inventory.map_err(|error| {
        linux_writer_io_diagnostic("zombie_task_read_dir", &process.join("task"), &error);
        CloseWriterGap::Unavailable
    })?;
    writer_scan_check(deadline)?;
    let task_result = tasks.next();
    writer_scan_check(deadline)?;
    let task = task_result
        .ok_or_else(|| {
            tracing::debug!(operation = "zombie_task_inventory_empty", path = %process.display());
            CloseWriterGap::Unavailable
        })?
        .map_err(|error| {
            linux_writer_io_diagnostic("zombie_task_entry", &process.join("task"), &error);
            CloseWriterGap::Unavailable
        })?;
    if Some(task.file_name().as_os_str()) != process.file_name() {
        tracing::debug!(
            operation = "zombie_task_identity",
            path = %process.display(),
            task = ?task.file_name()
        );
        return Err(CloseWriterGap::Unavailable.into());
    }
    writer_scan_check(deadline)?;
    let sibling = tasks.next();
    writer_scan_check(deadline)?;
    if sibling.is_some() {
        if let Some(Err(error)) = &sibling {
            linux_writer_io_diagnostic("zombie_sibling_entry", &process.join("task"), error);
        }
        tracing::debug!(operation = "zombie_sibling_present", path = %process.display());
        return Err(CloseWriterGap::Unavailable.into());
    }
    writer_scan_check(deadline)?;
    let after_result = std::fs::read_to_string(process.join("status"));
    writer_scan_check(deadline)?;
    let after = after_result.map_err(|error| {
        linux_writer_io_diagnostic("zombie_status_reread", &process.join("status"), &error);
        CloseWriterGap::Unavailable
    })?;
    validate(&after)?;
    if LinuxProcessEffectiveUid::parse_status(status).map_err(|error| {
        tracing::debug!(operation = "zombie_uid_parse_before", path = %process.display(), %error);
        CloseWriterGap::Unavailable
    })? != LinuxProcessEffectiveUid::parse_status(&after).map_err(|error| {
        tracing::debug!(operation = "zombie_uid_parse_after", path = %process.display(), %error);
        CloseWriterGap::Unavailable
    })? {
        tracing::debug!(operation = "zombie_uid_changed", path = %process.display());
        return Err(CloseWriterGap::Unavailable.into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_process_owner_until(
    process: &std::fs::DirEntry,
    effective_uid: libc::uid_t,
    inventory: &str,
    deadline: std::time::Instant,
) -> Result<LinuxProcessOwner, WriterScanError> {
    writer_scan_check(deadline)?;
    let status_result = std::fs::read_to_string(process.path().join("status"));
    writer_scan_check(deadline)?;
    let status = match status_result {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            linux_writer_io_diagnostic("owner_status_read", &process.path().join("status"), &error);
            return Ok(LinuxProcessOwner::Vanished);
        }
        Err(error) => {
            linux_writer_io_diagnostic("owner_status_read", &process.path().join("status"), &error);
            tracing::debug!(
                "cannot attribute process {} {inventory} inventory from kernel credentials: {error}",
                process.file_name().to_string_lossy()
            );
            return Err(CloseWriterGap::Unavailable.into());
        }
    };
    let process_effective_uid =
        LinuxProcessEffectiveUid::parse_status(&status).map_err(|error| {
            tracing::debug!(
                operation = "owner_uid_parse",
                path = %process.path().join("status").display(),
                "cannot attribute process {} {inventory} inventory from kernel credentials: {error}",
                process.file_name().to_string_lossy()
            );
            CloseWriterGap::Unavailable
        })?;

    // The scanned proc files use `PTRACE_MODE_READ_FSCREDS`: Linux compares the
    // scanner's filesystem UID with the target's real, effective, and saved UIDs.
    // Effective UID is Phoenix's process-ownership boundary; unlike proc-dir inode
    // ownership, it is not rewritten to root when a same-user target is nondumpable.
    Ok(
        if process_effective_uid == LinuxProcessEffectiveUid(effective_uid) {
            // Exclude only a positively single-task zombie, not a zombie leader
            // whose live sibling threads may still retain shared write authority.
            if linux_process_is_zombie(&status).map_err(|error| {
                tracing::debug!(
                    operation = "owner_state_parse",
                    path = %process.path().join("status").display(),
                    inventory,
                    ?process_effective_uid,
                    %error
                );
                CloseWriterGap::Unavailable
            })? {
                linux_zombie_has_no_sibling_threads_until(&process.path(), &status, deadline)?;
                LinuxProcessOwner::Zombie
            } else {
                LinuxProcessOwner::Relevant
            }
        } else {
            LinuxProcessOwner::Unrelated
        },
    )
}

#[cfg(target_os = "linux")]
fn linux_writer_process_until(
    process: &Path,
    boot_id: &str,
    deadline: std::time::Instant,
) -> Result<CloseWriterProcess, WriterScanError> {
    writer_scan_check(deadline)?;
    let pid = process
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse().ok())
        .ok_or_else(|| {
            tracing::debug!(operation = "process_pid_parse", path = %process.display());
            CloseWriterGap::Malformed
        })?;
    writer_scan_check(deadline)?;
    let stat_result = std::fs::read_to_string(process.join("stat"));
    writer_scan_check(deadline)?;
    let stat = stat_result.map_err(|error| {
        linux_writer_io_diagnostic("process_stat_read", &process.join("stat"), &error);
        CloseWriterGap::Unavailable
    })?;
    let (_, tail) = stat.rsplit_once(')').ok_or_else(|| {
        tracing::debug!(operation = "process_stat_tail_parse", pid, path = %process.display());
        CloseWriterGap::Malformed
    })?;
    let start_ticks = tail
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| {
            tracing::debug!(operation = "process_start_ticks_missing", pid, path = %process.display());
            CloseWriterGap::Malformed
        })?
        .parse::<u64>()
        .map_err(|error| {
            tracing::debug!(
                operation = "process_start_ticks_parse",
                pid,
                path = %process.display(),
                %error
            );
            CloseWriterGap::Malformed
        })?;
    writer_scan_check(deadline)?;
    let executable_result = std::fs::read_link(process.join("exe"));
    writer_scan_check(deadline)?;
    let executable = executable_result.map_err(|error| {
        linux_writer_io_diagnostic("process_exe_read_link", &process.join("exe"), &error);
        tracing::debug!(operation = "process_identity_incomplete", pid, start_ticks);
        CloseWriterGap::Unavailable
    })?;
    Ok(CloseWriterProcess {
        pid,
        incarnation: OpaqueIdentity::parse(format!("{boot_id}:{start_ticks}")).map_err(|_| {
            tracing::debug!(operation = "process_incarnation_parse", pid, start_ticks);
            CloseWriterGap::Malformed
        })?,
        executable,
    })
}

#[cfg(all(test, target_os = "linux"))]
fn linux_writer_scan(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
    detector: CloseWriterDetector,
    mut inspect: impl FnMut(
        &Path,
        &Path,
    ) -> Result<Option<(PathBuf, CloseWriterAuthority)>, CloseWriterGap>,
) -> Result<ExternalWriterEvidence, String> {
    linux_writer_scan_until(
        path,
        proc_root,
        effective_uid,
        detector,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
        |process, canonical| inspect(process, canonical).map_err(WriterScanError::from),
    )
}

#[cfg(target_os = "linux")]
fn linux_writer_scan_until(
    path: &Path,
    proc_root: &Path,
    effective_uid: libc::uid_t,
    detector: CloseWriterDetector,
    deadline: std::time::Instant,
    mut inspect: impl FnMut(
        &Path,
        &Path,
    ) -> Result<Option<(PathBuf, CloseWriterAuthority)>, WriterScanError>,
) -> Result<ExternalWriterEvidence, String> {
    let canonical = linux_writer_target_until(path, deadline)?;
    let (boot_id, mut processes) = match linux_writer_inventory_until(proc_root, deadline) {
        Ok(inventory) => inventory,
        Err(WriterScanError::Stopped(message)) => return Err(message),
        Err(WriterScanError::Gap(reason)) => return Ok(writer_gap(detector, None, reason)),
    };
    let mut incomplete = ExternalWriterEvidence::NoPositiveEvidence;
    loop {
        check_directory_deletion_deadline(Some(deadline))?;
        let next = processes.next();
        check_directory_deletion_deadline(Some(deadline))?;
        let Some(process_result) = next else {
            break;
        };
        let process = match process_result {
            Ok(process) => process,
            Err(error) => {
                linux_writer_io_diagnostic("process_inventory_entry", proc_root, &error);
                incomplete.remember_gap(writer_gap(detector, None, CloseWriterGap::Unavailable));
                continue;
            }
        };
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        match linux_writer_candidate_until(&process, effective_uid, detector, pid, deadline) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(WriterScanError::Stopped(message)) => return Err(message),
            Err(WriterScanError::Gap(reason)) => {
                incomplete.remember_gap(writer_gap(detector, Some(pid), reason));
                continue;
            }
        }
        let before = match linux_writer_identity_before_until(
            &process.path(),
            boot_id.trim(),
            detector,
            pid,
            deadline,
        ) {
            Ok(identity) => identity,
            Err(WriterScanError::Stopped(message)) => return Err(message),
            Err(WriterScanError::Gap(reason)) => {
                incomplete.remember_gap(writer_gap(detector, Some(pid), reason));
                continue;
            }
        };
        let observed_result = inspect(&process.path(), &canonical);
        check_directory_deletion_deadline(Some(deadline))?;
        let observed = match observed_result {
            Ok(observed) => observed,
            Err(WriterScanError::Stopped(message)) => return Err(message),
            Err(WriterScanError::Gap(reason)) => {
                tracing::debug!(
                    operation = "inspect_process",
                    detector = detector.as_str(),
                    pid,
                    ?before,
                    ?reason
                );
                incomplete.remember_gap(writer_gap(detector, Some(pid), reason));
                continue;
            }
        };
        if !linux_writer_process_still_matches_until(
            &process,
            boot_id.trim(),
            effective_uid,
            detector,
            deadline,
            &before,
        )? {
            incomplete.remember_gap(writer_gap(
                detector,
                Some(pid),
                CloseWriterGap::ProcessChanged,
            ));
            continue;
        }
        if let Some((matched_path, authority)) = observed {
            check_directory_deletion_deadline(Some(deadline))?;
            return Ok(ExternalWriterEvidence::Observed(
                CloseWriterObservation::Authority {
                    detector,
                    process: before,
                    matched_path,
                    authority,
                },
            ));
        }
    }
    check_directory_deletion_deadline(Some(deadline))?;
    Ok(incomplete)
}

#[cfg(target_os = "linux")]
fn linux_writer_candidate_until(
    process: &std::fs::DirEntry,
    effective_uid: libc::uid_t,
    detector: CloseWriterDetector,
    pid: u32,
    deadline: std::time::Instant,
) -> Result<bool, WriterScanError> {
    let owner = linux_process_owner_until(process, effective_uid, detector.as_str(), deadline);
    writer_scan_check(deadline)?;
    match owner {
        Ok(LinuxProcessOwner::Relevant) => Ok(true),
        Ok(LinuxProcessOwner::Unrelated | LinuxProcessOwner::Zombie) => Ok(false),
        Err(WriterScanError::Stopped(message)) => Err(WriterScanError::Stopped(message)),
        result => {
            tracing::debug!(
                operation = "owner_before",
                detector = detector.as_str(),
                pid,
                ?result
            );
            Err(WriterScanError::Gap(CloseWriterGap::Unavailable))
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_writer_identity_before_until(
    process: &Path,
    boot_id: &str,
    detector: CloseWriterDetector,
    pid: u32,
    deadline: std::time::Instant,
) -> Result<CloseWriterProcess, WriterScanError> {
    let before_result = linux_writer_process_until(process, boot_id, deadline);
    writer_scan_check(deadline)?;
    if let Err(WriterScanError::Gap(reason)) = &before_result {
        tracing::debug!(
            operation = "identity_before",
            detector = detector.as_str(),
            pid,
            ?reason
        );
    }
    before_result
}

#[cfg(target_os = "linux")]
fn linux_writer_target_until(path: &Path, deadline: std::time::Instant) -> Result<PathBuf, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let canonical_result = path.canonicalize();
    check_directory_deletion_deadline(Some(deadline))?;
    let canonical =
        canonical_result.map_err(|error| format!("cannot bind Close writer target: {error}"))?;
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn linux_writer_inventory_until(
    proc_root: &Path,
    deadline: std::time::Instant,
) -> Result<(String, std::fs::ReadDir), WriterScanError> {
    writer_scan_check(deadline)?;
    let boot_result = std::fs::read_to_string(proc_root.join("sys/kernel/random/boot_id"));
    writer_scan_check(deadline)?;
    let boot_id = match boot_result {
        Ok(boot_id) => boot_id,
        Err(error) => {
            linux_writer_io_diagnostic(
                "boot_id_read",
                &proc_root.join("sys/kernel/random/boot_id"),
                &error,
            );
            return Err(CloseWriterGap::Unavailable.into());
        }
    };
    if boot_id.trim().is_empty() {
        tracing::debug!(operation = "boot_id_empty", path = %proc_root.display());
        return Err(CloseWriterGap::Malformed.into());
    }
    writer_scan_check(deadline)?;
    let inventory = std::fs::read_dir(proc_root);
    writer_scan_check(deadline)?;
    let processes = match inventory {
        Ok(processes) => processes,
        Err(error) => {
            linux_writer_io_diagnostic("process_read_dir", proc_root, &error);
            return Err(CloseWriterGap::Unavailable.into());
        }
    };
    Ok((boot_id, processes))
}

#[cfg(target_os = "linux")]
fn linux_writer_process_still_matches_until(
    process: &std::fs::DirEntry,
    boot_id: &str,
    effective_uid: libc::uid_t,
    detector: CloseWriterDetector,
    deadline: std::time::Instant,
    before: &CloseWriterProcess,
) -> Result<bool, String> {
    let after = linux_writer_process_until(&process.path(), boot_id, deadline);
    check_directory_deletion_deadline(Some(deadline))?;
    let after = match after {
        Err(WriterScanError::Stopped(message)) => return Err(message),
        result => result,
    };
    let owner_after =
        linux_process_owner_until(process, effective_uid, detector.as_str(), deadline);
    check_directory_deletion_deadline(Some(deadline))?;
    let owner_after = match owner_after {
        Err(WriterScanError::Stopped(message)) => return Err(message),
        result => result,
    };
    if !matches!(after, Ok(ref identity) if identity == before)
        || !matches!(owner_after, Ok(LinuxProcessOwner::Relevant))
    {
        tracing::debug!(
            operation = "process_revalidation",
            detector = detector.as_str(),
            pid = before.pid,
            ?before,
            ?after,
            ?owner_after
        );
        return Ok(false);
    }
    Ok(true)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn self_descriptor_borrow_matches(
    borrow: &SelfDeletionDescriptorBorrow<'_>,
    pid: libc::pid_t,
    fd: libc::c_int,
    device: u64,
    inode: u64,
    kind: libc::mode_t,
) -> bool {
    pid == borrow.pid
        && fd == borrow.fd
        && device == borrow.device
        && inode == borrow.inode
        && kind == borrow.kind
        && borrow.witness.validate().is_ok()
}

fn descriptor_authority(
    is_directory: bool,
    is_regular_file: bool,
    access: CloseDescriptorAccess,
) -> Option<CloseWriterAuthority> {
    if is_directory {
        return Some(CloseWriterAuthority::DirectoryDescriptor(access));
    }
    if !is_regular_file {
        return None;
    }
    match access {
        CloseDescriptorAccess::ReadOnly => None,
        CloseDescriptorAccess::WriteOnly => Some(CloseWriterAuthority::WritableFile(
            CloseFileWriteAccess::WriteOnly,
        )),
        CloseDescriptorAccess::ReadWrite => Some(CloseWriterAuthority::WritableFile(
            CloseFileWriteAccess::ReadWrite,
        )),
    }
}

#[cfg(target_os = "linux")]
fn linux_fd_access(info: &str) -> Result<CloseDescriptorAccess, CloseWriterGap> {
    let mut flags = info.lines().filter_map(|line| line.strip_prefix("flags:"));
    let value = flags.next().ok_or(CloseWriterGap::Malformed)?;
    if flags.next().is_some() {
        return Err(CloseWriterGap::Malformed);
    }
    let value = u32::from_str_radix(value.trim(), 8).map_err(|_| CloseWriterGap::Malformed)?;
    match value & 3 {
        0 => Ok(CloseDescriptorAccess::ReadOnly),
        1 => Ok(CloseDescriptorAccess::WriteOnly),
        2 => Ok(CloseDescriptorAccess::ReadWrite),
        _ => Err(CloseWriterGap::Malformed),
    }
}

#[cfg(all(test, target_os = "linux"))]
fn quarantine_has_open_descriptors_in(
    path: &Path,
    proc_root: &Path,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_with_info_reader(path, proc_root, |path| {
        std::fs::read_to_string(path)
    })
}

#[cfg(all(test, target_os = "linux"))]
fn quarantine_has_open_descriptors_with_info_reader(
    path: &Path,
    proc_root: &Path,
    read_info: impl FnMut(&Path) -> std::io::Result<String>,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_with_info_reader_until(
        path,
        proc_root,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
        read_info,
    )
}

#[cfg(all(test, target_os = "linux"))]
fn quarantine_has_open_descriptors_with_info_reader_until(
    path: &Path,
    proc_root: &Path,
    deadline: std::time::Instant,
    read_info: impl FnMut(&Path) -> std::io::Result<String>,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_with_info_reader_until_scoped(
        path, proc_root, deadline, read_info, None,
    )
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors_with_info_reader_until_scoped(
    path: &Path,
    proc_root: &Path,
    deadline: std::time::Instant,
    mut read_info: impl FnMut(&Path) -> std::io::Result<String>,
    self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<ExternalWriterEvidence, String> {
    // SAFETY: geteuid has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    linux_writer_scan_until(
        path,
        proc_root,
        effective_uid,
        CloseWriterDetector::LinuxDescriptor,
        deadline,
        |process, canonical| {
            writer_scan_check(deadline)?;
            let inventory = std::fs::read_dir(process.join("fd"));
            writer_scan_check(deadline)?;
            let descriptors = inventory.map_err(|error| {
                linux_writer_io_diagnostic("fd_read_dir", &process.join("fd"), &error);
                CloseWriterGap::Unavailable
            })?;
            scan_writer_entries(descriptors, deadline, |descriptor_result| {
                let descriptor = descriptor_result.map_err(|error| {
                    linux_writer_io_diagnostic("fd_inventory_entry", &process.join("fd"), &error);
                    CloseWriterGap::Unavailable
                })?;
                writer_scan_check(deadline)?;
                let candidate_result = std::fs::read_link(descriptor.path());
                writer_scan_check(deadline)?;
                let candidate = candidate_result.map_err(|error| {
                    linux_writer_io_diagnostic("fd_read_link", &descriptor.path(), &error);
                    CloseWriterGap::DescriptorChanged
                })?;
                if !path_is_within(&candidate, canonical) {
                    return Ok(None);
                }
                writer_scan_check(deadline)?;
                let metadata_result = std::fs::metadata(descriptor.path());
                writer_scan_check(deadline)?;
                let metadata = metadata_result.map_err(|error| {
                    linux_writer_io_diagnostic("fd_metadata", &descriptor.path(), &error);
                    CloseWriterGap::DescriptorChanged
                })?;
                let info_path = process.join("fdinfo").join(descriptor.file_name());
                writer_scan_check(deadline)?;
                let info_result = read_info(&info_path);
                writer_scan_check(deadline)?;
                let info = info_result.map_err(|error| {
                    linux_writer_io_diagnostic("fdinfo_read", &info_path, &error);
                    CloseWriterGap::DescriptorChanged
                })?;
                let access = linux_fd_access(&info).inspect_err(|reason| {
                    tracing::debug!(
                        operation = "fdinfo_access_parse",
                        path = %info_path.display(),
                        ?reason
                    );
                })?;
                revalidate_linux_descriptor_until(
                    &descriptor,
                    &candidate,
                    &metadata,
                    access,
                    &info_path,
                    deadline,
                    &mut read_info,
                )?;
                if let Some(authority) =
                    descriptor_authority(metadata.is_dir(), metadata.is_file(), access)
                {
                    let is_exact_self_borrow = self_descriptor.is_some_and(|borrow| {
                        linux_descriptor_matches_self_borrow(
                            process,
                            &descriptor,
                            &metadata,
                            borrow,
                        )
                    });
                    if is_exact_self_borrow {
                        return Ok(None);
                    }
                    return Ok(Some((candidate, authority)));
                }
                if !metadata.is_file() {
                    tracing::debug!(operation = "fd_type_unsupported", path = %descriptor.path().display());
                    return Err(CloseWriterGap::Unsupported.into());
                }
                Ok(None)
            })
        },
    )
}

#[cfg(target_os = "linux")]
fn revalidate_linux_descriptor_until(
    descriptor: &std::fs::DirEntry,
    candidate: &Path,
    metadata: &std::fs::Metadata,
    access: CloseDescriptorAccess,
    info_path: &Path,
    deadline: std::time::Instant,
    read_info: &mut impl FnMut(&Path) -> std::io::Result<String>,
) -> Result<(), WriterScanError> {
    use std::os::unix::fs::MetadataExt as _;

    writer_scan_check(deadline)?;
    let repeated_result = std::fs::metadata(descriptor.path());
    writer_scan_check(deadline)?;
    let repeated = repeated_result.map_err(|error| {
        linux_writer_io_diagnostic("fd_metadata_repeat", &descriptor.path(), &error);
        CloseWriterGap::DescriptorChanged
    })?;
    writer_scan_check(deadline)?;
    let repeated_link = std::fs::read_link(descriptor.path());
    writer_scan_check(deadline)?;
    let repeated_info = read_info(info_path);
    writer_scan_check(deadline)?;
    let repeated_info = repeated_info.map_err(|error| {
        linux_writer_io_diagnostic("fdinfo_read_repeat", info_path, &error);
        CloseWriterGap::DescriptorChanged
    })?;
    if let Err(error) = &repeated_link {
        linux_writer_io_diagnostic("fd_read_link_repeat", &descriptor.path(), error);
    }
    if (metadata.dev(), metadata.ino()) != (repeated.dev(), repeated.ino())
        || repeated_link.ok().as_deref() != Some(candidate)
        || linux_fd_access(&repeated_info).inspect_err(|reason| {
            tracing::debug!(
                operation = "fdinfo_access_parse_repeat",
                path = %info_path.display(),
                ?reason
            );
        })? != access
    {
        tracing::debug!(operation = "fd_revalidation", path = %descriptor.path().display());
        return Err(CloseWriterGap::DescriptorChanged.into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_descriptor_matches_self_borrow(
    process: &Path,
    descriptor: &std::fs::DirEntry,
    metadata: &std::fs::Metadata,
    borrow: &SelfDeletionDescriptorBorrow<'_>,
) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    let pid = process
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|value| value.parse::<libc::pid_t>().ok());
    let fd = descriptor
        .file_name()
        .to_str()
        .and_then(|value| value.parse::<libc::c_int>().ok());
    pid.zip(fd).is_some_and(|(pid, fd)| {
        self_descriptor_borrow_matches(
            borrow,
            pid,
            fd,
            metadata.dev(),
            metadata.ino(),
            metadata.mode() & libc::S_IFMT,
        )
    })
}

#[cfg(target_os = "macos")]
fn descriptor_inventory_may_be_truncated(returned_bytes: usize, capacity_bytes: usize) -> bool {
    returned_bytes >= capacity_bytes
}

#[cfg(any(target_os = "macos", test))]
fn mac_descriptor_authority(
    mode: u16,
    open_flags: u32,
) -> Result<Option<CloseWriterAuthority>, CloseWriterGap> {
    // proc_fileinfo.fi_openflags uses kernel FREAD/FWRITE, not user O_ACCMODE.
    let access = match open_flags & 3 {
        1 => CloseDescriptorAccess::ReadOnly,
        2 => CloseDescriptorAccess::WriteOnly,
        3 => CloseDescriptorAccess::ReadWrite,
        _ => return Err(CloseWriterGap::Unsupported),
    };
    Ok(descriptor_authority(
        mode & 0o170_000 == 0o040_000,
        mode & 0o170_000 == 0o100_000,
        access,
    ))
}

#[cfg(any(target_os = "macos", test))]
fn mac_mapping_is_writer(protection: u32, share_mode: u32) -> bool {
    protection & 2 != 0 && matches!(share_mode, 4 | 5 | 7)
}

#[cfg(any(target_os = "macos", test))]
fn macos_selected_process_uid(
    observed_uid: libc::uid_t,
    effective_uid: libc::uid_t,
) -> Result<(), CloseWriterGap> {
    if observed_uid == effective_uid {
        Ok(())
    } else {
        Err(CloseWriterGap::ProcessChanged)
    }
}

#[cfg(target_os = "macos")]
fn mac_writer_process_until(
    pid: libc::pid_t,
    effective_uid: libc::uid_t,
    deadline: std::time::Instant,
) -> Result<CloseWriterProcess, WriterScanError> {
    use std::mem::{size_of, MaybeUninit};
    use std::os::unix::ffi::OsStrExt as _;
    writer_scan_check(deadline)?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: output buffer matches PROC_PIDTBSDINFO and is valid for the call.
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            i32::try_from(size_of::<libc::proc_bsdinfo>()).expect("bsd info fits i32"),
        )
    };
    writer_scan_check(deadline)?;
    if usize::try_from(bytes).ok() != Some(size_of::<libc::proc_bsdinfo>()) {
        return Err(CloseWriterGap::Unavailable.into());
    }
    // SAFETY: the complete structure was initialized above.
    let info = unsafe { info.assume_init() };
    macos_selected_process_uid(info.pbi_uid, effective_uid)?;
    let mut executable = [0_u8; 4096];
    writer_scan_check(deadline)?;
    // SAFETY: executable is a writable buffer with the declared length.
    let bytes = unsafe { libc::proc_pidpath(pid, executable.as_mut_ptr().cast(), 4096) };
    writer_scan_check(deadline)?;
    if bytes <= 0 {
        return Err(CloseWriterGap::Unavailable.into());
    }
    let end = executable
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(CloseWriterGap::Malformed)?;
    if end == 0 {
        return Err(CloseWriterGap::Malformed.into());
    }
    writer_scan_check(deadline)?;
    Ok(CloseWriterProcess {
        pid: u32::try_from(pid).map_err(|_| CloseWriterGap::Malformed)?,
        incarnation: OpaqueIdentity::parse(format!(
            "{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        ))
        .map_err(|_| CloseWriterGap::Malformed)?,
        executable: PathBuf::from(std::ffi::OsStr::from_bytes(&executable[..end])),
    })
}

#[cfg(any(target_os = "macos", test))]
fn stable_writer_observation(
    detector: CloseWriterDetector,
    before: CloseWriterProcess,
    observed: Option<(PathBuf, CloseWriterAuthority)>,
    after: Result<Option<CloseWriterProcess>, CloseWriterGap>,
) -> ExternalWriterEvidence {
    if after.ok().flatten().as_ref() != Some(&before) {
        return writer_gap(detector, Some(before.pid), CloseWriterGap::ProcessChanged);
    }
    match observed {
        Some((matched_path, authority)) => {
            ExternalWriterEvidence::Observed(CloseWriterObservation::Authority {
                detector,
                process: before,
                matched_path,
                authority,
            })
        }
        None => ExternalWriterEvidence::NoPositiveEvidence,
    }
}

#[cfg(target_os = "macos")]
fn mac_writer_scan_until(
    path: &Path,
    detector: CloseWriterDetector,
    deadline: std::time::Instant,
    mut inspect: impl FnMut(
        libc::pid_t,
        &Path,
    ) -> Result<Option<(PathBuf, CloseWriterAuthority)>, WriterScanError>,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let canonical_result = path.canonicalize();
    check_directory_deletion_deadline(Some(deadline))?;
    let canonical =
        canonical_result.map_err(|error| format!("cannot bind Close writer target: {error}"))?;
    let effective_uid = unsafe { libc::geteuid() };
    let pids_result = macos_uid_pids_until(effective_uid, deadline);
    check_directory_deletion_deadline(Some(deadline))?;
    let pids = match pids_result {
        Ok(pids) => pids,
        Err(WriterScanError::Stopped(s)) => return Err(s),
        Err(WriterScanError::Gap(reason)) => return Ok(writer_gap(detector, None, reason)),
    };
    let mut incomplete = ExternalWriterEvidence::NoPositiveEvidence;
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        check_directory_deletion_deadline(Some(deadline))?;
        let before_result = mac_writer_process_until(pid, effective_uid, deadline);
        check_directory_deletion_deadline(Some(deadline))?;
        let before = match before_result {
            Ok(identity) => identity,
            Err(WriterScanError::Stopped(message)) => return Err(message),
            Err(WriterScanError::Gap(gap)) => {
                incomplete.remember_gap(writer_gap(detector, u32::try_from(pid).ok(), gap));
                continue;
            }
        };
        let observed_result = inspect(pid, &canonical);
        check_directory_deletion_deadline(Some(deadline))?;
        let observed = match observed_result {
            Ok(v) => v,
            Err(WriterScanError::Stopped(s)) => return Err(s),
            Err(WriterScanError::Gap(g)) => {
                incomplete.remember_gap(writer_gap(detector, Some(before.pid), g));
                continue;
            }
        };
        let after_result = mac_writer_process_until(pid, effective_uid, deadline);
        check_directory_deletion_deadline(Some(deadline))?;
        let after = match after_result {
            Ok(identity) => Ok(Some(identity)),
            Err(WriterScanError::Gap(gap)) => Err(gap),
            Err(WriterScanError::Stopped(message)) => return Err(message),
        };
        let evidence = stable_writer_observation(detector, before, observed, after);
        if evidence.is_positive() {
            check_directory_deletion_deadline(Some(deadline))?;
            return Ok(evidence);
        }
        incomplete.remember_gap(evidence);
    }
    check_directory_deletion_deadline(Some(deadline))?;
    Ok(incomplete)
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct MacProcFileInfo {
    open_flags: u32,
    status: u32,
    offset: i64,
    file_type: i32,
    guard_flags: u32,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct MacVnodeFdInfoWithPath {
    file: MacProcFileInfo,
    vnode: libc::vnode_info_path,
}

#[cfg(target_os = "macos")]
fn mac_vnode_fd_info(
    pid: libc::pid_t,
    fd: i32,
    recheck: bool,
) -> Result<MacVnodeFdInfoWithPath, CloseWriterGap> {
    use std::mem::{size_of, MaybeUninit};

    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    let mut output = MaybeUninit::<MacVnodeFdInfoWithPath>::zeroed();
    let length = i32::try_from(size_of::<MacVnodeFdInfoWithPath>()).expect("vnode info fits i32");
    // SAFETY: output is valid writable storage of the requested C structure size.
    let bytes = unsafe {
        libc::proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDVNODEPATHINFO,
            output.as_mut_ptr().cast(),
            length,
        )
    };
    // libproc returns zero on syscall failure and leaves errno from that call.
    if bytes == 0 {
        let errno = std::io::Error::last_os_error().raw_os_error();
        tracing::warn!(
            pid,
            fd,
            recheck,
            ?errno,
            "Close macOS vnode FD read unavailable"
        );
        return Err(CloseWriterGap::Unavailable);
    }
    if bytes != length {
        tracing::warn!(
            pid,
            fd,
            recheck,
            bytes,
            expected_bytes = length,
            "Close macOS vnode FD read malformed"
        );
        return Err(CloseWriterGap::Malformed);
    }
    // SAFETY: the complete structure was initialized by the successful call.
    Ok(unsafe { output.assume_init() })
}

#[cfg(target_os = "macos")]
fn mac_vnode_descriptors_until(
    pid: libc::pid_t,
    deadline: std::time::Instant,
) -> Result<Vec<libc::proc_fdinfo>, WriterScanError> {
    use std::mem::size_of;

    let mut descriptor_capacity = 256_usize;
    loop {
        writer_scan_check(deadline)?;
        let mut descriptors = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0
            };
            descriptor_capacity
        ];
        let capacity_bytes = descriptors.len() * size_of::<libc::proc_fdinfo>();
        let capacity_bytes_i32 = i32::try_from(capacity_bytes)
            .map_err(|_| WriterScanError::Gap(CloseWriterGap::Unavailable))?;
        writer_scan_check(deadline)?;
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
        writer_scan_check(deadline)?;
        if descriptor_bytes <= 0 {
            return Err(WriterScanError::Gap(CloseWriterGap::Unavailable));
        }
        let descriptor_bytes =
            usize::try_from(descriptor_bytes).expect("positive descriptor byte count");
        if descriptor_bytes % size_of::<libc::proc_fdinfo>() != 0 {
            return Err(WriterScanError::Gap(CloseWriterGap::Malformed));
        }
        if !descriptor_inventory_may_be_truncated(descriptor_bytes, capacity_bytes) {
            descriptors.truncate(descriptor_bytes / size_of::<libc::proc_fdinfo>());
            let mut vnode_descriptors = Vec::new();
            for descriptor in descriptors {
                writer_scan_check(deadline)?;
                if descriptor.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32 {
                    vnode_descriptors.push(descriptor);
                }
            }
            writer_scan_check(deadline)?;
            return Ok(vnode_descriptors);
        }
        descriptor_capacity = descriptor_capacity
            .checked_mul(2)
            .filter(|capacity| *capacity <= 1_048_576)
            .ok_or(WriterScanError::Gap(CloseWriterGap::Unavailable))?;
    }
}

#[cfg(target_os = "macos")]
fn mac_inspect_open_descriptors_until(
    pid: libc::pid_t,
    canonical: &Path,
    deadline: std::time::Instant,
    self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<Option<(PathBuf, CloseWriterAuthority)>, WriterScanError> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt as _;

    let descriptors = mac_vnode_descriptors_until(pid, deadline)?;
    scan_writer_entries(descriptors, deadline, |descriptor| {
        writer_scan_check(deadline)?;
        let info_result = mac_vnode_fd_info(pid, descriptor.proc_fd, false);
        writer_scan_check(deadline)?;
        let info = info_result.map_err(WriterScanError::from)?;
        let path_bytes = info.vnode.vip_path.as_flattened();
        // SAFETY: path_bytes points to the initialized kernel path buffer; bounded below.
        let path_bytes = unsafe {
            std::slice::from_raw_parts(path_bytes.as_ptr().cast::<u8>(), path_bytes.len())
        };
        let candidate =
            CStr::from_bytes_until_nul(path_bytes).map_err(|_| CloseWriterGap::Malformed)?;
        let candidate = PathBuf::from(std::ffi::OsStr::from_bytes(candidate.to_bytes()));
        if !path_is_within(&candidate, canonical) {
            writer_scan_check(deadline)?;
            return Ok(None);
        }
        writer_scan_check(deadline)?;
        let repeated_result = mac_vnode_fd_info(pid, descriptor.proc_fd, true);
        writer_scan_check(deadline)?;
        let repeated = repeated_result.map_err(WriterScanError::from)?;
        let stat = info.vnode.vip_vi.vi_stat;
        let repeated_stat = repeated.vnode.vip_vi.vi_stat;
        let flags_changed = info.file.open_flags != repeated.file.open_flags;
        let device_changed = stat.vst_dev != repeated_stat.vst_dev;
        let inode_changed = stat.vst_ino != repeated_stat.vst_ino;
        let mode_changed = stat.vst_mode != repeated_stat.vst_mode;
        let path_buffer_changed = info.vnode.vip_path != repeated.vnode.vip_path;
        if flags_changed || device_changed || inode_changed || mode_changed || path_buffer_changed {
            writer_scan_check(deadline)?;
            tracing::warn!(
                pid,
                fd = descriptor.proc_fd,
                flags_changed,
                device_changed,
                inode_changed,
                mode_changed,
                path_buffer_changed,
                "Close macOS vnode FD samples differ"
            );
            return Err(WriterScanError::Gap(CloseWriterGap::DescriptorChanged));
        }
        let is_exact_self_borrow = self_descriptor.is_some_and(|borrow| {
            self_descriptor_borrow_matches(
                borrow,
                pid,
                descriptor.proc_fd,
                stat.vst_dev as u64,
                stat.vst_ino,
                stat.vst_mode & libc::S_IFMT,
            )
        });
        if is_exact_self_borrow {
            writer_scan_check(deadline)?;
            return Ok(None);
        }
        if let Some(authority) = mac_descriptor_authority(stat.vst_mode, info.file.open_flags)
            .map_err(WriterScanError::from)?
        {
            writer_scan_check(deadline)?;
            return Ok(Some((candidate, authority)));
        }
        Ok(None)
    })
}

#[cfg(target_os = "macos")]
fn quarantine_has_open_descriptors_mac_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_mac_until_scoped(path, deadline, None)
}

#[cfg(target_os = "macos")]
fn quarantine_has_open_descriptors_mac_until_scoped(
    path: &Path,
    deadline: std::time::Instant,
    self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<ExternalWriterEvidence, String> {
    mac_writer_scan_until(
        path,
        CloseWriterDetector::MacDescriptor,
        deadline,
        |pid, canonical| {
            mac_inspect_open_descriptors_until(pid, canonical, deadline, self_descriptor)
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_open_descriptors(_path: &Path) -> Result<ExternalWriterEvidence, String> {
    Err("open-descriptor inspection is unsupported on this platform".to_string())
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_until_with_self_descriptor(path, deadline, None)
}

#[cfg(target_os = "linux")]
fn quarantine_has_open_descriptors_until_with_self_descriptor(
    path: &Path,
    deadline: std::time::Instant,
    self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let result = quarantine_has_open_descriptors_with_info_reader_until_scoped(
        path,
        Path::new("/proc"),
        deadline,
        |path| std::fs::read_to_string(path),
        self_descriptor,
    );
    check_directory_deletion_deadline(Some(deadline))?;
    result
}

#[cfg(target_os = "macos")]
fn quarantine_has_open_descriptors_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    quarantine_has_open_descriptors_mac_until(path, deadline)
}

#[cfg(target_os = "macos")]
fn quarantine_has_open_descriptors_until_with_self_descriptor(
    path: &Path,
    deadline: std::time::Instant,
    self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    quarantine_has_open_descriptors_mac_until_scoped(path, deadline, self_descriptor)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_open_descriptors_until_with_self_descriptor(
    path: &Path,
    deadline: std::time::Instant,
    _self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
) -> Result<ExternalWriterEvidence, String> {
    quarantine_has_open_descriptors_until(path, deadline)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn quarantine_has_open_descriptors_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    let result = quarantine_has_open_descriptors(path);
    check_directory_deletion_deadline(Some(deadline))?;
    result
}

fn quarantine_has_process_cwd_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    #[cfg(target_os = "linux")]
    let result = {
        let uid = unsafe { libc::geteuid() };
        quarantine_has_process_cwd_in_until(path, Path::new("/proc"), uid, deadline)
    };
    #[cfg(target_os = "macos")]
    let result = quarantine_has_process_cwd_mac_until(path, deadline);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = quarantine_has_process_cwd(path);
    check_directory_deletion_deadline(Some(deadline))?;
    result
}

fn quarantine_has_writable_mappings_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<ExternalWriterEvidence, String> {
    check_directory_deletion_deadline(Some(deadline))?;
    #[cfg(target_os = "linux")]
    let result = {
        let uid = unsafe { libc::geteuid() };
        quarantine_has_writable_mappings_in_until(path, Path::new("/proc"), uid, deadline)
    };
    #[cfg(target_os = "macos")]
    let result = quarantine_has_writable_mappings_mac_until(path, deadline);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = quarantine_has_writable_mappings(path);
    check_directory_deletion_deadline(Some(deadline))?;
    result
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
    if !planned_administrative_dir_is_absent(planned_administrative_dir)? {
        if observe_administrative_dir_incarnation(planned_administrative_dir)?
            != planned_administrative_dir_incarnation
        {
            return Err(
                "persisted cleanup plan administrative-directory incarnation changed".to_string(),
            );
        }
        validate_live_persisted_worktree_registration(identity, planned_administrative_dir)?;
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

pub(super) type SubmoduleSpawnAdmissionEffect = Box<
    dyn FnOnce(
            &mut std::process::Command,
            std::time::Instant,
        ) -> Result<std::process::Child, String>
        + Send,
>;
type StopFsmonitorEffect = Box<
    dyn FnOnce(
            &Path,
            std::time::Instant,
            Option<SubmoduleSpawnAdmissionEffect>,
        ) -> Result<FsmonitorStop, String>
        + Send,
>;
type OpenDescriptorsEffect =
    Box<dyn FnOnce(&Path, std::time::Instant) -> Result<ExternalWriterEvidence, String> + Send>;
type ExternalWriterEffect =
    Box<dyn FnOnce(&Path, std::time::Instant) -> Result<bool, String> + Send>;

/// Host-interacting effects of exact-worktree quarantine and removal, gathered
/// at one narrow adapter so a test can supply deterministic observations instead
/// of launching the real fsmonitor shutdown or scanning the host `/proc`.
///
/// Production always uses [`CloseWorktreeEffects::production`], whose fields are
/// the real functions; there is no test-only global switch. The shared decision
/// logic in [`quarantine_and_remove_exact_worktree_with_repository_lock_until`]
/// is identical whether the real or an injected adapter is supplied.
pub(super) struct CloseWorktreeEffects {
    removal_entry: WorktreeRemovalEntry,
    stop_fsmonitor: StopFsmonitorEffect,
    first_submodule_spawn_admission: Option<SubmoduleSpawnAdmissionEffect>,
    observe_open_descriptors: OpenDescriptorsEffect,
    observe_external_writer: ExternalWriterEffect,
}

enum WorktreeRemovalEntry {
    GenericOriginal(FirstCloseRetirementDispatch),
    RetainedQuarantine,
    AdmittedNativeRetry,
    #[cfg(test)]
    Test,
}

impl CloseWorktreeEffects {
    fn for_admitted_native_retry(probe: CloseWriterProbe) -> Self {
        Self::writer_probes(probe, WorktreeRemovalEntry::AdmittedNativeRetry)
    }

    fn production(
        probe: CloseWriterProbe,
        removal_entry: WorktreeRemovalEntry,
    ) -> Result<Self, String> {
        if let WorktreeRemovalEntry::GenericOriginal(first) = &removal_entry {
            let request = first.request();
            if probe.run.ordinal != CloseRunOrdinal::INITIAL
                || request.attempt_id != probe.run.attempt_id
                || request.scope != probe.scope
                || request.snapshot != probe.snapshot
                || request.resource != probe.resource
            {
                return Err("first dispatch does not match the worktree writer evidence target".to_string());
            }
        }
        Ok(Self::writer_probes(probe, removal_entry))
    }

    fn writer_probes(probe: CloseWriterProbe, removal_entry: WorktreeRemovalEntry) -> Self {
        let descriptor_probe = probe.clone();
        Self {
            removal_entry,
            stop_fsmonitor: Box::new(|path, deadline, admission| {
                stop_bound_fsmonitor_daemons_with_admission_until(path, deadline, admission)
            }),
            first_submodule_spawn_admission: None,
            observe_open_descriptors: Box::new(move |path, deadline| {
                descriptor_probe.observe_until(
                    path,
                    deadline,
                    quarantine_has_open_descriptors_until,
                )
            }),
            observe_external_writer: Box::new(move |path, deadline| {
                Ok(probe
                    .observe_until(path, deadline, quarantine_has_external_writer_until)?
                    .blocks_retirement())
            }),
        }
    }
}

// Forward the existing exact-removal bindings and injected effects without hiding authority.
#[allow(clippy::too_many_arguments)]
async fn quarantine_and_remove_exact_worktree<F, B>(
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
    final_tombstone: Option<CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    effects: CloseWorktreeEffects,
    deadline: std::time::Instant,
    repository_lock: RepositoryMutationLock,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    quarantine_and_remove_exact_worktree_with_repository_lock_until(
        identity,
        confirmed_snapshot,
        planned_administrative_dir,
        planned_administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
        after_quarantine,
        effects,
        deadline,
        Some(repository_lock),
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
async fn quarantine_and_remove_exact_worktree_with_effects<F, B>(
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
    final_tombstone: Option<CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    effects: CloseWorktreeEffects,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    quarantine_and_remove_exact_worktree_with_effects_until(
        identity,
        confirmed_snapshot,
        planned_administrative_dir,
        planned_administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
        after_quarantine,
        effects,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn quarantine_and_remove_exact_worktree_with_effects_until<F, B>(
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
    final_tombstone: Option<CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    effects: CloseWorktreeEffects,
    deadline: std::time::Instant,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    quarantine_and_remove_exact_worktree_with_repository_lock_until(
        identity,
        confirmed_snapshot,
        planned_administrative_dir,
        planned_administrative_dir_incarnation,
        final_tombstone,
        bind_tombstone,
        after_quarantine,
        effects,
        deadline,
        None,
    )
    .await
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn quarantine_and_remove_exact_worktree_with_repository_lock_until<F, B>(
    identity: &WorktreeIdentity,
    confirmed_snapshot: &CloseRetirementSnapshot,
    planned_administrative_dir: PathBuf,
    planned_administrative_dir_incarnation: String,
    final_tombstone: Option<CloseWorktreeFinalTombstone>,
    bind_tombstone: B,
    after_quarantine: F,
    effects: CloseWorktreeEffects,
    deadline: std::time::Instant,
    repository_lock: Option<RepositoryMutationLock>,
) -> Result<ExactWorktreeRemoval, String>
where
    F: FnOnce(&Path) + Send + 'static,
    B: FnMut(&Path, (u64, u64), Option<(u64, u64)>) -> Result<(), String> + Send + 'static,
{
    check_directory_deletion_deadline(Some(deadline))?;
    let CloseWorktreeEffects {
        removal_entry,
        stop_fsmonitor,
        first_submodule_spawn_admission,
        observe_open_descriptors,
        observe_external_writer,
    } = effects;
    let path = worktree_path(identity);
    let quarantine = worktree_quarantine_path(identity)?;
    let expected = identity.fingerprint().as_str().to_string();
    let inspection_identity = identity.clone();
    let confirmed_fingerprint = confirmed_snapshot.fingerprint().to_string();
    tokio::task::spawn_blocking(move || {
        let _repository_lock = repository_lock;
        let original_exists = path
            .try_exists()
            .map_err(|error| format!("cannot observe captured worktree: {error}"))?;
        let quarantine_exists = quarantine
            .try_exists()
            .map_err(|error| format!("cannot observe quarantined worktree: {error}"))?;
        let resuming_quarantine = !original_exists && quarantine_exists;
        if !original_exists && !resuming_quarantine {
            return Err("captured worktree path is absent without an exact prior receipt".to_string());
        }
        match removal_entry {
            WorktreeRemovalEntry::GenericOriginal(first) => {
                let dispatch = first.into_request();
                if !original_exists
                    || quarantine_exists
                    || final_tombstone.is_some()
                    || dispatch.resource.identity() != &LossItemIdentity::Worktree(inspection_identity.clone())
                {
                    return Err("first dispatch does not match the present original worktree".to_string());
                }
            }
            WorktreeRemovalEntry::RetainedQuarantine if !resuming_quarantine => {
                return Err("quarantine continuation cannot enter the original worktree".to_string());
            }
            WorktreeRemovalEntry::RetainedQuarantine | WorktreeRemovalEntry::AdmittedNativeRetry => {}
            #[cfg(test)]
            WorktreeRemovalEntry::Test => {}
        }
        let inspection_path = if resuming_quarantine {
            &quarantine
        } else {
            &path
        };
        check_directory_deletion_deadline(Some(deadline))?;
        let common = exact_worktree_common_git_dir_until(inspection_path, deadline)?;
        let target = std::fs::canonicalize(inspection_path).map_err(|error| error.to_string())?;
        let listed = list_registered_worktrees_in_common_directory_until(&common, deadline)?;
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

        check_directory_deletion_deadline(Some(deadline))?;
        if let Err(detail) = stop_fsmonitor(
            inspection_path,
            deadline,
            first_submodule_spawn_admission,
        ) {
            return Ok(ExactWorktreeRemoval::StopFailed { detail });
        }
        check_directory_deletion_deadline(Some(deadline))?;
        let verification_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let verify_confirmed_snapshot = |
            candidate: &Path,
            git_bindings: Option<&std::collections::BTreeMap<PathBuf, PathBuf>>,
        | -> Result<(), String> {
            if let Some(git_bindings) = git_bindings {
                verify_git_directory_bindings_until(candidate, git_bindings, deadline)?;
            }
            let (fresh, _) = verification_runtime.block_on(inspect_worktree_at_until(
                &inspection_identity,
                candidate.to_path_buf(),
                deadline,
            ))?;
            if fresh.fingerprint() != confirmed_fingerprint {
                return Err(
                    "worktree changed after Close inspection confirmation; fresh confirmation is required"
                        .to_string(),
                );
            }
            ensure_no_ignored_content_until(candidate, deadline)
        };
        if let Err(detail) = verify_confirmed_snapshot(inspection_path, None) {
            return Ok(ExactWorktreeRemoval::ReinspectionRequired { detail });
        }
        let verified_git_bindings = capture_git_directory_bindings_until(inspection_path, deadline)?;
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
            check_directory_deletion_deadline(Some(deadline))?;
            std::fs::rename(&path, &quarantine)
                .map_err(|error| format!("cannot quarantine captured worktree: {error}"))?;
        }
        check_directory_deletion_deadline(Some(deadline))?;
        if observe_worktree_fingerprint(&quarantine).as_deref() != Some(expected.as_str()) {
            return Err(format!(
                "quarantined worktree administrative incarnation changed; retained at {}",
                quarantine.display(),
            ));
        }

        check_directory_deletion_deadline(Some(deadline))?;
        after_quarantine(&path);
        check_directory_deletion_deadline(Some(deadline))?;
        let open_descriptors = observe_open_descriptors(&quarantine, deadline)?;
        check_directory_deletion_deadline(Some(deadline))?;
        if open_descriptors.blocks_retirement() {
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
        check_directory_deletion_deadline(Some(deadline))?;
        let external_writer = observe_external_writer(&quarantine, deadline)?;
        check_directory_deletion_deadline(Some(deadline))?;
        if external_writer {
            return Ok(ExactWorktreeRemoval::Residual {
                detail: format!(
                    "an external process can still write the confirmed worktree; retained at {}",
                    quarantine.display()
                ),
            });
        }
        if let Err(detail) =
            verify_confirmed_snapshot(&quarantine, Some(&verified_git_bindings))
        {
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
        remove_identity_bound_directory_with_deadline(
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
            |_, object| verify_confirmed_snapshot(object, Some(&verified_git_bindings)),
            "quarantined worktree",
            Some(deadline),
        )?;
        remove_exact_worktree_administrative_dir_until(
            &administrative_dir,
            &planned_administrative_dir_incarnation,
            deadline,
        )?;
        check_directory_deletion_deadline(Some(deadline))?;
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
    ensure_no_ignored_content_until(
        path,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
    )
}

fn ensure_no_ignored_content_until(
    path: &Path,
    deadline: std::time::Instant,
) -> Result<(), String> {
    ensure_no_ignored_content_with_git_leaf_until(path, deadline, &mut run_bounded_spawned_git)
}

fn ensure_no_ignored_content_with_git_leaf_until(
    path: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(), String> {
    fn inspect(
        path: &Path,
        deadline: std::time::Instant,
        visited: &mut std::collections::HashSet<PathBuf>,
        git_leaf: &mut WorktreeGitLeaf<'_>,
    ) -> Result<(), String> {
        if std::time::Instant::now() >= deadline {
            return Err("ignored-content inspection exceeded its deadline".into());
        }
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !visited.insert(canonical) {
            return Err("initialized submodule graph contains a cycle".into());
        }
        ensure_no_ignored_content_with_status_leaf_until(path, deadline, |path, deadline| {
            run_bounded_git_status_with_leaf_until(path, deadline, git_leaf)
        })?;
        let (_, gitlinks) = index_gitlinks_with_leaf_until(path, deadline, git_leaf)?;
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
                inspect(&submodule, deadline, visited, git_leaf)?;
            }
        }
        Ok(())
    }
    inspect(
        path,
        deadline,
        &mut std::collections::HashSet::new(),
        git_leaf,
    )
}

/// One ignored-content status leaf, with execution supplied explicitly by the caller.
///
/// This is deliberately not the recursive ignored-content inspection: a native
/// caller must admit every submodule probe separately, using its captured binding.
/// A clean result here alone is never full inspection or removal authority.
fn ensure_no_ignored_content_with_status_leaf_until(
    path: &Path,
    deadline: std::time::Instant,
    status_leaf: impl FnOnce(&Path, std::time::Instant) -> Result<std::process::Output, String>,
) -> Result<(), String> {
    if std::time::Instant::now() >= deadline {
        return Err("ignored-content inspection exceeded its deadline".into());
    }
    let status = status_leaf(path, deadline)?;
    if std::time::Instant::now() >= deadline {
        return Err("ignored-content inspection exceeded its deadline".into());
    }
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
    Ok(())
}

fn observe_ignored_paths_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<GitPathIdentity>, String> {
    let output = run_bounded_git_command_with_leaf_until(
        repository,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
        ],
        None,
        deadline,
        "exact ignored-path inspection",
        git_leaf,
    )?;
    if !output.status.success() {
        return Err(format!(
            "cannot enumerate exact ignored paths: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| GitPathIdentity::from_bytes(path.to_vec()))
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| path.as_bytes().to_vec());
    paths.dedup();
    Ok(paths)
}

fn append_ignored_paths(observation: &mut Vec<u8>, paths: &[GitPathIdentity]) {
    for path in paths {
        observation.extend_from_slice(b"IGNORED_PATH\0");
        observation.extend_from_slice(path.as_bytes());
        observation.push(0);
    }
}

fn canonical_status_observation(status: &[u8]) -> Vec<u8> {
    let mut observation = Vec::with_capacity(status.len());
    let mut rows = status.split(|byte| *byte == 0);
    while let Some(row) = rows.next() {
        if row.len() < 4 {
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

#[cfg(test)]
fn observe_hidden_worktree_changes_until(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<Vec<GitPathIdentity>, String> {
    observe_hidden_worktree_changes_with_leaf_until(
        repository,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn observe_hidden_worktree_changes_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<GitPathIdentity>, String> {
    if std::time::Instant::now() >= deadline {
        return Err("hidden worktree inspection exceeded its deadline".into());
    }
    let index_path = git_index_path_with_leaf_until(repository, deadline, git_leaf)?;
    if !index_path.exists() {
        if std::time::Instant::now() >= deadline {
            return Err("hidden worktree inspection exceeded its deadline".into());
        }
        return Ok(Vec::new());
    }
    if std::time::Instant::now() >= deadline {
        return Err("hidden worktree inspection exceeded its deadline".into());
    }
    let private_index = tempfile::NamedTempFile::new_in(
        index_path
            .parent()
            .ok_or("Git index has no parent directory")?,
    )
    .map_err(|error| error.to_string())?;
    if std::time::Instant::now() >= deadline {
        return Err("hidden worktree inspection exceeded its deadline".into());
    }
    std::fs::copy(&index_path, private_index.path())
        .map_err(|error| format!("cannot copy Git index for hidden-change inspection: {error}"))?;
    let private_index = private_index.path();
    let candidates =
        inspect_hidden_index_candidates(repository, private_index, deadline, git_leaf)?;
    if candidates.is_empty() {
        if std::time::Instant::now() >= deadline {
            return Err("hidden worktree inspection exceeded its deadline".into());
        }
        return Ok(Vec::new());
    }
    let sparse_absent =
        sparse_checkout_absent_paths_with_leaf_until(repository, &candidates, deadline, git_leaf)?;

    let mut dirty = BTreeSet::new();
    for batch in candidates.chunks(256) {
        if std::time::Instant::now() >= deadline {
            return Err("hidden worktree inspection exceeded its deadline".into());
        }
        let batch_paths = batch
            .iter()
            .filter(|candidate| !sparse_absent.contains(candidate.path.as_slice()))
            .map(|candidate| candidate.path.clone())
            .collect::<Vec<_>>();
        if batch_paths.is_empty() {
            continue;
        }
        // Path arguments are OsStrings below so non-UTF-8 paths remain literal.
        clear_hidden_index_flags(repository, private_index, &batch_paths, deadline, git_leaf)?;
        let diff = run_bounded_git_command_with_leaf_until(
            repository,
            &["diff-files", "--raw", "-z", "--no-ext-diff"],
            Some(private_index),
            deadline,
            "hidden worktree comparison",
            git_leaf,
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
    if std::time::Instant::now() >= deadline {
        return Err("hidden worktree inspection exceeded its deadline".into());
    }
    Ok(dirty.into_iter().map(GitPathIdentity::from_bytes).collect())
}

fn clear_hidden_index_flags(
    repository: &Path,
    private_index: &Path,
    batch_paths: &[Vec<u8>],
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<(), String> {
    let clear = run_bounded_git_paths_with_leaf_until(
        repository,
        &["update-index", "--no-assume-unchanged", "--"],
        batch_paths,
        true,
        private_index,
        deadline,
        "hidden-index flag clearing",
        git_leaf,
    )?;
    if !clear.status.success() {
        return Err(format!(
            "cannot clear hidden flags in private index: {}",
            String::from_utf8_lossy(&clear.stderr).trim()
        ));
    }
    let clear_skip = run_bounded_git_paths_with_leaf_until(
        repository,
        &["update-index", "--no-skip-worktree", "--"],
        batch_paths,
        true,
        private_index,
        deadline,
        "hidden-index flag clearing",
        git_leaf,
    )?;
    if !clear_skip.status.success() {
        return Err(format!(
            "cannot clear hidden flags in private index: {}",
            String::from_utf8_lossy(&clear_skip.stderr).trim()
        ));
    }
    Ok(())
}

fn inspect_hidden_index_candidates(
    repository: &Path,
    private_index: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<HiddenIndexCandidate>, String> {
    let flags = run_bounded_git_command_with_leaf_until(
        repository,
        &["ls-files", "-v", "-z", "--"],
        Some(private_index),
        deadline,
        "hidden-index inspection",
        git_leaf,
    )?;
    if !flags.status.success() {
        return Err(format!(
            "cannot inspect hidden index flags: {}",
            String::from_utf8_lossy(&flags.stderr).trim()
        ));
    }
    Ok(hidden_index_candidates(&flags.stdout))
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

#[cfg(test)]
fn sparse_checkout_absent_paths(
    repository: &Path,
    candidates: &[HiddenIndexCandidate],
    deadline: std::time::Instant,
) -> Result<BTreeSet<Vec<u8>>, String> {
    sparse_checkout_absent_paths_with_leaf_until(
        repository,
        candidates,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn sparse_checkout_absent_paths_with_leaf_until(
    repository: &Path,
    candidates: &[HiddenIndexCandidate],
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
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

    let sparse_patterns = git_index_path_with_leaf_until(repository, deadline, git_leaf)?
        .parent()
        .ok_or("Git index has no parent directory")?
        .join("info")
        .join("sparse-checkout");
    if !sparse_patterns.exists() {
        return Ok(BTreeSet::new());
    }

    let cone = git_sparse_checkout_cone_mode_with_leaf_until(repository, deadline, git_leaf)?;
    let sparse_matches = git_sparse_checkout_selected_paths_with_leaf_until(
        repository,
        &skip_absent,
        cone,
        deadline,
        git_leaf,
    )?
    .into_iter()
    .collect::<BTreeSet<_>>();
    Ok(skip_absent
        .into_iter()
        .filter(|path| !sparse_matches.contains(path))
        .collect())
}

#[cfg(test)]
fn git_sparse_checkout_cone_mode(
    repository: &Path,
    deadline: std::time::Instant,
) -> Result<bool, String> {
    git_sparse_checkout_cone_mode_with_leaf_until(
        repository,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn git_sparse_checkout_cone_mode_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<bool, String> {
    let output = run_bounded_git_command_with_leaf_until(
        repository,
        &["config", "--bool", "--get", "core.sparseCheckoutCone"],
        None,
        deadline,
        "sparse-checkout cone inspection",
        git_leaf,
    )?;
    Ok(output.status.success() && output.stdout.trim_ascii() == b"true")
}

#[cfg(test)]
fn git_sparse_checkout_selected_paths(
    repository: &Path,
    paths: &[Vec<u8>],
    cone: bool,
    deadline: std::time::Instant,
) -> Result<Vec<Vec<u8>>, String> {
    git_sparse_checkout_selected_paths_with_leaf_until(
        repository,
        paths,
        cone,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn git_sparse_checkout_selected_paths_with_leaf_until(
    repository: &Path,
    paths: &[Vec<u8>],
    cone: bool,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<Vec<u8>>, String> {
    const OPERATION: &str = "sparse-checkout rule inspection";
    if std::time::Instant::now() >= deadline {
        return Err(format!("{OPERATION} exceeded its deadline"));
    }

    let mut input = tempfile::tempfile()
        .map_err(|error| format!("cannot prepare sparse-checkout probe paths: {error}"))?;
    {
        use std::io::Write as _;
        for path in paths {
            if std::time::Instant::now() >= deadline {
                return Err(format!("{OPERATION} exceeded its deadline"));
            }
            // Filesystem writes are cooperative and cannot be preempted mid-write.
            input
                .write_all(path)
                .and_then(|()| input.write_all(&[0]))
                .map_err(|error| format!("cannot write sparse-checkout probe paths: {error}"))?;
        }
    }
    if std::time::Instant::now() >= deadline {
        return Err(format!("{OPERATION} exceeded its deadline"));
    }
    {
        use std::io::Seek as _;
        input
            .rewind()
            .map_err(|error| format!("cannot rewind sparse-checkout probe paths: {error}"))?;
    }

    let mut command = phoenix_core::git::command();
    bind_git_command_to_worktree_until(&mut command, repository, deadline)?;
    command
        .arg("sparse-checkout")
        .arg("check-rules")
        .arg("-z")
        .stdin(std::process::Stdio::from(input))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0");
    if cone {
        command.arg("--cone");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let output = execute_worktree_git_leaf_until(command, deadline, OPERATION, git_leaf)?;
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

#[cfg(test)]
fn git_index_path(repository: &Path, deadline: std::time::Instant) -> Result<PathBuf, String> {
    git_index_path_with_leaf_until(repository, deadline, &mut run_bounded_spawned_git)
}

fn git_index_path_with_leaf_until(
    repository: &Path,
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<PathBuf, String> {
    let output = run_bounded_git_command_with_leaf_until(
        repository,
        &["rev-parse", "--git-path", "index"],
        None,
        deadline,
        "Git index path inspection",
        git_leaf,
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

#[cfg(test)]
fn observe_dirty_content_until(
    repository: &Path,
    losses: &[CloseLossItem],
    deadline: std::time::Instant,
) -> Result<Vec<u8>, String> {
    observe_dirty_content_with_leaf_until(
        repository,
        losses,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn observe_dirty_content_with_leaf_until(
    repository: &Path,
    losses: &[CloseLossItem],
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Vec<u8>, String> {
    if std::time::Instant::now() >= deadline {
        return Err("dirty content inspection exceeded its deadline".into());
    }
    let mut observation = Vec::new();
    // These checks are cooperative: a blocking filesystem operation already in
    // progress cannot be interrupted when the deadline elapses.
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
    let staged_entries =
        staged_index_entries_for_paths_with_leaf_until(repository, &paths, deadline, git_leaf)?;
    for path in paths {
        if std::time::Instant::now() >= deadline {
            return Err("dirty content inspection exceeded its deadline".into());
        }
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
                use std::io::Read as _;

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
                    if std::time::Instant::now() >= deadline {
                        return Err("dirty content inspection exceeded its deadline".into());
                    }
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
    if std::time::Instant::now() >= deadline {
        return Err("dirty content inspection exceeded its deadline".into());
    }
    Ok(observation)
}

#[cfg(test)]
fn staged_index_entries_for_paths_until(
    repository: &Path,
    paths: &[Vec<u8>],
    deadline: std::time::Instant,
) -> Result<std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>, String> {
    staged_index_entries_for_paths_with_leaf_until(
        repository,
        paths,
        deadline,
        &mut run_bounded_spawned_git,
    )
}

fn staged_index_entries_for_paths_with_leaf_until(
    repository: &Path,
    paths: &[Vec<u8>],
    deadline: std::time::Instant,
    git_leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>, String> {
    if std::time::Instant::now() >= deadline {
        return Err("dirty index inspection exceeded its deadline".into());
    }
    let mut entries = std::collections::BTreeMap::new();
    for batch in paths.chunks(256) {
        if std::time::Instant::now() >= deadline {
            return Err("dirty index inspection exceeded its deadline".into());
        }
        let mut command = phoenix_core::git::command();
        bind_git_command_to_worktree_until(&mut command, repository, deadline)?;
        command
            .args(["--literal-pathspecs", "ls-files", "--stage", "-z", "--"])
            .args(batch.iter().map(|path| path_buf_from_git_bytes(path)))
            .current_dir(repository)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let output =
            execute_worktree_git_leaf_until(command, deadline, "dirty index inspection", git_leaf)?;
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
            // Porcelain may collapse an ignored directory; exact files are read
            // separately with ls-files before any blocker rows are materialized.
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

/// Parses nonempty stable registry identity strings into opaque identities.
fn lease_process_identities(identities: impl Iterator<Item = String>) -> Vec<OpaqueIdentity> {
    identities
        .map(|identity| {
            OpaqueIdentity::parse(identity).expect("registry stable instance identity is non-empty")
        })
        .collect()
}

fn require_absent(outcome: BashRetirementOutcome) -> Result<CloseProcessStepOutcome, String> {
    match outcome {
        BashRetirementOutcome::StaleGeneration(_) => {
            Err("bash retirement generation is stale".to_string())
        }
        BashRetirementOutcome::Retired(report) if !report.has_failures() => {
            Ok(CloseProcessStepOutcome::Retired)
        }
        BashRetirementOutcome::AbsenceVerified(report) if !report.has_failures() => {
            Ok(CloseProcessStepOutcome::AbsenceVerified)
        }
        BashRetirementOutcome::Retired(report) | BashRetirementOutcome::AbsenceVerified(report) => {
            Err(format!(
                "bash retirement left {} process-group failure(s); scope uncertainty: {:?}",
                report.kill_failures.len(),
                report.scope_uncertainty
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
    fn opaque_identity(value: &str) -> OpaqueIdentity {
        OpaqueIdentity::parse(value).expect("test stable identity is non-empty")
    }

    #[test]
    fn native_inventory_preparation_preserves_exact_durable_identities() {
        use phoenix_core::domain::close::{
            CapturedWorktreeIdentity, GitPathIdentity, RetiredResourceKind as Kind,
            WorktreeFingerprint, WorktreeId, WorktreeIdentity,
        };
        use phoenix_core::work_scope::WorkScopeId;

        let attempt_id =
            phoenix_core::domain::close::CloseAttemptId::parse("native-inventory-attempt").unwrap();
        let snapshot = super::snapshot_for(b"native inventory snapshot");
        let scope = WorkScopeId::parse("native-inventory-scope").unwrap();
        let worktree = WorktreeIdentity::from_parts(
            WorktreeId::parse("native-inventory-worktree").unwrap(),
            WorktreeFingerprint::parse("captured-worktree-incarnation").unwrap(),
            GitPathIdentity::from_bytes(b"/tmp/native-inventory-worktree".to_vec()),
        );
        let tmux = super::opaque_resource(Kind::TmuxServer, "tmux-stable-server-instance".into());
        let input = super::CloseRetirementInventoryScopeInput {
            scope: scope.clone(),
            captured_worktree: Some(CapturedWorktreeIdentity::Resolved(worktree.clone())),
            acquired_resources: vec![
                tmux.clone(),
                super::opaque_resource(Kind::BashProcessGroup, "ephemeral-bash".into()),
                super::opaque_resource(Kind::PtySession, "ephemeral-pty".into()),
                super::opaque_resource(Kind::BrowserSession, "ephemeral-browser".into()),
            ],
        };

        let request = super::build_close_retirement_inventory_scope_request(
            attempt_id.clone(),
            snapshot.clone(),
            input,
            super::CloseRetirementInventoryPolicy::NativeInitial,
        )
        .unwrap();
        assert_eq!(request.attempt_id, attempt_id);
        assert_eq!(request.snapshot, snapshot);
        assert_eq!(request.scopes.len(), 1);
        assert_eq!(request.scopes[0].scope, scope);
        assert_eq!(request.scopes[0].inventory.worktree, Some(worktree));
        let phoenix_core::domain::close::LossItemIdentity::Opaque(tmux_identity) = tmux.identity()
        else {
            unreachable!();
        };
        assert_eq!(request.scopes[0].inventory.tmux_servers.len(), 1);
        assert!(request.scopes[0]
            .inventory
            .tmux_servers
            .contains(tmux_identity));
        assert_eq!(request.scopes[0].inventory.bash_process_groups.len(), 1);
        assert_eq!(request.scopes[0].inventory.pty_sessions.len(), 1);
        assert_eq!(request.scopes[0].inventory.browser_sessions.len(), 1);
        assert!(request.scopes[0]
            .inventory
            .equivalent_live_resources
            .is_empty());
        assert!(request.scopes[0]
            .inventory
            .bash_process_groups
            .contains(&opaque_identity("ephemeral-bash")));
        assert!(request.scopes[0]
            .inventory
            .pty_sessions
            .contains(&opaque_identity("ephemeral-pty")));
        assert!(request.scopes[0]
            .inventory
            .browser_sessions
            .contains(&opaque_identity("ephemeral-browser")));
    }

    #[test]
    fn legacy_inventory_preparation_keeps_tmux_only_filtering() {
        use phoenix_core::domain::close::RetiredResourceKind as Kind;
        use phoenix_core::work_scope::WorkScopeId;

        let attempt_id =
            phoenix_core::domain::close::CloseAttemptId::parse("legacy-inventory-attempt").unwrap();
        let snapshot = super::snapshot_for(b"legacy inventory snapshot");
        let request = super::build_close_retirement_inventory_scope_request(
            attempt_id,
            snapshot,
            super::CloseRetirementInventoryScopeInput {
                scope: WorkScopeId::parse("legacy-inventory-scope").unwrap(),
                captured_worktree: None,
                acquired_resources: vec![
                    super::opaque_resource(Kind::BashProcessGroup, "legacy-bash".into()),
                    super::opaque_resource(Kind::PtySession, "legacy-pty".into()),
                    super::opaque_resource(Kind::BrowserSession, "legacy-browser".into()),
                    super::opaque_resource(Kind::TmuxServer, "legacy-tmux".into()),
                ],
            },
            super::CloseRetirementInventoryPolicy::LegacyDurable,
        )
        .unwrap();
        let inventory = &request.scopes[0].inventory;
        assert!(inventory.bash_process_groups.is_empty());
        assert!(inventory.pty_sessions.is_empty());
        assert!(inventory.browser_sessions.is_empty());
        assert_eq!(inventory.tmux_servers.len(), 1);
        assert!(inventory
            .tmux_servers
            .contains(&opaque_identity("legacy-tmux")));
    }

    #[test]
    fn native_inventory_preparation_keeps_empty_process_inventory_empty() {
        use phoenix_core::domain::close::CapturedWorktreeIdentity;
        use phoenix_core::work_scope::WorkScopeId;

        let attempt_id =
            phoenix_core::domain::close::CloseAttemptId::parse("native-empty-attempt").unwrap();
        let request = super::build_close_retirement_inventory_scope_request(
            attempt_id,
            super::snapshot_for(b"native empty inventory snapshot"),
            super::CloseRetirementInventoryScopeInput {
                scope: WorkScopeId::parse("native-empty-scope").unwrap(),
                captured_worktree: None::<CapturedWorktreeIdentity>,
                acquired_resources: Vec::new(),
            },
            super::CloseRetirementInventoryPolicy::NativeInitial,
        )
        .unwrap();
        let inventory = &request.scopes[0].inventory;
        assert!(inventory.bash_process_groups.is_empty());
        assert!(inventory.tmux_servers.is_empty());
        assert!(inventory.pty_sessions.is_empty());
        assert!(inventory.browser_sessions.is_empty());
        assert!(inventory.equivalent_live_resources.is_empty());
    }

    #[test]
    fn native_inventory_preparation_rejects_unexpected_registry_identity() {
        use phoenix_core::domain::close::RetiredResourceKind as Kind;
        use phoenix_core::work_scope::WorkScopeId;

        let error = super::build_close_retirement_inventory_scope_request(
            phoenix_core::domain::close::CloseAttemptId::parse("native-unexpected-attempt")
                .unwrap(),
            super::snapshot_for(b"native unexpected inventory snapshot"),
            super::CloseRetirementInventoryScopeInput {
                scope: WorkScopeId::parse("native-unexpected-scope").unwrap(),
                captured_worktree: None,
                acquired_resources: vec![super::opaque_resource(
                    Kind::EquivalentLiveResource,
                    "unexpected".into(),
                )],
            },
            super::CloseRetirementInventoryPolicy::NativeInitial,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            super::CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed { .. }
        ));
    }

    #[test]
    fn native_inventory_preparation_rejects_unresolved_captured_scope() {
        use phoenix_core::domain::close::{
            CapturedWorktreeIdentity, GitPathIdentity, RetirementFailureReason,
        };
        use phoenix_core::work_scope::WorkScopeId;

        let scope = WorkScopeId::parse("native-unresolved-scope").unwrap();
        let attempt_id =
            phoenix_core::domain::close::CloseAttemptId::parse("native-unresolved-attempt")
                .unwrap();
        let snapshot = super::snapshot_for(b"unresolved inventory snapshot");
        let error = super::build_close_retirement_inventory_scope_request(
            attempt_id,
            snapshot,
            super::CloseRetirementInventoryScopeInput {
                scope: scope.clone(),
                captured_worktree: Some(CapturedWorktreeIdentity::Unresolved {
                    locator: GitPathIdentity::from_bytes(b"/tmp/unresolved-worktree".to_vec()),
                }),
                acquired_resources: Vec::new(),
            },
            super::CloseRetirementInventoryPolicy::NativeInitial,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            super::CloseRetirementInventoryPreparationError::CapturedScopeIdentity {
                scope: error_scope,
                reason: RetirementFailureReason::IdentityNotProven,
                ..
            } if error_scope == scope
        ));
    }

    #[test]
    fn scope_uncertainty_blocks_close_without_inventing_a_process_failure() {
        use phoenix_tools::bash::registry::{
            BashRetirementOutcome, BashScopeUncertainty, CascadeBashReport,
        };

        let report = CascadeBashReport {
            scope_uncertainty: Some(BashScopeUncertainty::UnregisteredLaunch),
            ..CascadeBashReport::default()
        };
        assert!(report.kill_failures.is_empty());
        for outcome in [
            BashRetirementOutcome::Retired(report.clone()),
            BashRetirementOutcome::AbsenceVerified(report),
        ] {
            let error =
                super::require_absent(outcome).expect_err("scope uncertainty must block Close");
            assert!(error.contains("0 process-group failure(s)"));
            assert!(error.contains("UnregisteredLaunch"));
        }
    }

    use super::{
        append_ignored_paths, both_worktree_paths_absent, canonical_status_observation,
        complete_persisted_worktree_administrative_cleanup, exact_worktree_administrative_dir,
        git_path_from_observation, inspect_and_remove_exact_worktree_with_hook, inspect_worktree,
        observe_administrative_dir_incarnation, observe_ignored_paths_with_leaf_until,
        observe_worktree_fingerprint, parse_status_losses, planned_administrative_dir_is_absent,
        remove_directory_contents_at_with_hook, remove_exact_worktree_administrative_dir_with_hook,
        remove_identity_bound_directory, remove_quarantine_then_administrative_dir,
        remove_quarantine_then_administrative_dir_with_hooks, rotate_inspection_generation,
        run_bounded_git_status_until, snapshot_for, staged_index_entries_by_path,
        staged_index_entries_for_paths_until, worktree_quarantine_path, CloseLeaseFailure,
        ExactWorktreeRemoval, FinalTombstoneRecovery,
    };
    use crate::db::{CloseCleanupResourceDisposition, CloseWorktreeFinalTombstone};
    use phoenix_core::domain::close::{
        CloseLossItem, GitPathIdentity, OpaqueIdentity, WorktreeFingerprint, WorktreeId,
        WorktreeIdentity,
    };
    use std::path::{Path, PathBuf};

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
        let remaining = plan
            .iter()
            .map(|effect| crate::db::CloseCleanupFailureResource {
                scope: effect.scope.clone(),
                resource: effect.resource.clone(),
                disposition: crate::db::CloseCleanupResourceDisposition::Failed,
            })
            .collect::<Vec<_>>();
        let admitted_plan = super::close_safe_retry_effects(&remaining).unwrap();
        assert_eq!(
            admitted_plan,
            vec![plan[2].clone(), plan[1].clone(), plan[0].clone()]
        );
        assert_eq!(
            super::close_safe_retry_execution_order(&admitted_plan),
            vec![0, 1, 2]
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
        assert!(manager
            .close_retirement_leases
            .lock()
            .await
            .complete
            .is_empty());
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
        assert_eq!(
            manager.close_retirement_leases.lock().await.complete.len(),
            1
        );
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
            assert!(manager
                .close_retirement_leases
                .lock()
                .await
                .complete
                .is_empty());
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

    #[allow(clippy::too_many_lines)]
    async fn build_close_attempt_with_sentinel_worktree(
        db: &crate::db::Database,
        ids: &[&str; 3],
    ) -> (
        phoenix_core::domain::close::CloseAttemptId,
        phoenix_core::work_scope::WorkScopeId,
        WorktreeIdentity,
    ) {
        use phoenix_core::domain::close::{
            CloseAttemptId, CloseRetirementSnapshot, TranscriptConversationId,
        };

        let root = db
            .create_conversation(ids[0], ids[0], "/tmp", true, None, None)
            .await
            .expect("create root conversation");
        let scope = root
            .attached_work_scope_id
            .clone()
            .expect("root conversation has an attached scope");

        let worktree = WorktreeIdentity::from_parts(
            WorktreeId::parse("sentinel-worktree-id").unwrap(),
            WorktreeFingerprint::parse("sentinel-fingerprint").unwrap(),
            GitPathIdentity::from_bytes(b"/inert/sentinel/worktree-never-touched".to_vec()),
        );
        sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = 'allocated_worktree',
                 worktree_path = ?1, worktree_id = ?2, worktree_fingerprint = ?3,
                 branch_name = 'sentinel-branch', base_branch = 'main'
             WHERE id = ?4",
        )
        .bind("/inert/sentinel/worktree-never-touched")
        .bind(worktree.id().as_str())
        .bind(worktree.fingerprint().as_str())
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .expect("seal the sentinel worktree onto the scope (no filesystem touched)");

        // Two continuations sharing the root's scope -- same membership
        // wiring as the chain-archive fixture, minus the real git worktree.
        for pair in ids.windows(2) {
            let predecessor_id = pair[0];
            let id = pair[1];
            let mut tx = db.pool().begin().await.expect("begin continuation fixture");
            sqlx::query("PRAGMA defer_foreign_keys = ON")
                .execute(&mut *tx)
                .await
                .expect("defer fixture foreign keys");
            sqlx::query(
                "INSERT INTO product_continuation_reservations (
                     predecessor_conversation_id, successor_conversation_id, product_conversation_id
                 ) VALUES (?1, ?2, ?3)",
            )
            .bind(predecessor_id)
            .bind(id)
            .bind(root.product_conversation_id.as_str())
            .execute(&mut *tx)
            .await
            .expect("reserve continuation edge");
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(id)
                .bind(predecessor_id)
                .execute(&mut *tx)
                .await
                .expect("link");
            sqlx::query(
                "INSERT INTO conversations (
                     id, product_conversation_id, slug, title, user_initiated,
                     state, state_kind, state_updated_at, created_at, updated_at,
                     archived, model, project_id, desired_base_branch,
                     cm_kind, cm_task_id, cm_task_title, runtime_role, work_scope_id
                 )
                 SELECT ?1, product_conversation_id, ?2, ?3, 1,
                        state, state_kind, ?4, ?4, ?4,
                        0, model, project_id, desired_base_branch,
                        cm_kind, cm_task_id, cm_task_title, 'user', work_scope_id
                 FROM conversations WHERE id = ?5",
            )
            .bind(id)
            .bind(format!("slug-{id}"))
            .bind(format!("Title {id}"))
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(predecessor_id)
            .execute(&mut *tx)
            .await
            .expect("create continuation member");
            sqlx::query(
                "DELETE FROM product_continuation_reservations
                 WHERE predecessor_conversation_id = ?1",
            )
            .bind(predecessor_id)
            .execute(&mut *tx)
            .await
            .expect("consume continuation reservation");
            tx.commit().await.expect("commit continuation fixture");
        }

        let attempt_id = CloseAttemptId::parse(format!("close-attempt-{}", ids[0])).unwrap();
        db.begin_close_foundation(
            &root.product_conversation_id,
            &TranscriptConversationId::parse(ids[2]).unwrap(),
            attempt_id.as_str(),
        )
        .await
        .expect("begin close foundation");
        db.confirm_close_stop_work(attempt_id.as_str())
            .await
            .expect("confirm stop-work");
        db.begin_close_active_work_settlement(attempt_id.as_str())
            .await
            .expect("begin active-work settlement");
        db.advance_close_settlement_when_quiescent(attempt_id.as_str())
            .await
            .expect("advance to awaiting retirement inspection");
        db.replace_close_inspection(crate::db::ReplaceCloseInspectionRequest {
            attempt_id: attempt_id.clone(),
            scopes: vec![crate::db::ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("sentinel-gen", "sentinel-fp").unwrap(),
                losses: vec![],
            }],
        })
        .await
        .expect("replace inspection with zero losses -> RetirementRequested");

        (attempt_id, scope, worktree)
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn prepare_retirement_captures_and_seals_one_exact_shared_worktree() {
        use phoenix_core::domain::close::{
            CapturedWorktreeIdentity, ClosePhase, LossItemIdentity, RetiredResourceKind,
        };
        use std::sync::Arc;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let ids = ["ri-a", "ri-a2", "ri-a3"];
        let (attempt_id, scope, worktree) =
            build_close_attempt_with_sentinel_worktree(&db, &ids).await;

        let obligation = db.get_close_obligation(attempt_id.as_str()).await.unwrap();
        assert_eq!(obligation.phase(), ClosePhase::RetirementRequested);
        let snapshot = obligation
            .snapshot()
            .cloned()
            .expect("sealed aggregate snapshot present");
        let product_conversation_id = obligation.product_conversation_id().clone();

        assert!(
            !db.close_retirement_inventory_is_complete(attempt_id.as_str())
                .await
                .unwrap(),
            "inventory must be incomplete before capture"
        );

        let mut manager = super::RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "test".to_string(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        );

        manager.tmux_registry =
            Arc::new(phoenix_tools::tmux::registry::TmuxRegistry::with_backend(
                std::path::PathBuf::from("/inert/tmux"),
                phoenix_tools::tmux::fake_backend::FakeTmuxBackend::unavailable(),
                None,
            ));
        let prepared = manager
            .prepare_close_retirement_targets(&attempt_id, &snapshot)
            .await
            .expect("production capture must succeed from committed DB state alone");

        assert!(
            db.close_retirement_inventory_is_complete(attempt_id.as_str())
                .await
                .unwrap(),
            "inventory must be complete after capture"
        );

        let captured_scopes = db
            .list_close_attempt_scopes(attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(
            captured_scopes.len(),
            1,
            "3 continuation members must share exactly one captured scope"
        );
        assert_eq!(captured_scopes[0].scope, scope);
        assert_eq!(
            captured_scopes[0].captured_worktree,
            Some(CapturedWorktreeIdentity::Resolved(worktree.clone())),
            "captured worktree identity must be the exact sentinel identity"
        );

        let expected = db
            .list_close_expected_retirement_resources(attempt_id.as_str())
            .await
            .unwrap();
        let worktree_targets: Vec<_> = expected
            .iter()
            .filter(|target| target.resource.kind() == RetiredResourceKind::Worktree)
            .collect();
        assert_eq!(
            worktree_targets.len(),
            1,
            "exactly one worktree target sealed"
        );
        assert_eq!(
            worktree_targets[0].resource.identity(),
            &LossItemIdentity::Worktree(worktree.clone())
        );

        assert_eq!(prepared, expected);
        for target in &prepared {
            assert_eq!(target.attempt_id, attempt_id);
            assert_eq!(target.scope, scope);
            assert_eq!(target.snapshot, snapshot);
        }
        let repeated = manager
            .prepare_close_retirement_targets(&attempt_id, &snapshot)
            .await
            .expect("repeat capture against the sealed snapshot must be idempotent");
        assert_eq!(repeated, prepared);
        let expected_again = db
            .list_close_expected_retirement_resources(attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(
            expected, expected_again,
            "repeat capture must not duplicate sealed resources"
        );

        assert!(
            db.list_close_retirement_evidence(attempt_id.as_str())
                .await
                .unwrap()
                .is_empty(),
            "capture must not produce any retirement receipts"
        );

        assert_close_admission_fenced(&manager, &scope).await;

        let aggregate = db
            .get_ordinary_product_conversation(&product_conversation_id)
            .await
            .unwrap();
        assert_ne!(
            aggregate.product_conversation.ordinary_lifecycle(),
            Some(phoenix_core::domain::product_conversation::OrdinaryProductConversationLifecycle::History),
            "capture must not transition the aggregate to History by itself"
        );

        let after = db.get_close_obligation(attempt_id.as_str()).await.unwrap();
        assert_eq!(after.phase(), ClosePhase::RetirementRequested);
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
                    .complete
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
            assert!(manager
                .close_retirement_leases
                .lock()
                .await
                .complete
                .is_empty());
        }
    }

    struct ProcessStepFixture {
        manager: super::RuntimeManager,
        run: super::CloseRunRef,
        snapshot: super::CloseRetirementSnapshot,
        scopes: Vec<super::WorkScopeId>,
        identities: Vec<super::CloseProcessStepIdentities>,
        bash: Vec<super::RetiredResourceIdentity>,
        tmux: Vec<Vec<super::RetiredResourceIdentity>>,
        terminal: Vec<super::RetiredResourceIdentity>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ProcessStepCall {
        Bash,
        Tmux,
        Terminal,
        Browser,
    }

    #[derive(Debug)]
    enum ScriptedObservation {
        Bash(Result<super::CloseObservedProcessRetirement, super::CloseProcessStepFailure>),
        Tmux(Result<super::CloseObservedTmuxRetirement, (super::RetirementFailureReason, String)>),
        Terminal(Result<super::CloseObservedProcessRetirement, super::CloseProcessStepFailure>),
        Browser(Result<super::CloseObservedProcessRetirement, super::CloseProcessStepFailure>),
    }

    fn opaque_ids(ids: &[String]) -> Vec<super::OpaqueIdentity> {
        ids.iter()
            .map(|id| super::OpaqueIdentity::parse(id.clone()).unwrap())
            .collect()
    }

    /// Builds the observed process retirement a scripted executor "saw",
    /// carrying exactly the supplied identities. Tests pass the fixture's
    /// expected identities here for a matching observation, or a deliberately
    /// different set to exercise mismatch handling.
    fn observed_process(
        ids: &[String],
        outcome: super::CloseProcessStepOutcome,
    ) -> super::CloseObservedProcessRetirement {
        super::CloseObservedProcessRetirement {
            identities: opaque_ids(ids),
            outcome,
        }
    }

    fn observed_bash(ids: &[String]) -> ScriptedObservation {
        ScriptedObservation::Bash(Ok(observed_process(
            ids,
            super::CloseProcessStepOutcome::Retired,
        )))
    }

    fn observed_terminal(ids: &[String]) -> ScriptedObservation {
        ScriptedObservation::Terminal(Ok(observed_process(
            ids,
            super::CloseProcessStepOutcome::Retired,
        )))
    }

    fn observed_browser(ids: &[String]) -> ScriptedObservation {
        ScriptedObservation::Browser(Ok(observed_process(
            ids,
            super::CloseProcessStepOutcome::Retired,
        )))
    }

    fn observed_tmux(
        resources: &[super::RetiredResourceIdentity],
        outcome: super::RetirementOutcome,
    ) -> ScriptedObservation {
        ScriptedObservation::Tmux(Ok(super::CloseObservedTmuxRetirement {
            resources: resources.to_vec(),
            outcome,
        }))
    }

    #[derive(Default)]
    struct ScriptedProcessStepExecutor {
        observations: std::collections::VecDeque<ScriptedObservation>,
        calls: Vec<ProcessStepCall>,
    }

    impl ScriptedProcessStepExecutor {
        fn new(observations: impl IntoIterator<Item = ScriptedObservation>) -> Self {
            Self {
                observations: observations.into_iter().collect(),
                calls: Vec::new(),
            }
        }

        fn next(&mut self, call: ProcessStepCall) -> ScriptedObservation {
            self.calls.push(call);
            self.observations
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected {call:?} effect"))
        }

        fn assert_finished(&self) {
            assert!(
                self.observations.is_empty(),
                "unconsumed scripted observations"
            );
        }
    }

    #[async_trait::async_trait]
    impl super::CloseProcessStepExecutor for ScriptedProcessStepExecutor {
        async fn retire_bash(
            &mut self,
        ) -> Result<super::CloseObservedProcessRetirement, super::CloseProcessStepFailure> {
            match self.next(ProcessStepCall::Bash) {
                ScriptedObservation::Bash(observation) => observation,
                other => panic!("unexpected scripted observation: {other:?}"),
            }
        }

        async fn retire_tmux(
            &mut self,
        ) -> Result<super::CloseObservedTmuxRetirement, (super::RetirementFailureReason, String)>
        {
            match self.next(ProcessStepCall::Tmux) {
                ScriptedObservation::Tmux(observation) => observation,
                other => panic!("unexpected scripted observation: {other:?}"),
            }
        }

        async fn retire_terminal(
            &mut self,
        ) -> Result<super::CloseObservedProcessRetirement, super::CloseProcessStepFailure> {
            match self.next(ProcessStepCall::Terminal) {
                ScriptedObservation::Terminal(observation) => observation,
                other => panic!("unexpected scripted observation: {other:?}"),
            }
        }

        async fn retire_browser(
            &mut self,
        ) -> Result<super::CloseObservedProcessRetirement, super::CloseProcessStepFailure> {
            match self.next(ProcessStepCall::Browser) {
                ScriptedObservation::Browser(observation) => observation,
                other => panic!("unexpected scripted observation: {other:?}"),
            }
        }
    }

    /// Bash observation reporting the fixture's exact expected identities for a
    /// scope, so the runner's identity check passes.
    fn retired(fixture: &ProcessStepFixture, index: usize) -> ScriptedObservation {
        observed_bash(&fixture.identities[index].bash)
    }

    fn successful_process_steps(
        fixture: &ProcessStepFixture,
        index: usize,
    ) -> Vec<ScriptedObservation> {
        vec![
            retired(fixture, index),
            observed_tmux(&fixture.tmux[index], super::RetirementOutcome::Retired),
            observed_terminal(&fixture.identities[index].terminal),
            observed_browser(&fixture.identities[index].browser),
        ]
    }

    fn process_resource(
        kind: super::RetiredResourceKind,
        scope: &super::WorkScopeId,
    ) -> (String, super::RetiredResourceIdentity) {
        let identity = format!("{}-{}", kind.as_str(), scope.as_str());
        (identity.clone(), super::opaque_resource(kind, identity))
    }

    async fn mark_process_step_scope_allocated(
        db: &crate::db::Database,
        scope: &super::WorkScopeId,
        suffix: &str,
    ) -> phoenix_core::domain::close::WorktreeIdentity {
        use phoenix_core::domain::close::{
            GitPathIdentity, WorktreeFingerprint, WorktreeId, WorktreeIdentity,
        };

        let path = format!("/inert/process-step/{suffix}");
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse(format!("process-step-worktree-{suffix}")).unwrap(),
            WorktreeFingerprint::parse(format!("process-step-fingerprint-{suffix}")).unwrap(),
            GitPathIdentity::from_bytes(path.as_bytes().to_vec()),
        );
        sqlx::query(
            "UPDATE work_scopes
             SET environment_kind='allocated_worktree', worktree_path=?1, worktree_id=?2,
                 worktree_fingerprint=?3, branch_name=?4, base_branch='main'
             WHERE id=?5",
        )
        .bind(path)
        .bind(identity.id().as_str())
        .bind(identity.fingerprint().as_str())
        .bind(format!("process-step-branch-{suffix}"))
        .bind(scope.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        identity
    }

    async fn process_step_fixture(two_scopes: bool) -> ProcessStepFixture {
        process_step_fixture_with_worktree(two_scopes, None).await
    }

    #[allow(clippy::too_many_lines)]
    async fn process_step_fixture_with_worktree(
        two_scopes: bool,
        live_worktree: Option<&Path>,
    ) -> ProcessStepFixture {
        use phoenix_core::domain::close::{CloseOwnedResourceInventory, TranscriptConversationId};
        use std::collections::BTreeSet;
        use std::sync::Arc;

        let db = crate::db::Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("process-steps", "process-steps", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut scopes = vec![source.attached_work_scope_id.clone().unwrap()];
        let mut worktrees = vec![mark_process_step_scope_allocated(&db, &scopes[0], "first").await];
        if let Some(path) = live_worktree {
            let identity = inspection_identity(path);
            sqlx::query("UPDATE work_scopes SET worktree_path=?1, worktree_id=?2, worktree_fingerprint=?3 WHERE id=?4")
                .bind(path.to_str().unwrap())
                .bind(identity.id().as_str())
                .bind(identity.fingerprint().as_str())
                .bind(scopes[0].as_str())
                .execute(db.pool()).await.unwrap();
            worktrees[0] = identity;
        }
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
            let later_scope = super::WorkScopeId::parse("z-process-step-later-scope").unwrap();
            sqlx::query("INSERT INTO work_scopes (id, authority_kind, lifecycle, environment_kind, cwd, created_at, updated_at) SELECT ?1, 'work', 'active', 'unowned_cwd', '/tmp', created_at, updated_at FROM work_scopes WHERE id=?2")
                .bind(later_scope.as_str()).bind(scopes[0].as_str()).execute(db.pool()).await.unwrap();
            sqlx::query("UPDATE conversations SET work_scope_id=?1 WHERE id=?2")
                .bind(later_scope.as_str())
                .bind(&latest.id)
                .execute(db.pool())
                .await
                .unwrap();
            worktrees.push(mark_process_step_scope_allocated(&db, &later_scope, "later").await);
            scopes.push(later_scope);
            latest_id = latest.id;
        }
        scopes.sort();
        let run =
            super::CloseRunRef::initial(super::CloseAttemptId::parse("process-step-run").unwrap());
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
        scopes = db
            .list_close_attempt_scopes(run.attempt_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .map(|captured| captured.scope)
            .collect();
        scopes.sort();
        let snapshot = if live_worktree.is_some() {
            let (snapshot, losses) = super::inspect_worktree(&worktrees[0]).await.unwrap();
            assert!(losses.is_empty());
            snapshot
        } else {
            super::CloseRetirementSnapshot::parse(
                "process-step-generation",
                "process-step-fingerprint",
            )
            .unwrap()
        };
        db.replace_close_inspection(crate::db::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: scopes
                .iter()
                .cloned()
                .map(|scope| crate::db::ReplaceCloseInspectionScopeRequest {
                    scope,
                    snapshot: snapshot.clone(),
                    losses: vec![],
                })
                .collect(),
        })
        .await
        .unwrap();

        let snapshot = db
            .get_close_obligation(run.attempt_id.as_str())
            .await
            .unwrap()
            .snapshot()
            .cloned()
            .unwrap();

        let mut identities = Vec::new();
        let mut bash = Vec::new();
        let mut tmux = Vec::new();
        let mut terminal = Vec::new();
        let mut inventories = Vec::new();
        for (scope_index, scope) in scopes.iter().enumerate() {
            let (bash_id, bash_resource) =
                process_resource(super::RetiredResourceKind::BashProcessGroup, scope);
            let (tmux_id, tmux_resource) =
                process_resource(super::RetiredResourceKind::TmuxServer, scope);
            let (terminal_id, terminal_resource) =
                process_resource(super::RetiredResourceKind::PtySession, scope);
            let (browser_id, _browser_resource) =
                process_resource(super::RetiredResourceKind::BrowserSession, scope);
            identities.push(super::CloseProcessStepIdentities {
                bash: vec![bash_id.clone()],
                terminal: vec![terminal_id.clone()],
                browser: vec![browser_id.clone()],
            });
            bash.push(bash_resource.clone());
            tmux.push(vec![tmux_resource.clone()]);
            terminal.push(terminal_resource.clone());
            inventories.push(crate::db::CaptureCloseRetirementInventoryScopeRequest {
                scope: scope.clone(),
                inventory: CloseOwnedResourceInventory {
                    worktree: Some(worktrees[scope_index].clone()),
                    work_scopes: BTreeSet::new(),
                    bash_process_groups: BTreeSet::from([
                        phoenix_core::domain::close::OpaqueIdentity::parse(bash_id).unwrap(),
                    ]),
                    tmux_servers: BTreeSet::from([
                        phoenix_core::domain::close::OpaqueIdentity::parse(tmux_id).unwrap(),
                    ]),
                    pty_sessions: BTreeSet::from([
                        phoenix_core::domain::close::OpaqueIdentity::parse(terminal_id).unwrap(),
                    ]),
                    browser_sessions: BTreeSet::from([
                        phoenix_core::domain::close::OpaqueIdentity::parse(browser_id).unwrap(),
                    ]),
                    equivalent_live_resources: BTreeSet::new(),
                },
            });
            assert_eq!(
                bash_resource.identity().value(),
                identities.last().unwrap().bash[0]
            );
        }
        db.capture_close_retirement_inventory(crate::db::CaptureCloseRetirementInventoryRequest {
            attempt_id: run.attempt_id.clone(),
            snapshot: snapshot.clone(),
            scopes: inventories,
        })
        .await
        .unwrap();

        let manager = super::RuntimeManager::new(
            db,
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "process step tests".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        );
        ProcessStepFixture {
            manager,
            run,
            snapshot,
            scopes,
            identities,
            bash,
            tmux,
            terminal,
        }
    }

    type NativeRetryFixture = (
        ProcessStepFixture,
        super::StoppedCloseOriginalRetirementProof,
        Vec<(super::CloseRetirementTarget, super::CloseRetirementSnapshot)>,
    );

    async fn stopped_native_retry_fixture() -> NativeRetryFixture {
        stopped_native_retry_fixture_with_worktree(None).await
    }

    async fn stopped_native_retry_fixture_with_worktree(
        live_worktree: Option<&Path>,
    ) -> NativeRetryFixture {
        stopped_native_retry_fixture_with_worktree_at_g2(live_worktree, false).await
    }

    #[allow(clippy::too_many_lines)] // Keep the supported G1-to-G2 fixture journey together.
    async fn stopped_native_retry_fixture_with_worktree_at_g2(
        live_worktree: Option<&Path>,
        advance_to_successor_inspection: bool,
    ) -> NativeRetryFixture {
        use std::collections::BTreeSet;

        // Keep the initial run running while any successor inspection is recorded.
        let mut fixture = process_step_fixture_with_worktree(false, live_worktree).await;
        // Isolate this Git identity/drift journey from ambient runner processes.
        // The same per-manager fixture already controls deletion checkpoints.
        fixture.manager.clean_close_writer_fixture = advance_to_successor_inspection;
        let db = fixture.manager.db();
        let targets = db
            .list_close_expected_retirement_resources(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        for target in targets {
            let outcome = if target.resource.kind() == super::RetiredResourceKind::WorkScope
                || (live_worktree.is_some()
                    && target.resource.kind() == super::RetiredResourceKind::Worktree)
            {
                super::RetirementOutcome::Residual {
                    residual_reason: super::RetirementFailureReason::RemovalFailed,
                }
            } else {
                super::RetirementOutcome::Retired
            };
            db.record_close_retirement_evidence(crate::db::RecordCloseRetirementEvidenceRequest {
                attempt_id: fixture.run.attempt_id.clone(),
                snapshot: fixture.snapshot.clone(),
                scope: target.scope,
                resource: target.resource,
                outcome,
                detail: Some("native retry fixture preserves pending resources".into()),
            })
            .await
            .unwrap();
        }
        let g1_inspections = db
            .list_close_retirement_inspections(fixture.run.attempt_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .map(|inspection| (inspection.target, inspection.snapshot))
            .collect::<Vec<_>>();
        if advance_to_successor_inspection {
            let worktree = live_worktree.expect("G2 native retry fixtures need a live worktree");
            let captured = db
                .list_close_attempt_scopes(fixture.run.attempt_id.as_str())
                .await
                .unwrap();
            let Some(super::CapturedWorktreeIdentity::Resolved(identity)) =
                &captured[0].captured_worktree
            else {
                panic!("G2 reinspection requires the captured worktree identity");
            };
            assert_eq!(super::worktree_path(identity), worktree);
            db.retry_close_retirement(&fixture.run.attempt_id)
                .await
                .unwrap();
            let (g2, losses) = super::inspect_worktree(identity).await.unwrap();
            assert!(losses.is_empty());
            let g2 = super::rotate_inspection_generation(g2, Some("native-retry-G2")).unwrap();
            db.replace_close_inspection(crate::db::ReplaceCloseInspectionRequest {
                attempt_id: fixture.run.attempt_id.clone(),
                scopes: fixture
                    .scopes
                    .iter()
                    .cloned()
                    .map(|scope| crate::db::ReplaceCloseInspectionScopeRequest {
                        scope,
                        snapshot: g2.clone(),
                        losses: vec![],
                    })
                    .collect(),
            })
            .await
            .unwrap();
            let inventories = captured
                .into_iter()
                .map(|captured| {
                    crate::db::CaptureCloseRetirementInventoryScopeRequest {
                        scope: captured.scope,
                        inventory: super::CloseOwnedResourceInventory {
                            worktree: match captured.captured_worktree {
                                Some(super::CapturedWorktreeIdentity::Resolved(identity)) => {
                                    Some(identity)
                                }
                                _ => None,
                            },
                            // These process resources were retired under G1; G2 captures
                            // only the still-live worktree, not already-retired identities.
                            work_scopes: BTreeSet::default(),
                            bash_process_groups: BTreeSet::default(),
                            tmux_servers: BTreeSet::default(),
                            pty_sessions: BTreeSet::default(),
                            browser_sessions: BTreeSet::default(),
                            equivalent_live_resources: BTreeSet::default(),
                        },
                    }
                })
                .collect();
            // replace_close_inspection stores per-scope snapshots but updates the
            // obligation to their aggregate snapshot; inventory authority uses that pair.
            let aggregate_snapshot = db
                .get_close_obligation(fixture.run.attempt_id.as_str())
                .await
                .unwrap()
                .snapshot()
                .unwrap()
                .clone();
            db.capture_close_retirement_inventory(
                crate::db::CaptureCloseRetirementInventoryRequest {
                    attempt_id: fixture.run.attempt_id.clone(),
                    snapshot: aggregate_snapshot,
                    scopes: inventories,
                },
            )
            .await
            .unwrap();
        }
        db.classify_interrupted_close_run(&fixture.run)
            .await
            .unwrap();
        let original = db
            .read_stopped_close_original_retirement_proof(&fixture.run)
            .await
            .unwrap();
        if advance_to_successor_inspection {
            assert!(db
                .list_close_retirement_inspections_for_snapshot(
                    fixture.run.attempt_id.as_str(),
                    &original.snapshot,
                )
                .await
                .is_err());
        } else {
            assert_eq!(original.snapshot, fixture.snapshot);
        }
        (fixture, original, g1_inspections)
    }

    #[tokio::test]
    async fn native_retry_preserves_independent_work_scope_after_worktree_success() {
        let (fixture, original, inspections) = stopped_native_retry_fixture().await;
        let effects =
            super::native_close_retry_remaining_effects(&fixture.run, &original, &inspections)
                .unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(
            effects[0].resource.kind(),
            super::RetiredResourceKind::WorkScope,
        );
        assert_eq!(effects[0].scope, fixture.scopes[0]);
        assert!(original.outcomes.iter().any(|outcome| {
            outcome.resource.kind() == super::RetiredResourceKind::Worktree
                && outcome.outcome == super::RetirementOutcome::Retired
        }));
        let mut all_successful = original.clone();
        for outcome in &mut all_successful.outcomes {
            outcome.outcome = super::RetirementOutcome::Retired;
        }
        assert!(super::native_close_retry_remaining_effects(
            &fixture.run,
            &all_successful,
            &inspections,
        )
        .is_err());
    }

    #[tokio::test]
    async fn native_retry_refuses_missing_duplicate_and_mismatched_original_evidence() {
        let (fixture, original, inspections) = stopped_native_retry_fixture().await;
        let assess = |proof: &super::StoppedCloseOriginalRetirementProof| {
            super::native_close_retry_remaining_effects(&fixture.run, proof, &inspections)
        };
        let mut missing = original.clone();
        missing.outcomes.pop();
        assert!(assess(&missing).is_err());
        let mut duplicate = original.clone();
        duplicate.outcomes.push(duplicate.outcomes[0].clone());
        assert!(assess(&duplicate).is_err());
        let mut duplicate_target = original.clone();
        duplicate_target
            .resources
            .push(duplicate_target.resources[0].clone());
        assert!(assess(&duplicate_target).is_err());
        let mut mismatched = original.clone();
        mismatched.outcomes[0].snapshot =
            super::CloseRetirementSnapshot::parse("G2", "G2-fingerprint").unwrap();
        assert!(assess(&mismatched).is_err());
        let mut wrong_identity = original.clone();
        let worktree = wrong_identity
            .resources
            .iter_mut()
            .find(|target| target.resource.kind() == super::RetiredResourceKind::Worktree)
            .unwrap();
        let super::LossItemIdentity::Worktree(identity) = worktree.resource.identity() else {
            panic!("expected worktree identity");
        };
        let replacement = super::WorktreeIdentity::from_parts(
            identity.id().clone(),
            identity.fingerprint().clone(),
            super::GitPathIdentity::from_bytes(b"/different/repository".to_vec()),
        );
        worktree.resource = super::RetiredResourceIdentity::parse(
            super::RetiredResourceKind::Worktree,
            super::LossItemIdentity::Worktree(replacement),
        )
        .unwrap();
        assert!(assess(&wrong_identity).is_err());
        assert!(super::native_close_retry_remaining_effects(&fixture.run, &original, &[]).is_err());
        let duplicate_inspections = vec![inspections[0].clone(), inspections[0].clone()];
        assert!(super::native_close_retry_remaining_effects(
            &fixture.run,
            &original,
            &duplicate_inspections,
        )
        .is_err());
        let mut wrong_run = fixture.run.clone();
        wrong_run.ordinal = super::CloseRunOrdinal::parse(2).unwrap();
        assert!(
            super::native_close_retry_remaining_effects(&wrong_run, &original, &inspections,)
                .is_err()
        );
    }

    #[tokio::test]
    async fn native_retry_requires_original_scope_even_with_complete_remaining_receipts() {
        let (fixture, original, inspections) = stopped_native_retry_fixture().await;
        let assess = |proof: &super::StoppedCloseOriginalRetirementProof| {
            super::native_close_retry_remaining_effects(&fixture.run, proof, &inspections)
        };
        let mut missing_scope = original.clone();
        missing_scope
            .resources
            .retain(|target| target.resource.kind() != super::RetiredResourceKind::WorkScope);
        missing_scope
            .outcomes
            .retain(|outcome| outcome.resource.kind() != super::RetiredResourceKind::WorkScope);
        assert!(assess(&missing_scope)
            .unwrap_err()
            .contains("independent exact WorkScope"));

        let mut wrong_scope_identity = original.clone();
        let replacement = super::opaque_resource(
            super::RetiredResourceKind::WorkScope,
            "unrelated-scope".to_string(),
        );
        for target in &mut wrong_scope_identity.resources {
            if target.resource.kind() == super::RetiredResourceKind::WorkScope {
                target.resource = replacement.clone();
            }
        }
        for outcome in &mut wrong_scope_identity.outcomes {
            if outcome.resource.kind() == super::RetiredResourceKind::WorkScope {
                outcome.resource = replacement.clone();
            }
        }
        assert!(assess(&wrong_scope_identity).is_err());

        let mut duplicate_scope = original.clone();
        duplicate_scope
            .scopes
            .push(duplicate_scope.scopes[0].clone());
        assert!(assess(&duplicate_scope).is_err());
        let mut wrong_snapshot = original.clone();
        wrong_snapshot.resources[0].snapshot =
            super::CloseRetirementSnapshot::parse("G2", "G2-fingerprint").unwrap();
        assert!(assess(&wrong_snapshot).is_err());
        let mut not_stopped = original.clone();
        not_stopped.run.status = super::CloseRunStatus::Running;
        assert!(assess(&not_stopped).is_err());
    }

    #[tokio::test]
    async fn native_retry_refuses_original_process_local_residuals() {
        let (fixture, original, inspections) = stopped_native_retry_fixture().await;
        for kind in [
            super::RetiredResourceKind::BashProcessGroup,
            super::RetiredResourceKind::PtySession,
            super::RetiredResourceKind::BrowserSession,
        ] {
            let mut residual = original.clone();
            residual
                .outcomes
                .iter_mut()
                .find(|outcome| outcome.resource.kind() == kind)
                .unwrap()
                .outcome = super::RetirementOutcome::Residual {
                residual_reason: super::RetirementFailureReason::ManualRepairRequired,
            };
            assert!(super::native_close_retry_remaining_effects(
                &fixture.run,
                &residual,
                &inspections,
            )
            .unwrap_err()
            .contains("separate repair authority"));
        }
    }

    #[tokio::test]
    async fn native_retry_assessment_keeps_exact_snapshot_and_does_not_dispatch() {
        let (fixture, original, _) = stopped_native_retry_fixture().await;
        let assessment = fixture
            .manager
            .assess_native_close_retry(&fixture.run)
            .await
            .unwrap();
        assert_eq!(assessment.original, original);
        assert_eq!(assessment.remaining_effects.len(), 1);
        assert_eq!(
            assessment.remaining_effects[0].resource.kind(),
            super::RetiredResourceKind::WorkScope,
        );
        assert!(!assessment.safety_evidence.is_empty());
        let g2 = super::CloseRetirementSnapshot::parse(
            "different-generation",
            original.snapshot.fingerprint(),
        )
        .unwrap();
        assert!(fixture
            .manager
            .db()
            .list_close_retirement_inspections_for_snapshot(fixture.run.attempt_id.as_str(), &g2)
            .await
            .is_err());
        assert_eq!(
            fixture
                .manager
                .db()
                .get_close_run(&fixture.run)
                .await
                .unwrap(),
            original.run,
        );
        let successor = super::CloseRunRef {
            attempt_id: fixture.run.attempt_id.clone(),
            ordinal: super::CloseRunOrdinal::parse(2).unwrap(),
        };
        assert!(fixture
            .manager
            .db()
            .get_close_run(&successor)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn anchored_native_retry_assessment_preserves_writer_inspection_gap() {
        let temp = tempfile::tempdir().unwrap();
        let linked = linked_close_worktree(&temp);
        let (mut fixture, original, _) =
            stopped_native_retry_fixture_with_worktree_at_g2(Some(&linked), true).await;
        fixture.manager.clean_close_writer_fixture = true;
        let before_inspections = fixture
            .manager
            .db()
            .list_close_retirement_inspections(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        let original_run = fixture
            .manager
            .db()
            .get_close_run(&fixture.run)
            .await
            .unwrap();
        let mut observed_target = None;

        let error = fixture
            .manager
            .assess_native_close_retry_with_writer_probe(
                &fixture.run,
                |path, quarantine, tombstone, _clean_writer_fixture| {
                    super::probe_retry_worktree_writers_with(
                        path,
                        quarantine,
                        tombstone,
                        |target| {
                            observed_target = Some(target.to_path_buf());
                            Ok(super::RetryTargetObservation::Present(
                                super::ExternalWriterEvidence::Observed(
                                    super::CloseWriterObservation::InspectionGap {
                                        detector: super::CloseWriterDetector::LinuxDescriptor,
                                        pid: None,
                                        reason: super::CloseWriterGap::Unavailable,
                                    },
                                ),
                            ))
                        },
                    )
                },
            )
            .await
            .expect_err("writer inspection gap must block anchored retry assessment");
        let super::RetryAssessmentError::WriterInspectionGap(gap) = error else {
            panic!("anchored assessment lost typed writer inspection gap");
        };
        assert_eq!(gap.path, linked);
        assert_eq!(gap.detector, super::CloseWriterDetector::LinuxDescriptor);
        assert_eq!(gap.reason, super::CloseWriterGap::Unavailable);
        assert_eq!(gap.pid, None);
        assert_eq!(observed_target.as_deref(), Some(linked.as_path()));

        assert_eq!(
            fixture
                .manager
                .db()
                .get_close_run(&fixture.run)
                .await
                .unwrap(),
            original_run
        );
        assert_eq!(original.run.run, fixture.run);
        let successor = super::CloseRunRef {
            attempt_id: fixture.run.attempt_id.clone(),
            ordinal: super::CloseRunOrdinal::parse(2).unwrap(),
        };
        assert!(fixture
            .manager
            .db()
            .get_close_run(&successor)
            .await
            .is_err());
        assert_eq!(
            fixture
                .manager
                .db()
                .list_close_retirement_inspections(fixture.run.attempt_id.as_str())
                .await
                .unwrap(),
            before_inspections
        );
        assert!(linked.exists(), "anchored retry target must remain intact");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines)]
    async fn native_retry_source_actor_completes_original_worktree_and_scope_under_new_generation()
    {
        use crate::state_machine::state::{AssistantMessage, ToolCall, ToolInput};
        use phoenix_core::domain::close::CloseRunOrdinal;
        use phoenix_core::domain::db_schema::SourceToolCall;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::product_conversation::ProductConversationId;
        use phoenix_db::native_close::{NativeCloseIntent, NativeCloseRetryResult};

        async fn join_actor(mut actor: tokio::task::JoinHandle<bool>) -> bool {
            let result = tokio::time::timeout(std::time::Duration::from_secs(60), &mut actor).await;
            if result.is_err() {
                actor.abort();
                let _ = actor.await;
                panic!("bounded native retry actor did not finish");
            }
            // Inspect the route's durable diagnostics before asserting this flag.
            result.unwrap().unwrap()
        }

        async fn retained_counts(db: &crate::db::Database, attempt: &str) -> Vec<i64> {
            let mut counts = Vec::new();
            for query in [
                "SELECT COUNT(*) FROM close_runs WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM close_run_retry_successes WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM native_close_retry_plans WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM close_writer_observations WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM close_retirement_inspections WHERE attempt_id=?1",
                "SELECT COUNT(*) FROM close_retirement_inventories WHERE attempt_id=?1",
            ] {
                counts.push(
                    sqlx::query_scalar(query)
                        .bind(attempt)
                        .fetch_one(db.pool())
                        .await
                        .unwrap(),
                );
            }
            counts
        }

        let temp = tempfile::tempdir().unwrap();
        let linked = linked_close_worktree(&temp);
        let (fixture, original, snapshots) =
            stopped_native_retry_fixture_with_worktree_at_g2(Some(&linked), true).await;
        let manager = std::sync::Arc::new(fixture.manager);
        let fatal_local_authority_fence =
            std::sync::Arc::clone(&manager.fatal_local_authority_fence);
        let db = manager.db();
        let obligation = db
            .get_close_obligation(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        let g2 = obligation.snapshot().unwrap().clone();
        assert_ne!(g2, original.snapshot);
        assert!(db
            .list_close_retirement_inspections_for_snapshot(
                fixture.run.attempt_id.as_str(),
                &original.snapshot,
            )
            .await
            .is_err());
        let current_inspections_before_actor = db
            .list_close_retirement_inspections(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(current_inspections_before_actor.len(), snapshots.len());
        for (current, (original_target, original_snapshot)) in
            current_inspections_before_actor.iter().zip(&snapshots)
        {
            assert_eq!(&current.target, original_target);
            assert_ne!(&current.snapshot, original_snapshot);
        }

        let assessment = manager
            .assess_native_close_retry(&fixture.run)
            .await
            .unwrap();
        assert_eq!(assessment.original, original);
        assert_eq!(
            assessment
                .remaining_effects
                .iter()
                .map(|effect| effect.resource.kind())
                .collect::<Vec<_>>(),
            vec![
                super::RetiredResourceKind::Worktree,
                super::RetiredResourceKind::WorkScope
            ]
        );
        for effect in &assessment.remaining_effects {
            assert!(original.resources.iter().any(|resource| {
                resource.scope == effect.scope && resource.resource == effect.resource
            }));
        }

        let source_conversation = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let target: ProductConversationId = ProductConversationId::parse(
            sqlx::query_scalar::<_, String>(
                "SELECT product_conversation_id FROM close_obligations WHERE attempt_id=?1",
            )
            .bind(fixture.run.attempt_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        )
        .unwrap();
        let source = SourceToolCall {
            message_id: "retry-source-message".into(),
            tool_use_id: "retry-source-call".into(),
        };
        let intent = NativeCloseIntent::ExplicitRetry {
            target: target.clone(),
            attempt_id: fixture.run.attempt_id.clone(),
            failed_run_ordinal: fixture.run.ordinal,
        };
        let input = intent.source_input();
        let state = crate::state_machine::ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                source.message_id.clone(),
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id.clone(),
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        };
        db.update_conversation_state(&source_conversation.id, &state)
            .await
            .unwrap();
        // The bounded actor owns a real ConversationRuntime with the persisted
        // executing source state and exact Database authority; this deliberately
        // skips run_inner so startup recovery and provider dispatch are not involved.
        let (sender, actor) =
            crate::runtime::executor::creation_completion_lifecycle_tests::
                spawn_bounded_native_close_retry_actor(
                    db.clone(),
                    source_conversation.id.clone(),
                    state.clone(),
                    std::sync::Arc::clone(&fatal_local_authority_fence),
                );
        let _source_events = manager
            .inject_handle_with_event_capture_for_test(&source_conversation.id, state.clone())
            .await;
        manager
            .runtimes
            .write()
            .await
            .get_mut(&source_conversation.id)
            .unwrap()
            .acknowledged_event_tx =
            crate::runtime::cancellation_inbox::AcknowledgedEventSender::for_test_receiver(sender);
        // Exercise the production manager route: it owns lock acquisition, backend
        // selection, envelope construction and response classification.
        let (result, actor_fatal) = tokio::join!(
            manager.apply_native_close_explicit_retry(
                &source_conversation.id,
                source.clone(),
                target.clone(),
                fixture.run.attempt_id.clone(),
                CloseRunOrdinal::INITIAL,
            ),
            join_actor(actor),
        );
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                // Storage also represents a durably classified execution failure.
                // Retain its domain detail rather than hiding it behind unwrap.
                let attempt = fixture.run.attempt_id.as_str();
                let failures = db.list_close_cleanup_failures(attempt).await;
                let obligation = db.get_close_obligation(attempt).await;
                let counts = retained_counts(db, attempt).await;
                panic!(
                    "native retry route failed: {error:?}; failures: {failures:#?}; \
                     obligation: {obligation:#?}; retained counts: {counts:?}"
                );
            }
        };
        assert!(!actor_fatal, "retry actor must not exit fatally");
        let NativeCloseRetryResult::NewlyAdmitted(receipt) = result else {
            panic!("expected fresh explicit retry admission");
        };
        assert_eq!(receipt.admitted.run_ordinal.get(), 2);

        let before_replay = retained_counts(db, fixture.run.attempt_id.as_str()).await;
        let obligation_before_replay = db
            .get_close_obligation(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        // The same durable source occurrence must replay its exact receipt without
        // creating another run or dispatching either captured effect again.
        let (replay_sender, replay_actor) =
            crate::runtime::executor::creation_completion_lifecycle_tests::
                spawn_bounded_native_close_retry_actor(
                    db.clone(),
                    source_conversation.id.clone(),
                    state.clone(),
                    std::sync::Arc::clone(&fatal_local_authority_fence),
                );
        let _replay_events = manager
            .inject_handle_with_event_capture_for_test(&source_conversation.id, state.clone())
            .await;
        manager
            .runtimes
            .write()
            .await
            .get_mut(&source_conversation.id)
            .unwrap()
            .acknowledged_event_tx =
            crate::runtime::cancellation_inbox::AcknowledgedEventSender::for_test_receiver(
                replay_sender,
            );
        let (result, actor_fatal) = tokio::join!(
            manager.apply_native_close_explicit_retry(
                &source_conversation.id,
                source.clone(),
                target.clone(),
                fixture.run.attempt_id.clone(),
                CloseRunOrdinal::INITIAL,
            ),
            join_actor(replay_actor),
        );
        assert!(!actor_fatal, "retry replay actor must not exit fatally");
        let NativeCloseRetryResult::Replay(replayed_receipt) = result.unwrap() else {
            panic!("expected exact retry receipt replay");
        };
        assert_eq!(replayed_receipt, receipt);
        assert_eq!(
            retained_counts(db, fixture.run.attempt_id.as_str()).await,
            before_replay,
            "receipt replay must not append plans, effects, dispatches or writer observations"
        );
        assert_eq!(
            db.get_close_obligation(fixture.run.attempt_id.as_str())
                .await
                .unwrap(),
            obligation_before_replay
        );
        // A stale persisted source is insufficient even for a known receipt:
        // the actual source actor no longer has the executing occurrence.
        let stopped_state = crate::state_machine::ConvState::Idle;
        let (stopped_sender, stopped_actor) =
            crate::runtime::executor::creation_completion_lifecycle_tests::
                spawn_bounded_native_close_retry_actor(
                    db.clone(), source_conversation.id.clone(), stopped_state.clone(),
                    std::sync::Arc::clone(&fatal_local_authority_fence),
                );
        let _stopped_events = manager
            .inject_handle_with_event_capture_for_test(&source_conversation.id, stopped_state)
            .await;
        manager
            .runtimes
            .write()
            .await
            .get_mut(&source_conversation.id)
            .unwrap()
            .acknowledged_event_tx =
            crate::runtime::cancellation_inbox::AcknowledgedEventSender::for_test_receiver(
                stopped_sender,
            );
        let (result, actor_fatal) = tokio::join!(
            manager.apply_native_close_explicit_retry(
                &source_conversation.id,
                source.clone(),
                target.clone(),
                fixture.run.attempt_id.clone(),
                CloseRunOrdinal::INITIAL,
            ),
            join_actor(stopped_actor),
        );
        assert!(!actor_fatal, "stopped-source actor must not exit fatally");
        assert_eq!(
            result,
            Err(crate::runtime::NativeCloseApplicationError::SourceRefused)
        );
        assert_eq!(
            retained_counts(db, fixture.run.attempt_id.as_str()).await,
            before_replay
        );

        let run_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM close_runs WHERE attempt_id=?1")
                .bind(fixture.run.attempt_id.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(run_count, 2);

        assert!(
            !linked.exists(),
            "the exact clean captured worktree should be retired"
        );
        let completed = db
            .get_close_obligation(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert_ne!(completed, obligation);
        assert_eq!(db.get_close_run(&fixture.run).await.unwrap(), original.run);
        let retry_run = super::CloseRunRef {
            attempt_id: fixture.run.attempt_id.clone(),
            ordinal: receipt.admitted.run_ordinal,
        };
        assert_eq!(
            db.get_close_run(&retry_run).await.unwrap().status,
            super::CloseRunStatus::Completed
        );
        let worktree_success: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_run_retry_successes success
             JOIN close_run_retry_effects effect ON effect.attempt_id=success.attempt_id
               AND effect.run_ordinal=success.run_ordinal AND effect.ordinal=success.ordinal
             JOIN native_close_retry_plans plan ON plan.attempt_id=effect.attempt_id
               AND plan.run_ordinal=effect.run_ordinal
             WHERE success.attempt_id=?1 AND success.run_ordinal=?2
               AND effect.resource_kind='worktree'
               AND plan.inspection_generation=?3 AND plan.inspection_fingerprint=?4",
        )
        .bind(fixture.run.attempt_id.as_str())
        .bind(receipt.admitted.run_ordinal.get())
        .bind(original.snapshot.generation())
        .bind(original.snapshot.fingerprint())
        .fetch_one(db.pool())
        .await
        .unwrap();
        let scope_success: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_run_retry_successes success
             JOIN close_run_retry_effects effect ON effect.attempt_id=success.attempt_id
               AND effect.run_ordinal=success.run_ordinal AND effect.ordinal=success.ordinal
             JOIN native_close_retry_plans plan ON plan.attempt_id=effect.attempt_id
               AND plan.run_ordinal=effect.run_ordinal
             WHERE success.attempt_id=?1 AND success.run_ordinal=?2
               AND effect.resource_kind='work_scope'
               AND plan.inspection_generation=?3 AND plan.inspection_fingerprint=?4",
        )
        .bind(fixture.run.attempt_id.as_str())
        .bind(receipt.admitted.run_ordinal.get())
        .bind(original.snapshot.generation())
        .bind(original.snapshot.fingerprint())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((worktree_success, scope_success), (1, 1));
        assert_eq!(
            db.get_close_obligation(fixture.run.attempt_id.as_str())
                .await
                .unwrap()
                .snapshot(),
            Some(&g2),
        );
        let current_inspections_after_actor = db
            .list_close_retirement_inspections(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(
            current_inspections_after_actor,
            current_inspections_before_actor
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn native_retry_pending_worktree_uses_g1_under_g2_and_refuses_live_drift() {
        let temp = tempfile::tempdir().unwrap();
        let linked = linked_close_worktree(&temp);
        let (fixture, original, snapshots) =
            stopped_native_retry_fixture_with_worktree_at_g2(Some(&linked), true).await;
        let db = fixture.manager.db();
        let effects =
            super::native_close_retry_remaining_effects(&fixture.run, &original, &snapshots)
                .unwrap();
        let worktree = effects
            .iter()
            .find(|effect| effect.resource.kind() == super::RetiredResourceKind::Worktree)
            .unwrap();
        assert_eq!(effects.len(), 2);
        assert_eq!(
            snapshots
                .iter()
                .map(|(target, _)| target.scope.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([fixture.scopes[0].clone()])
        );
        assert!(!original.outcomes.iter().any(|outcome| {
            outcome.resource == worktree.resource
                && matches!(
                    outcome.outcome,
                    super::RetirementOutcome::Retired
                        | super::RetirementOutcome::AbsenceAdopted { .. }
                )
        }));
        assert!(original.outcomes.iter().any(|outcome| {
            outcome.resource == worktree.resource
                && matches!(outcome.outcome, super::RetirementOutcome::Residual { .. })
        }));
        let work_scope = effects
            .iter()
            .find(|effect| effect.resource.kind() == super::RetiredResourceKind::WorkScope)
            .unwrap();
        assert!(original.outcomes.iter().any(|outcome| {
            outcome.resource == work_scope.resource
                && matches!(outcome.outcome, super::RetirementOutcome::Residual { .. })
        }));

        // The retained G2 inspection is fresh; drift below is a separate live change.
        let g2 = db
            .get_close_obligation(fixture.run.attempt_id.as_str())
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
        let obligation = db
            .get_close_obligation(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        let inspections = db
            .list_close_retirement_inspections(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert_eq!(obligation.snapshot(), Some(&g2));
        assert_ne!(g2, original.snapshot);
        assert!(db
            .list_close_retirement_inspections_for_snapshot(
                fixture.run.attempt_id.as_str(),
                &original.snapshot,
            )
            .await
            .is_err());

        let assessment = fixture
            .manager
            .assess_native_close_retry(&fixture.run)
            .await
            .unwrap();
        assert_eq!(assessment.original, original);
        assert_eq!(assessment.remaining_effects, effects);
        std::fs::write(linked.join("new-unique-data"), "must survive\n").unwrap();
        let error = fixture
            .manager
            .assess_native_close_retry(&fixture.run)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not unchanged and reconstructible"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(linked.join("new-unique-data")).unwrap(),
            "must survive\n"
        );

        assert_eq!(
            db.get_close_obligation(fixture.run.attempt_id.as_str())
                .await
                .unwrap(),
            obligation
        );
        assert_eq!(
            db.list_close_retirement_inspections(fixture.run.attempt_id.as_str())
                .await
                .unwrap(),
            inspections
        );
        assert_eq!(
            db.read_stopped_close_original_retirement_proof(&fixture.run)
                .await
                .unwrap(),
            original
        );
        assert_eq!(db.get_close_run(&fixture.run).await.unwrap(), original.run);
        let successor = super::CloseRunRef {
            attempt_id: fixture.run.attempt_id.clone(),
            ordinal: super::CloseRunOrdinal::parse(2).unwrap(),
        };
        assert!(db.get_close_run(&successor).await.is_err());
        for effect in &effects {
            assert!(!db
                .close_retirement_resource_was_dispatched(
                    &fixture.run.attempt_id,
                    &effect.scope,
                    &original.snapshot,
                    &effect.resource,
                )
                .await
                .unwrap());
        }
    }

    async fn run_scope(
        fixture: &ProcessStepFixture,
        index: usize,
        executor: &mut ScriptedProcessStepExecutor,
    ) -> Result<(), super::CloseLeaseFailure> {
        super::run_close_process_steps(
            fixture.manager.db(),
            &fixture.run,
            &fixture.snapshot,
            &fixture.scopes[index],
            &fixture.identities[index],
            &fixture.tmux[index],
            executor,
        )
        .await
    }

    async fn assert_process_step_successes(fixture: &ProcessStepFixture, expected: usize) {
        let all_successes = fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap();
        let successes: Vec<_> = all_successes
            .iter()
            .filter(|success| {
                success.resource_kind == super::CloseProcessResourceKind::BashProcessGroup
            })
            .collect();
        assert_eq!(successes.len(), expected, "{successes:?}");
        for (scope, identities) in fixture.scopes.iter().zip(&fixture.identities) {
            for identity in &identities.bash {
                if successes.iter().any(|success| success.scope == *scope) {
                    assert!(successes.iter().any(|success| success.run == fixture.run
                        && success.scope == *scope
                        && success.resource_kind
                            == super::CloseProcessResourceKind::BashProcessGroup
                        && success.identity.as_str() == identity
                        && success.outcome == super::CloseProcessStepOutcome::Retired));
                }
            }
        }
    }

    async fn record_process_failure(
        fixture: &ProcessStepFixture,
        scope_index: usize,
        resource: super::RetiredResourceIdentity,
        detail: &str,
    ) -> String {
        fixture
            .manager
            .record_close_process_failure::<()>(
                &fixture.run,
                &fixture.scopes[scope_index],
                Some(resource),
                vec![],
                detail,
            )
            .await
            .unwrap_err()
    }

    async fn assert_one_failure(fixture: &ProcessStepFixture) {
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
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn dropped_live_completion_retains_original_lease_without_dispatch() {
        use super::lease_custody::{AcquisitionHook, CompletionHook};
        use std::sync::Arc;
        use tokio::sync::Notify;

        // The fixture seals inert identities, but owns no live processes. Only
        // acquire an empty live scope, with tmux explicitly unable to spawn.
        let mut fixture = process_step_fixture(false).await;
        fixture.manager.tmux_registry =
            Arc::new(phoenix_tools::tmux::registry::TmuxRegistry::with_backend(
                PathBuf::from("/inert/completion-cancellation-tmux"),
                phoenix_tools::tmux::fake_backend::FakeTmuxBackend::unavailable(),
                None,
            ));
        let manager = &fixture.manager;
        let scope = &fixture.scopes[0];
        let key = (fixture.run.attempt_id.as_str().to_string(), scope.clone());
        let registries = manager.close_lease_registries();
        let hook = CompletionHook {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        let generations = |lease: &super::CloseResourceLease| {
            (
                lease.bash.generation(),
                lease.tmux.generation(),
                lease.terminal.generation(),
                lease.browser.generation(),
            )
        };
        let original = {
            let mut store = manager.close_retirement_leases.lock().await;
            assert!(store
                .acquire(key.clone(), registries.clone(), None, None)
                .await
                .unwrap()
                .is_empty());
            store.completion_hook = Some(hook.clone());
            generations(store.complete.get(&key).unwrap())
        };

        {
            // Poll the actual manager wrapper, not the process-step helper or a
            // spawned task. Leaving this block drops the borrowed future while
            // it holds the original lease at its first post-lookup await.
            let completion =
                manager.complete_close_resource_lease(&fixture.run, &fixture.snapshot, scope, &[]);
            tokio::pin!(completion);
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    () = hook.entered.notified() => {},
                    result = &mut completion => panic!("completion escaped pause: {result:?}"),
                }
            })
            .await
            .expect("live completion did not reach its pause within the watchdog");
            assert!(
                manager.close_retirement_leases.try_lock().is_err(),
                "live completion must retain the store guard across its await"
            );
        }

        {
            let mut store = manager.close_retirement_leases.try_lock().unwrap();
            assert_eq!(store.complete.len(), 1);
            let lease = store.complete.get(&key).expect("original lease retained");
            assert_eq!(generations(lease), original);
            assert!(lease.resources.is_empty());
            assert!(!store.has_acquisition(&key));

            // Reusing the lease must not start a replacement acquisition. If it
            // does, this pre-released hook fails rather than hanging the test.
            let unused_hook = AcquisitionHook {
                entered: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
                fail: true,
            };
            unused_hook.release.notify_one();
            store.acquisition_hook = Some(unused_hook);
            assert!(store
                .acquire(key.clone(), registries, None, None)
                .await
                .unwrap()
                .is_empty());
            assert!(
                store.acquisition_hook.is_some(),
                "no replacement acquisition"
            );
            assert!(!store.has_acquisition(&key));
            assert_eq!(generations(store.complete.get(&key).unwrap()), original);
        }

        let db = manager.db();
        assert!(db
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .list_close_retirement_evidence(fixture.run.attempt_id.as_str())
            .await
            .unwrap()
            .is_empty());
        let targets = db
            .list_close_expected_retirement_resources(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert!(
            !targets.is_empty(),
            "sealed following effects exercise dispatch checks"
        );
        for target in targets {
            assert!(!db
                .close_retirement_resource_was_dispatched(
                    &fixture.run.attempt_id,
                    &target.scope,
                    &target.snapshot,
                    &target.resource,
                )
                .await
                .unwrap());
        }
    }

    #[tokio::test]
    async fn process_step_all_successes_persist_and_repeat_dispatches_nothing() {
        let fixture = process_step_fixture(false).await;
        let mut first = ScriptedProcessStepExecutor::new(successful_process_steps(&fixture, 0));
        run_scope(&fixture, 0, &mut first).await.unwrap();
        first.assert_finished();
        assert_eq!(
            first.calls,
            [
                ProcessStepCall::Bash,
                ProcessStepCall::Tmux,
                ProcessStepCall::Terminal,
                ProcessStepCall::Browser
            ]
        );
        let saved = fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap();
        assert_eq!(saved.len(), 3);
        for kind in [
            super::CloseProcessResourceKind::BashProcessGroup,
            super::CloseProcessResourceKind::PtySession,
            super::CloseProcessResourceKind::BrowserSession,
        ] {
            assert!(saved.iter().any(|receipt| receipt.resource_kind == kind));
        }
        let mut repeat = ScriptedProcessStepExecutor::new([]);
        run_scope(&fixture, 0, &mut repeat).await.unwrap();
        assert!(repeat.calls.is_empty());
        assert_eq!(
            fixture
                .manager
                .db()
                .list_close_process_step_successes(&fixture.run)
                .await
                .unwrap(),
            saved
        );
    }

    #[tokio::test]
    async fn process_step_stale_bash_generation_cannot_mint_success() {
        let fixture = process_step_fixture(false).await;
        let failed_resource = fixture.bash[0].clone();
        let mut executor = ScriptedProcessStepExecutor::new([ScriptedObservation::Bash(Err(
            super::CloseProcessStepFailure {
                resources: vec![failed_resource.clone()],
                reason: "bash retirement generation is stale".into(),
            },
        ))]);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(failure.to_string().contains("generation is stale"));
        assert_eq!(executor.calls, [ProcessStepCall::Bash]);
        executor.assert_finished();
        assert_process_step_successes(&fixture, 0).await;
        let error =
            record_process_failure(&fixture, 0, failed_resource.clone(), &failure.to_string())
                .await;
        assert!(error.contains("generation is stale"), "{error}");
        assert_one_failure(&fixture).await;
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
            .any(|item| {
                item.resource == failed_resource
                    && item.disposition == CloseCleanupResourceDisposition::Failed
            }));
    }

    #[tokio::test]
    async fn process_step_bash_success_survives_tmux_failure() {
        let fixture = process_step_fixture(false).await;
        let mut executor = ScriptedProcessStepExecutor::new([
            retired(&fixture, 0),
            ScriptedObservation::Tmux(Err((
                super::RetirementFailureReason::RemovalFailed,
                "tmux teardown failed".into(),
            ))),
        ]);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(failure.to_string().contains("tmux"));
        assert_eq!(
            executor.calls,
            [ProcessStepCall::Bash, ProcessStepCall::Tmux]
        );
        executor.assert_finished();
        assert_process_step_successes(&fixture, 1).await;
        let error = fixture
            .manager
            .record_close_residual::<()>(
                &fixture.run,
                &fixture.snapshot,
                &fixture.scopes[0],
                fixture.tmux[0][0].clone(),
                super::RetirementFailureReason::RemovalFailed,
                "tmux teardown failed",
            )
            .await
            .unwrap_err();
        assert!(error.contains("tmux teardown failed"), "{error}");
        assert_one_failure(&fixture).await;
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
            .any(|remaining| { remaining.resource == fixture.bash[0] }));
    }

    #[tokio::test]
    async fn process_step_bash_and_tmux_success_survive_pty_failure() {
        let fixture = process_step_fixture(false).await;
        let observations = vec![
            retired(&fixture, 0),
            observed_tmux(&fixture.tmux[0], super::RetirementOutcome::Retired),
            ScriptedObservation::Terminal(Err(super::CloseProcessStepFailure {
                resources: vec![fixture.terminal[0].clone()],
                reason: "terminal relay remains attached".into(),
            })),
        ];
        let mut executor = ScriptedProcessStepExecutor::new(observations);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(failure.to_string().contains("terminal relay"));
        assert_eq!(
            executor.calls,
            [
                ProcessStepCall::Bash,
                ProcessStepCall::Tmux,
                ProcessStepCall::Terminal
            ]
        );
        executor.assert_finished();
        assert_process_step_successes(&fixture, 1).await;
        let evidence = fixture
            .manager
            .db()
            .list_close_retirement_evidence(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert!(evidence.iter().any(|proof| {
            proof.scope == fixture.scopes[0]
                && proof.resource == fixture.tmux[0][0]
                && proof.outcome == super::RetirementOutcome::Retired
        }));
        let error = record_process_failure(
            &fixture,
            0,
            fixture.terminal[0].clone(),
            &failure.to_string(),
        )
        .await;
        assert!(error.contains("terminal relay"), "{error}");
        assert_one_failure(&fixture).await;
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
            .any(|remaining| remaining.resource == fixture.bash[0]));
        assert!(!failures[0]
            .occurrence
            .remaining_resources
            .iter()
            .any(|remaining| { remaining.resource == fixture.tmux[0][0] }));
    }

    #[tokio::test]
    async fn process_step_earlier_scope_success_survives_later_scope_failure() {
        let fixture = process_step_fixture(true).await;
        let mut first = ScriptedProcessStepExecutor::new(successful_process_steps(&fixture, 0));
        run_scope(&fixture, 0, &mut first).await.unwrap();
        assert_eq!(
            first.calls,
            [
                ProcessStepCall::Bash,
                ProcessStepCall::Tmux,
                ProcessStepCall::Terminal,
                ProcessStepCall::Browser
            ]
        );
        first.assert_finished();

        let mut later = ScriptedProcessStepExecutor::new([
            retired(&fixture, 1),
            ScriptedObservation::Tmux(Err((
                super::RetirementFailureReason::RemovalFailed,
                "later scope tmux failed".into(),
            ))),
        ]);
        let failure = run_scope(&fixture, 1, &mut later).await.unwrap_err();
        assert!(failure.to_string().contains("tmux"));
        assert_eq!(later.calls, [ProcessStepCall::Bash, ProcessStepCall::Tmux]);
        later.assert_finished();
        assert_process_step_successes(&fixture, 2).await;
        fixture
            .manager
            .record_close_residual::<()>(
                &fixture.run,
                &fixture.snapshot,
                &fixture.scopes[1],
                fixture.tmux[1][0].clone(),
                super::RetirementFailureReason::RemovalFailed,
                "later scope tmux failed",
            )
            .await
            .unwrap_err();
        assert_one_failure(&fixture).await;
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
            .any(|remaining| { remaining.resource == fixture.bash[0] }));
        assert!(!failures[0]
            .occurrence
            .remaining_resources
            .iter()
            .any(|remaining| { remaining.resource == fixture.bash[1] }));
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
            let observations = if fail_tmux_evidence {
                vec![
                    retired(&fixture, 0),
                    observed_tmux(&fixture.tmux[0], super::RetirementOutcome::Retired),
                ]
            } else {
                vec![retired(&fixture, 0)]
            };
            let mut executor = ScriptedProcessStepExecutor::new(observations);
            let error = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("injected step persistence failure"),
                "{error}"
            );
            assert_eq!(
                executor.calls,
                if fail_tmux_evidence {
                    vec![ProcessStepCall::Bash, ProcessStepCall::Tmux]
                } else {
                    vec![ProcessStepCall::Bash]
                }
            );
            executor.assert_finished();
            assert_process_step_successes(&fixture, usize::from(fail_tmux_evidence)).await;
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
        let observations = vec![
            retired(&fixture, 0),
            observed_tmux(&fixture.tmux[0], super::RetirementOutcome::Retired),
            ScriptedObservation::Terminal(Err(super::CloseProcessStepFailure {
                resources: vec![fixture.terminal[0].clone()],
                reason: "terminal relay remains attached".into(),
            })),
        ];
        let mut executor = ScriptedProcessStepExecutor::new(observations);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert_eq!(
            executor.calls,
            [
                ProcessStepCall::Bash,
                ProcessStepCall::Tmux,
                ProcessStepCall::Terminal
            ]
        );
        executor.assert_finished();
        sqlx::query("CREATE TRIGGER reject_step_failure_event BEFORE INSERT ON coordinator_watch_events BEGIN SELECT RAISE(ABORT, 'injected failure transaction rollback'); END")
            .execute(fixture.manager.db().pool()).await.unwrap();
        let error = record_process_failure(
            &fixture,
            0,
            fixture.terminal[0].clone(),
            &failure.to_string(),
        )
        .await;
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
        assert_eq!(evidence[0].resource, fixture.tmux[0][0]);
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
    }

    #[test]
    fn verify_observed_process_identities_requires_exact_set_match() {
        let expected = vec!["bash-a".to_string(), "bash-b".to_string()];
        let observed = opaque_ids(&expected);
        assert!(super::verify_observed_process_identities(&observed, &expected).is_ok());
        // Order does not matter.
        let reordered = opaque_ids(&["bash-b".into(), "bash-a".into()]);
        assert!(super::verify_observed_process_identities(&reordered, &expected).is_ok());
        // A different member is a mismatch.
        let wrong = opaque_ids(&["bash-a".into(), "bash-c".into()]);
        assert!(super::verify_observed_process_identities(&wrong, &expected).is_err());
        // A strict subset is a mismatch.
        let partial = opaque_ids(&["bash-a".into()]);
        assert!(super::verify_observed_process_identities(&partial, &expected).is_err());
        // A duplicate in the observation is rejected.
        let duplicated = opaque_ids(&["bash-a".into(), "bash-a".into()]);
        assert!(super::verify_observed_process_identities(&duplicated, &expected).is_err());
    }

    #[test]
    fn new_tmux_receipt_resources_filters_to_observed_membership() {
        use phoenix_core::domain::close::RetiredResourceKind::TmuxServer;
        let a = super::opaque_resource(TmuxServer, "tmux-a".to_string());
        let b = super::opaque_resource(TmuxServer, "tmux-b".to_string());
        let c = super::opaque_resource(TmuxServer, "tmux-c".to_string());
        // Expected is a subset of observed; only expected resources earn receipts,
        // and none absent from the observation are minted.
        let receipts = super::new_tmux_receipt_resources(
            &[a.clone(), b.clone(), c.clone()],
            std::slice::from_ref(&a),
        )
        .unwrap();
        assert_eq!(receipts, vec![a.clone()]);
        // An expected resource not carried by the observation is a mismatch.
        assert!(super::new_tmux_receipt_resources(
            std::slice::from_ref(&a),
            std::slice::from_ref(&b)
        )
        .is_err());
        // A duplicate in the observation is rejected.
        assert!(super::new_tmux_receipt_resources(
            &[a.clone(), a.clone()],
            std::slice::from_ref(&a)
        )
        .is_err());
    }

    /// A bash observation that names a different identity than the sealed
    /// inventory must refuse the receipt, classify the failure as
    /// `IdentityNotProven`, and not run any later process step.
    #[tokio::test]
    async fn process_step_mismatched_bash_identity_refuses_receipt_and_later_steps() {
        let fixture = process_step_fixture(false).await;
        let mut executor = ScriptedProcessStepExecutor::new([
            observed_bash(&["impostor-bash-identity".to_string()]),
            observed_tmux(&fixture.tmux[0], super::RetirementOutcome::Retired),
        ]);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(
            failure.to_string().contains("do not match expected"),
            "{failure}"
        );
        // The runner stopped before tmux: only bash was dispatched.
        assert_eq!(executor.calls, [ProcessStepCall::Bash]);
        assert!(matches!(
            executor.observations.front(),
            Some(ScriptedObservation::Tmux(_))
        ));
        // No receipt for any kind.
        assert_process_step_successes(&fixture, 0).await;
        assert!(fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap()
            .is_empty());
        // Re-running still refuses and leaves no receipt.
        let mut repeat = ScriptedProcessStepExecutor::new([observed_bash(&[
            "impostor-bash-identity".to_string(),
        ])]);
        run_scope(&fixture, 0, &mut repeat).await.unwrap_err();
        assert!(fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap()
            .is_empty());
    }

    /// A terminal observation carrying a strict subset of the sealed identities
    /// is a partial observation: no receipt, and the browser step never runs.
    #[tokio::test]
    async fn process_step_partial_terminal_identity_refuses_receipt_and_later_steps() {
        let fixture = process_step_fixture(false).await;
        let mut executor = ScriptedProcessStepExecutor::new([
            retired(&fixture, 0),
            observed_tmux(&fixture.tmux[0], super::RetirementOutcome::Retired),
            // An empty observation is a strict subset of the single expected id.
            observed_terminal(&[]),
            observed_browser(&fixture.identities[0].browser),
        ]);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(
            failure.to_string().contains("do not match expected"),
            "{failure}"
        );
        assert_eq!(
            executor.calls,
            [
                ProcessStepCall::Bash,
                ProcessStepCall::Tmux,
                ProcessStepCall::Terminal
            ]
        );
        // Browser was never dispatched.
        assert!(matches!(
            executor.observations.front(),
            Some(ScriptedObservation::Browser(_))
        ));
        // Bash + tmux already persisted; terminal and browser did not.
        let successes = fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap();
        assert!(successes
            .iter()
            .all(|receipt| receipt.resource_kind
                == super::CloseProcessResourceKind::BashProcessGroup));
    }

    /// A duplicated identity in the observation is refused even when it covers
    /// the expected set, so no receipt is minted and no later step runs.
    #[tokio::test]
    async fn process_step_duplicate_browser_identity_refuses_receipt() {
        let fixture = process_step_fixture(false).await;
        let mut duplicated = fixture.identities[0].browser.clone();
        duplicated.push(fixture.identities[0].browser[0].clone());
        let mut executor = ScriptedProcessStepExecutor::new([
            retired(&fixture, 0),
            observed_tmux(&fixture.tmux[0], super::RetirementOutcome::Retired),
            observed_terminal(&fixture.identities[0].terminal),
            observed_browser(&duplicated),
        ]);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(failure.to_string().contains("duplicate"), "{failure}");
        executor.assert_finished();
        let successes = fixture
            .manager
            .db()
            .list_close_process_step_successes(&fixture.run)
            .await
            .unwrap();
        assert!(!successes.iter().any(
            |receipt| receipt.resource_kind == super::CloseProcessResourceKind::BrowserSession
        ));
    }

    /// A tmux observation that omits an expected (pending) resource is refused
    /// as `IdentityNotProven`: no evidence minted, no later step runs.
    #[tokio::test]
    async fn process_step_tmux_observation_missing_expected_resource_refuses_receipt() {
        let fixture = process_step_fixture(false).await;
        let mut executor = ScriptedProcessStepExecutor::new([
            retired(&fixture, 0),
            // The executor observed no tmux resource, but one is expected.
            observed_tmux(&[], super::RetirementOutcome::Retired),
            observed_terminal(&fixture.identities[0].terminal),
        ]);
        let failure = run_scope(&fixture, 0, &mut executor).await.unwrap_err();
        assert!(failure.to_string().contains("tmux"), "{failure}");
        assert_eq!(
            executor.calls,
            [ProcessStepCall::Bash, ProcessStepCall::Tmux]
        );
        assert!(matches!(
            executor.observations.front(),
            Some(ScriptedObservation::Terminal(_))
        ));
        let evidence = fixture
            .manager
            .db()
            .list_close_retirement_evidence(fixture.run.attempt_id.as_str())
            .await
            .unwrap();
        assert!(evidence.is_empty());
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
    #[tokio::test]
    async fn macos_disposable_detached_worktree_retires_with_real_writer_scans() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let repository = root.join("repository");
        let target = root.join("detached");
        initialize_repository(&repository);
        run_git(
            &repository,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                target.to_str().unwrap(),
                "HEAD",
            ],
        );
        let identity = inspection_identity(&target);
        let confirmed = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = exact_worktree_administrative_dir(&target, &common).unwrap();
        let mut effects = injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning));
        // Keep persistence and fsmonitor fixture-local; use the actual host
        // descriptor, cwd, and mapping probes at the normal removal boundary.
        effects.observe_open_descriptors = Box::new(super::quarantine_has_open_descriptors_until);
        effects.observe_external_writer = Box::new(|path, deadline| {
            Ok(super::quarantine_has_external_writer_until(path, deadline)?.blocks_retirement())
        });
        let outcome = super::quarantine_and_remove_exact_worktree_with_effects(
            &identity,
            &confirmed,
            admin.clone(),
            observe_administrative_dir_incarnation(&admin).unwrap(),
            None,
            |_, _, _| Ok(()),
            |_| {},
            effects,
        )
        .await
        .unwrap();
        match outcome {
            ExactWorktreeRemoval::Retired => {}
            ExactWorktreeRemoval::StopFailed { detail }
            | ExactWorktreeRemoval::ReinspectionRequired { detail }
            | ExactWorktreeRemoval::Residual { detail } => panic!("retirement blocked: {detail}"),
        }
        assert!(!target.exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
        assert!(!admin.exists());
        let listing = run_git_output(&repository, &["worktree", "list", "--porcelain"]);
        assert!(!listing.contains(target.to_str().unwrap()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_disposable_writer_scan_finds_same_user_process_cwd() {
        use std::process::Command;
        use std::time::{Duration, Instant};

        let target = tempfile::tempdir().unwrap();
        let mut writer = Command::new("/bin/sleep")
            .arg("30")
            .current_dir(target.path())
            .spawn()
            .unwrap();
        let evidence = super::quarantine_has_process_cwd_mac_until(
            target.path(),
            Instant::now() + Duration::from_secs(10),
        );
        let _ = writer.kill();
        let _ = writer.wait();
        assert!(
            matches!(
                &evidence,
                Ok(super::ExternalWriterEvidence::Observed(
                    super::CloseWriterObservation::Authority {
                        authority: super::CloseWriterAuthority::WorkingDirectory,
                        process,
                        ..
                    }
                )) if process.pid == writer.id()
            ),
            "same-user process cwd should block retirement: {evidence:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn full_descriptor_buffer_requires_larger_inventory() {
        assert!(super::descriptor_inventory_may_be_truncated(4096, 4096));
        assert!(super::descriptor_inventory_may_be_truncated(8192, 4096));
        assert!(!super::descriptor_inventory_may_be_truncated(4080, 4096));
    }

    #[test]
    fn macos_process_inventory_failures_are_absent_evidence() {
        assert_eq!(super::macos_uid_pids_with(501, |_, _, _, _| 0), None);
        assert_eq!(super::macos_uid_pids_with(501, |_, _, _, _| -1), None);
        assert_eq!(super::macos_uid_pids_with(501, |_, _, _, _| 3), None);
    }

    #[test]
    fn selected_macos_pid_with_changed_uid_fails_closed_as_process_changed() {
        assert_eq!(super::macos_selected_process_uid(501, 501), Ok(()));
        assert_eq!(
            super::macos_selected_process_uid(502, 501),
            Err(super::CloseWriterGap::ProcessChanged)
        );
    }

    #[test]
    fn macos_process_inventory_excludes_foreign_uid_before_inspection() {
        const FOREIGN_PID: i32 = 17;
        const SAME_USER_PID: i32 = 23;
        let list = |selector, uid, buffer: *mut libc::c_void, _| {
            // Model XNU: the live foreign PID is visible to the all-PID
            // selector, but its per-PID identity read would be denied.
            let values: &[i32] = match (selector, uid) {
                (1, _) => &[FOREIGN_PID, SAME_USER_PID],
                (4, 501) => &[SAME_USER_PID],
                _ => panic!("unexpected inventory selector {selector}, uid {uid}"),
            };
            // SAFETY: the scanner supplied its initial 4096-PID buffer.
            unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), buffer.cast(), values.len()) };
            i32::try_from(std::mem::size_of_val(values)).unwrap()
        };
        let pids = super::macos_uid_pids_with(501, list).unwrap();
        assert_eq!(pids, [SAME_USER_PID]);
    }

    #[test]
    fn expired_macos_process_inventory_stops_before_listing() {
        let listed = std::cell::Cell::new(false);
        let result = super::macos_uid_pids_with_until(
            501,
            |_, _, _, _| {
                listed.set(true);
                0
            },
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        );
        assert!(matches!(result, Err(super::WriterScanError::Stopped(_))));
        assert!(!listed.get());
    }

    #[test]
    fn macos_process_inventory_retries_full_buffer_and_accepts_complete_list() {
        use std::mem::size_of;

        let mut calls = 0;
        let pids = super::macos_uid_pids_with(501, |selector, uid, buffer, capacity_bytes| {
            assert_eq!(selector, 4);
            assert_eq!(uid, 501);
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

    fn run_git_output(repository: &Path, arguments: &[&str]) -> String {
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
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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

    #[cfg(unix)]
    #[tokio::test]
    async fn shared_close_inspection_distinguishes_absence_from_quarantine_probe_failure() {
        use phoenix_core::domain::close::{CapturedWorktreeIdentity, RetirementFailureReason};
        use phoenix_core::work_scope::WorkScopeId;
        use std::os::unix::{ffi::OsStrExt as _, fs::symlink};

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("captured");
        let scope = WorkScopeId::parse("inspection-test-scope").unwrap();
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("inspection-test-worktree").unwrap(),
            WorktreeFingerprint::parse("inspection-test-fingerprint").unwrap(),
            GitPathIdentity::from_bytes(path.as_os_str().as_bytes().to_vec()),
        );
        assert!(matches!(
            super::inspect_captured_close_worktree(
                &scope,
                CapturedWorktreeIdentity::Resolved(identity.clone()),
                None,
            )
            .await,
            Ok(super::CapturedCloseWorktreeInspection::Absent(observed)) if observed == identity
        ));

        let quarantine = super::worktree_quarantine_path(&identity).unwrap();
        symlink(&quarantine, &quarantine).unwrap();
        assert!(matches!(
            super::inspect_captured_close_worktree(
                &scope,
                CapturedWorktreeIdentity::Resolved(identity),
                None,
            )
            .await,
            Err(super::CapturedCloseWorktreeInspectionFailure {
                reason: RetirementFailureReason::ManualRepairRequired,
                ..
            })
        ));
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

        let outcome = super::quarantine_and_remove_exact_worktree_with_effects(
            &identity,
            &confirmed,
            administrative_dir.clone(),
            observe_administrative_dir_incarnation(&administrative_dir).unwrap(),
            None,
            |_, _, _| Ok(()),
            |_| {},
            injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning)),
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
        let quarantine = worktree_quarantine_path(&identity).unwrap();
        let late_path = linked.join("late-write");
        let common = super::exact_worktree_common_git_dir(&linked).unwrap();
        let administrative = super::exact_worktree_administrative_dir(&linked, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative).unwrap();
        let result = super::quarantine_and_remove_exact_worktree_with_effects(
            &identity,
            &snapshot,
            administrative,
            incarnation,
            None,
            |_, _, _| Ok(()),
            move |original| {
                std::fs::create_dir(original).unwrap();
                std::fs::write(original.join("late-write"), "must survive\n").unwrap();
            },
            injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning)),
        )
        .await
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

        assert_eq!(
            std::fs::read_to_string(quarantine.join("confirmed-loss")).unwrap(),
            "confirmed\n"
        );
        assert!(quarantine.join(".git").exists());
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
        let common = super::exact_worktree_common_git_dir(&quarantine).unwrap();
        let administrative =
            super::exact_worktree_administrative_dir(&quarantine, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative).unwrap();
        // Resuming from quarantine with a post-crash write that changed the
        // snapshot: the production reinspection precondition rejects deletion
        // before any descriptor/writer scan, so inject deterministic effects
        // rather than stopping the real fsmonitor daemon.
        let outcome = super::quarantine_and_remove_exact_worktree_with_effects(
            &identity,
            &confirmed,
            administrative,
            incarnation,
            None,
            |_, _, _| panic!("must not bind a tombstone for a changed resumed quarantine"),
            |_| panic!("must not re-quarantine a changed resumed worktree"),
            injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning)),
        )
        .await
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
    #[test]
    fn common_directory_registry_listing_does_not_resolve_a_nested_dot_git() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        assert!(!common.join(".git").exists());
        let output = super::list_registered_worktrees_in_common_directory_until(
            &common,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap();
        assert!(output.status.success());
        let canonical_target = target.canonicalize().unwrap();
        assert!(output.stdout.split(|byte| *byte == 0).any(|field| {
            field
                .strip_prefix(b"worktree ")
                .map(super::path_buf_from_git_bytes)
                .and_then(|path| path.canonicalize().ok())
                .as_ref()
                == Some(&canonical_target)
        }));
    }

    #[test]
    fn expired_common_directory_registry_listing_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("unaccessed-common-directory");
        let error = super::list_registered_worktrees_in_common_directory_until(
            &nonexistent,
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert!(!nonexistent.exists());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn expired_prepared_worktree_refuses_before_first_teardown_effect() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let snapshot = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
        let mut prepared = super::prepare_physical_worktree_removal_until(
            &identity,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap()
        .unwrap();
        prepared.deadline = std::time::Instant::now();
        let effects = injected_close_worktree_effects(|_| {
            panic!("expired preparation must refuse before the first teardown effect")
        });

        let result = tokio::task::spawn_blocking(move || {
            super::inspect_and_remove_prepared_worktree_with_hook_and_plan_until(
                &tokio::runtime::Handle::current(),
                prepared,
                &snapshot,
                &admin,
                &incarnation,
                None,
                |_, _, _| Ok(()),
                |_| {},
                effects,
            )
        })
        .await
        .unwrap();
        let Err(error) = result else {
            panic!("expired preparation unexpectedly reached a retirement outcome");
        };
        assert!(error.contains("deadline"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn prepared_worktree_core_holds_existing_repository_lock_through_effect_hook() {
        use fs2::FileExt as _;

        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let snapshot = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
        let prepared = super::prepare_physical_worktree_removal_until(
            &identity,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap()
        .unwrap();
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(common.join("phoenix-creation.lock"))
            .unwrap();
        let effect_contender = contender.try_clone().unwrap();
        let effects = injected_close_worktree_effects(move |_| {
            assert!(effect_contender.try_lock_exclusive().is_err());
            Err("fixture stopped at first teardown effect".to_string())
        });

        let result = tokio::task::spawn_blocking(move || {
            super::inspect_and_remove_prepared_worktree_with_hook_and_plan_until(
                &tokio::runtime::Handle::current(),
                prepared,
                &snapshot,
                &admin,
                &incarnation,
                None,
                |_, _, _| Ok(()),
                |_| {},
                effects,
            )
        })
        .await
        .unwrap();
        let super::ExactWorktreeRemoval::StopFailed { detail } = result.unwrap() else {
            panic!("expected the injected first teardown effect refusal");
        };
        assert!(detail.contains("fixture stopped at first teardown effect"));
        contender.try_lock_exclusive().unwrap();
        contender.unlock().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn aborting_close_waiter_keeps_original_repository_lock_until_inner_effect_returns() {
        use fs2::FileExt as _;

        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let snapshot = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let prepared = super::prepare_physical_worktree_removal_until(&identity, deadline)
            .unwrap()
            .unwrap();
        let repository_lock = prepared.repository_lock;
        let contender_path = common.join("phoenix-creation.lock");
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&contender_path)
            .unwrap();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let effects = injected_close_worktree_effects(move |_| {
            let _ = reached_tx.send(());
            if release_rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .is_err()
            {
                return Err("fixture release signal was dropped".to_string());
            }
            Err("fixture stopped at first teardown effect".to_string())
        });

        let waiter = tokio::spawn(async move {
            super::quarantine_and_remove_exact_worktree_with_repository_lock_until(
                &identity,
                &snapshot,
                admin,
                incarnation,
                None,
                |_, _, _| panic!("must not bind a deletion tombstone"),
                |_| panic!("must not quarantine after failed fsmonitor stop"),
                effects,
                deadline,
                Some(repository_lock),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
            .await
            .expect("first teardown effect was not reached")
            .expect("effect reach signal was dropped");

        waiter.abort();
        let joined = waiter.await;
        assert!(matches!(joined, Err(error) if error.is_cancelled()));
        assert!(contender.try_lock_exclusive().is_err());

        release_tx
            .send(())
            .expect("inner effect should still be waiting for release");
        drop(release_tx);
        let lock_path = contender_path;
        tokio::task::spawn_blocking(move || {
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock_path)
                .unwrap();
            lock.lock_exclusive().unwrap();
            lock.unlock().unwrap();
        })
        .await
        .expect("competing repository lock worker panicked");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn prepared_worktree_refuses_replaced_common_directory_before_teardown() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let snapshot = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
        let prepared = super::prepare_physical_worktree_removal_until(
            &identity,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap()
        .unwrap();

        // Retain every child inode, including the held lock, but replace the
        // common directory itself so Git still resolves the same path.
        let original_common = temp.path().join("original-common-directory");
        std::fs::rename(&common, &original_common).unwrap();
        std::fs::create_dir(&common).unwrap();
        for entry in std::fs::read_dir(&original_common).unwrap() {
            let entry = entry.unwrap();
            std::fs::rename(entry.path(), common.join(entry.file_name())).unwrap();
        }
        assert_eq!(
            super::exact_worktree_common_git_dir(&target).unwrap(),
            common
        );
        assert_ne!(
            observe_administrative_dir_incarnation(&common).unwrap(),
            prepared.common_git_dir_incarnation,
        );
        let effects = injected_close_worktree_effects(|_| {
            panic!("replaced repository must refuse before the first teardown effect")
        });
        let result = tokio::task::spawn_blocking(move || {
            super::inspect_and_remove_prepared_worktree_with_hook_and_plan_until(
                &tokio::runtime::Handle::current(),
                prepared,
                &snapshot,
                &admin,
                &incarnation,
                None,
                |_, _, _| Ok(()),
                |_| {},
                effects,
            )
        })
        .await
        .unwrap();
        let Err(error) = result else {
            panic!("replaced common directory unexpectedly reached a retirement outcome");
        };
        assert!(error.contains("repository binding changed"), "{error}");
        assert!(target.exists());
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

    #[test]
    fn expired_repository_lock_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("unaccessed-worktree").unwrap(),
            WorktreeFingerprint::parse("unaccessed-incarnation").unwrap(),
            GitPathIdentity::from_bytes(
                temp.path()
                    .join("not-a-repository")
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec(),
            ),
        );
        let result =
            super::prepare_physical_worktree_removal_until(&identity, std::time::Instant::now());
        let Err(error) = result else {
            panic!("expired repository lock unexpectedly acquired");
        };
        assert!(error.contains("deadline"), "{error}");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn worktree_repository_lock_uses_shared_common_directory_until_released() {
        use fs2::FileExt as _;

        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let repository = temp.path().join("repository");
        let guard = super::prepare_physical_worktree_removal_until(
            &inspection_identity(&target),
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap()
        .unwrap();
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(repository.join(".git/phoenix-creation.lock"))
            .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        drop(guard);
        contender.try_lock_exclusive().unwrap();
        contender.unlock().unwrap();
        assert!(!target.join("phoenix-creation.lock").exists());
    }

    #[test]
    fn expired_common_git_directory_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let error = super::exact_worktree_common_git_dir_until(
            &temp.path().join("not-a-repository"),
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn expired_git_directory_resolver_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let error = super::git_directory_for_worktree_until(
            &temp.path().join("not-a-repository"),
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn expired_git_directory_capture_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let error = super::capture_git_directory_bindings_until(
            &temp.path().join("not-a-repository"),
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn expired_git_directory_verification_deadline_rejects_even_empty_bindings() {
        let error = super::verify_git_directory_bindings_until(
            Path::new("not-a-repository"),
            &std::collections::BTreeMap::new(),
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
    }

    #[test]
    fn expired_administrative_deletion_deadline_preserves_live_registration() {
        let temp = tempfile::tempdir().unwrap();
        let registration = temp.path().join("registration");
        std::fs::create_dir(&registration).unwrap();
        std::fs::write(registration.join("sentinel"), b"preserve").unwrap();
        let incarnation = super::observe_administrative_dir_incarnation(&registration).unwrap();
        let error = super::remove_exact_worktree_administrative_dir_until(
            &registration,
            &incarnation,
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(registration.join("sentinel")).unwrap(),
            b"preserve"
        );
    }

    #[test]
    fn administrative_final_tombstone_prevents_false_absence_completion() {
        let temp = tempfile::tempdir().unwrap();
        let registration = temp.path().join("registration");
        std::fs::create_dir(&registration).unwrap();
        std::fs::write(registration.join("sentinel"), b"preserve").unwrap();
        let incarnation = super::observe_administrative_dir_incarnation(&registration).unwrap();
        let quarantine =
            super::administrative_dir_quarantine_path(&registration, &incarnation).unwrap();
        let root = super::administrative_dir_final_tombstone_path(&quarantine);
        std::fs::create_dir(&root).unwrap();
        std::fs::rename(&registration, root.join("object")).unwrap();

        let error = super::remove_exact_worktree_administrative_dir(&registration, &incarnation)
            .unwrap_err();

        assert!(error.contains("unresolved final tombstone"), "{error}");
        assert!(!registration.exists());
        assert!(!quarantine.exists());
        assert_eq!(
            std::fs::read(root.join("object/sentinel")).unwrap(),
            b"preserve"
        );
        // Even an empty reserved root is uncertainty, not proof of completion.
        std::fs::remove_file(root.join("object/sentinel")).unwrap();
        std::fs::remove_dir(root.join("object")).unwrap();
        let error = super::remove_exact_worktree_administrative_dir(&registration, &incarnation)
            .unwrap_err();
        assert!(error.contains("unresolved final tombstone"), "{error}");
        assert!(root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn administrative_remover_reserves_distinct_random_private_final_roots() {
        use std::os::unix::fs::MetadataExt as _;

        let temp = tempfile::tempdir().unwrap();
        let registration = temp.path().join("registration");
        let mut observed_roots = Vec::new();
        for _ in 0..2 {
            std::fs::create_dir(&registration).unwrap();
            std::fs::write(registration.join("owned"), b"remove").unwrap();
            let incarnation = observe_administrative_dir_incarnation(&registration).unwrap();
            let quarantine =
                super::administrative_dir_quarantine_path(&registration, &incarnation).unwrap();
            let legacy = super::administrative_dir_final_tombstone_path(&quarantine);
            let mut prefix = legacy.file_name().unwrap().to_os_string();
            prefix.push("-");
            super::remove_exact_worktree_administrative_dir_with_hook(
                &registration,
                &incarnation,
                |target| {
                    assert_eq!(target, quarantine);
                    let roots: Vec<_> = std::fs::read_dir(temp.path())
                        .unwrap()
                        .map(Result::unwrap)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .as_encoded_bytes()
                                .starts_with(prefix.as_encoded_bytes())
                        })
                        .map(|entry| entry.path())
                        .collect();
                    assert_eq!(roots.len(), 1);
                    let root = roots.into_iter().next().unwrap();
                    assert_ne!(root, legacy);
                    assert_eq!(root.parent(), quarantine.parent());
                    let metadata = std::fs::symlink_metadata(&root).unwrap();
                    assert!(metadata.is_dir());
                    assert_eq!(metadata.mode() & 0o077, 0);
                    let suffix = root.file_name().unwrap().as_encoded_bytes()
                        [prefix.as_encoded_bytes().len()..]
                        .to_vec();
                    assert!(uuid::Uuid::parse_str(std::str::from_utf8(&suffix).unwrap()).is_ok());
                    assert!(!observed_roots.contains(&root));
                    observed_roots.push(root);
                },
            )
            .unwrap();
            assert!(!registration.exists());
            assert!(!quarantine.exists());
            assert!(!observed_roots.last().unwrap().exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn randomized_administrative_final_residues_are_preserved_by_exact_remover() {
        let temp = tempfile::tempdir().unwrap();
        let registration = temp.path().join("absent-registration");
        let incarnation = "captured-incarnation";
        let quarantine =
            super::administrative_dir_quarantine_path(&registration, incarnation).unwrap();
        let mut roots = Vec::new();
        for kind in 0..4 {
            let root = super::fresh_administrative_dir_final_tombstone_path(&quarantine);
            assert!(!roots.contains(&root));
            assert_eq!(root.parent(), quarantine.parent());
            match kind {
                0 => {
                    super::reserve_private_tombstone(&root, "test administrative directory")
                        .unwrap();
                    std::fs::write(root.join("sentinel"), b"preserve").unwrap();
                }
                1 => super::reserve_private_tombstone(&root, "test administrative directory")
                    .unwrap(),
                2 => std::os::unix::fs::symlink(temp.path().join("absent-target"), &root).unwrap(),
                _ => std::fs::write(&root, b"preserve file").unwrap(),
            }
            let error = super::remove_exact_worktree_administrative_dir(&registration, incarnation)
                .unwrap_err();
            assert!(error.contains("unresolved final tombstone"), "{error}");
            assert!(std::fs::symlink_metadata(&root).is_ok());
            match kind {
                0 => {
                    assert_eq!(std::fs::read(root.join("sentinel")).unwrap(), b"preserve");
                    std::fs::remove_dir_all(&root).unwrap();
                }
                1 => std::fs::remove_dir(&root).unwrap(),
                2 => {
                    assert_eq!(
                        std::fs::read_link(&root).unwrap(),
                        temp.path().join("absent-target")
                    );
                    std::fs::remove_file(&root).unwrap();
                }
                _ => {
                    assert_eq!(std::fs::read(&root).unwrap(), b"preserve file");
                    std::fs::remove_file(&root).unwrap();
                }
            }
            roots.push(root);
        }
        super::remove_exact_worktree_administrative_dir(&registration, incarnation).unwrap();
    }

    #[test]
    fn administrative_cleanup_with_missing_parent_is_truly_absent() {
        let temp = tempfile::tempdir().unwrap();
        let registration = temp.path().join("absent-parent/registration");
        super::remove_exact_worktree_administrative_dir(&registration, "captured-incarnation")
            .unwrap();
        assert!(!registration.parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_administrative_parent_is_not_proof_of_absence() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        std::os::unix::fs::symlink(temp.path().join("missing"), &parent).unwrap();
        let error = super::remove_exact_worktree_administrative_dir(
            &parent.join("registration"),
            "captured-incarnation",
        )
        .unwrap_err();
        assert!(
            error.contains("cannot enumerate administrative final tombstones"),
            "{error}"
        );
        assert!(std::fs::symlink_metadata(parent)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn permissive_final_tombstone_is_rejected_before_target_move() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let root = temp.path().join("tombstone");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"preserve").unwrap();
        super::reserve_private_tombstone(&root, "test directory").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        let error = remove_identity_bound_directory(
            &target,
            Some(&root),
            "unused-identity",
            |_| panic!("must reject root before observing moved target"),
            |_| {},
            |_, _, _| panic!("must reject root before binding custody"),
            |_, _| panic!("must reject root before deletion"),
            "test directory",
        )
        .unwrap_err();
        assert!(error.contains("owner-only permissions"), "{error}");
        assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"preserve");
        assert_eq!(
            std::fs::symlink_metadata(&root).unwrap().mode() & 0o777,
            0o777
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[derive(Default)]
    struct RecordingWorktreeMutationExecutor {
        attempts: usize,
        executed: usize,
        refuse_at: Option<usize>,
        in_effect: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(unix)]
    impl super::WorktreeMutationExecutor for RecordingWorktreeMutationExecutor {
        fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
        where
            T: Send + 'static,
            F: FnOnce() -> Result<T, String> + Send + 'static,
        {
            use std::sync::atomic::Ordering::SeqCst;

            self.attempts += 1;
            // Refuse only this admission, deliberately not latching: an erroneous
            // later admission would run and be observable in the assertions.
            if self.refuse_at == Some(self.attempts) {
                return Err("test mutation admission refused".to_string());
            }
            assert!(!self.in_effect.swap(true, SeqCst));
            self.executed += 1;
            // Exercise the owned Send + 'static seam, including fd/name custody
            // and errno handling, on a thread different from the descriptor walk.
            let result = std::thread::spawn(effect)
                .join()
                .expect("mutation panicked");
            assert!(self.in_effect.swap(false, SeqCst));
            result
        }
    }

    // Synthetic admission wait: mutate a name AFTER the caller has prepared
    // its owned effect and BEFORE invoking that effect. No DB/native fixture.
    #[cfg(unix)]
    struct BoundaryReplacingExecutor {
        attempts: usize,
        before_effect: Box<dyn FnMut(usize)>,
    }

    #[cfg(unix)]
    impl super::WorktreeMutationExecutor for BoundaryReplacingExecutor {
        fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
        where
            T: Send + 'static,
            F: FnOnce() -> Result<T, String> + Send + 'static,
        {
            self.attempts += 1;
            (self.before_effect)(self.attempts);
            effect()
        }
    }

    #[cfg(unix)]
    struct BoundaryDeletionDriver(BoundaryReplacingExecutor);

    #[cfg(unix)]
    impl super::WorktreeMutationExecutor for BoundaryDeletionDriver {
        fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
        where
            T: Send + 'static,
            F: FnOnce() -> Result<T, String> + Send + 'static,
        {
            super::WorktreeMutationExecutor::execute(&mut self.0, effect)
        }
    }

    #[cfg(unix)]
    impl super::WorktreeDeletionDriver for BoundaryDeletionDriver {
        fn allows_existing_root(&self) -> bool {
            false
        }
        fn cleanup_after_failure(&self) -> bool {
            false
        }
        fn before_move(&mut self, _: &std::path::Path) -> Result<(), String> {
            Ok(())
        }
        fn bind_root(
            &mut self,
            _: &std::path::Path,
            _: (u64, u64),
            _: Option<(u64, u64)>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn observe_identity(&mut self, object: &std::path::Path) -> Result<String, String> {
            observe_administrative_dir_incarnation(object)
        }
        fn after_move(
            &mut self,
            _: &std::path::Path,
            _: &std::path::Path,
            _: Option<&super::SelfDeletionDescriptorBorrow<'_>>,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[cfg(unix)]
    #[test]
    fn admitted_unlink_revalidates_file_directory_and_symlink_private_slots() {
        for kind in ["file", "directory", "symlink"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            let displaced = temp.path().join("original");
            let external = temp.path().join("external");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(&external, b"external sentinel").unwrap();
            match kind {
                "file" => std::fs::write(root.join("entry"), b"original").unwrap(),
                "directory" => std::fs::create_dir(root.join("entry")).unwrap(),
                _ => std::os::unix::fs::symlink(&external, root.join("entry")).unwrap(),
            }
            let effect_root = root.clone();
            let effect_displaced = displaced.clone();
            let effect_external = external.clone();
            let mut executor = BoundaryReplacingExecutor {
                attempts: 0,
                before_effect: Box::new(move |attempt| {
                    if attempt != 2 {
                        return;
                    }
                    let slot = preserved_private_slot(&effect_root);
                    std::fs::rename(&slot, &effect_displaced).unwrap();
                    match kind {
                        "file" => std::fs::write(&slot, b"replacement").unwrap(),
                        "directory" => std::fs::create_dir(&slot).unwrap(),
                        _ => std::os::unix::fs::symlink(&effect_external, &slot).unwrap(),
                    }
                }),
            };
            let error = super::remove_directory_contents_at_with_hook_and_deadline_with_executor(
                &std::sync::Arc::new(open_directory(&root)),
                &mut |_, _| {},
                None,
                &mut executor,
            )
            .unwrap_err();
            assert!(error.contains("slot was replaced"), "{kind}: {error}");
            assert_eq!(executor.attempts, 2, "no later leaf or compensation");
            let slot = preserved_private_slot(&root);
            assert!(std::fs::symlink_metadata(&displaced).is_ok());
            assert!(
                std::fs::symlink_metadata(&slot).is_ok(),
                "replacement survives"
            );
            if kind == "file" {
                assert_eq!(std::fs::read(slot).unwrap(), b"replacement");
                assert_eq!(std::fs::read(displaced).unwrap(), b"original");
            }
            assert_eq!(std::fs::read(&external).unwrap(), b"external sentinel");
        }
    }

    #[cfg(unix)]
    #[test]
    fn admitted_object_and_final_root_unlinks_preserve_empty_replacements() {
        // Empty replacements make an unchecked rmdir succeed, not merely fail
        // because a sentinel happens to make the substituted directory nonempty.
        for replace_at in [3, 4] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            let root = temp.path().join("root");
            let displaced = temp.path().join("displaced");
            std::fs::create_dir(&source).unwrap();
            let identity = observe_administrative_dir_incarnation(&source).unwrap();
            let effect_root = root.clone();
            let effect_displaced = displaced.clone();
            let mut driver = BoundaryDeletionDriver(BoundaryReplacingExecutor {
                attempts: 0,
                before_effect: Box::new(move |attempt| {
                    if attempt != replace_at {
                        return;
                    }
                    let slot = if replace_at == 3 {
                        effect_root.join("object")
                    } else {
                        effect_root.clone()
                    };
                    std::fs::rename(&slot, &effect_displaced).unwrap();
                    std::fs::create_dir(&slot).unwrap();
                }),
            });
            let error = super::remove_identity_bound_directory_with_driver(
                &source,
                Some(&root),
                &identity,
                "test",
                None,
                &mut driver,
            )
            .unwrap_err();
            assert!(error.contains("slot was replaced"), "{error}");
            assert_eq!(driver.0.attempts, replace_at);
            assert!(displaced.is_dir());
            assert!(root.is_dir());
            assert_eq!(root.join("object").is_dir(), replace_at == 3);
        }
    }

    #[cfg(unix)]
    #[test]
    fn admitted_final_rename_rejects_replaced_source_root_and_occupied_object() {
        for replacement in [
            "source-directory",
            "source-symlink",
            "root-directory",
            "root-symlink",
            "occupied-object",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            let root = temp.path().join("root");
            let displaced = temp.path().join("displaced");
            let external = temp.path().join("external");
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&external).unwrap();
            std::fs::write(source.join("owned"), b"original").unwrap();
            std::fs::write(external.join("sentinel"), b"external").unwrap();
            let identity = observe_administrative_dir_incarnation(&source).unwrap();
            let effect_source = source.clone();
            let effect_root = root.clone();
            let effect_external = external.clone();
            let effect_displaced = displaced.clone();
            let mut driver = BoundaryDeletionDriver(BoundaryReplacingExecutor {
                attempts: 0,
                before_effect: Box::new(move |attempt| {
                    if attempt != 2 {
                        return;
                    }
                    if replacement == "occupied-object" {
                        // Empty directory would be overwritten by ordinary rename.
                        std::fs::create_dir(effect_root.join("object")).unwrap();
                        return;
                    }
                    let slot = if replacement.starts_with("source") {
                        &effect_source
                    } else {
                        &effect_root
                    };
                    std::fs::rename(slot, &effect_displaced).unwrap();
                    if replacement.ends_with("symlink") {
                        std::os::unix::fs::symlink(&effect_external, slot).unwrap();
                    } else {
                        std::fs::create_dir(slot).unwrap();
                    }
                }),
            });
            assert!(
                super::remove_identity_bound_directory_with_driver(
                    &source,
                    Some(&root),
                    &identity,
                    "test",
                    None,
                    &mut driver,
                )
                .is_err(),
                "{replacement}"
            );
            assert_eq!(driver.0.attempts, 2, "no compensation or later deletion");
            assert_eq!(
                std::fs::read(external.join("sentinel")).unwrap(),
                b"external"
            );
            assert!(
                !external.join("object").exists(),
                "root symlink must not redirect rename"
            );
            let original = if replacement.starts_with("source") {
                &displaced
            } else {
                &source
            };
            assert_eq!(std::fs::read(original.join("owned")).unwrap(), b"original");
            assert!(std::fs::symlink_metadata(&root).is_ok());
            if replacement == "occupied-object" {
                assert!(root.join("object").is_dir());
                assert_eq!(std::fs::read_dir(root.join("object")).unwrap().count(), 0);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn mutation_executor_refusal_stops_private_rename_and_later_unlink() {
        for refuse_at in [1, 2] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("sentinel"), b"preserve").unwrap();
            let descriptor = std::sync::Arc::new(open_directory(&root));
            let mut executor = RecordingWorktreeMutationExecutor {
                refuse_at: Some(refuse_at),
                ..Default::default()
            };

            let error = super::remove_directory_contents_at_with_hook_and_deadline_with_executor(
                &descriptor,
                &mut |_, _| {},
                None,
                &mut executor,
            )
            .unwrap_err();

            assert_eq!(error, "test mutation admission refused");
            assert_eq!(executor.attempts, refuse_at);
            assert_eq!(executor.executed, refuse_at - 1);
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
            let preserved = if refuse_at == 1 {
                root.join("sentinel")
            } else {
                preserved_private_slot(&root)
            };
            assert_eq!(std::fs::read(preserved).unwrap(), b"preserve");
        }
    }

    #[cfg(unix)]
    #[test]
    fn recursive_mutation_walk_uses_same_executor_for_every_rename_and_unlink() {
        // A single-child chain makes mutation ordering deterministic: three
        // private renames followed by three unlinks from the leaf outward.
        for refuse_at in (1..=6).map(Some).chain(std::iter::once(None)) {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
            std::fs::write(root.join("nested/deeper/owned"), b"preserve").unwrap();
            let descriptor = std::sync::Arc::new(open_directory(&root));
            let mut executor = RecordingWorktreeMutationExecutor {
                refuse_at,
                ..Default::default()
            };

            let result = super::remove_directory_contents_at_with_hook_and_deadline_with_executor(
                &descriptor,
                &mut |_, _| {},
                Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
                &mut executor,
            );

            if let Some(refused) = refuse_at {
                assert_eq!(result.unwrap_err(), "test mutation admission refused");
                assert_eq!(executor.attempts, refused);
                assert_eq!(executor.executed, refused - 1);
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
                if refused <= 4 {
                    let nested = if refused == 1 {
                        root.join("nested")
                    } else {
                        preserved_private_slot(&root)
                    };
                    let deeper = if refused <= 2 {
                        nested.join("deeper")
                    } else {
                        preserved_private_slot(&nested)
                    };
                    let leaf = if refused <= 3 {
                        deeper.join("owned")
                    } else {
                        preserved_private_slot(&deeper)
                    };
                    assert_eq!(std::fs::read(leaf).unwrap(), b"preserve");
                }
            } else {
                result.unwrap();
                assert_eq!(executor.attempts, 6);
                assert_eq!(executor.executed, 6);
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn root_object_mutations_are_admitted_separately_from_durable_bind_callbacks() {
        use std::sync::atomic::Ordering::SeqCst;

        // Reserve, move object, bind entry, unlink entry, unlink object, remove
        // root. Refusal at any leaf must stop the graph without compensating
        // cleanup or any later mutation. In particular, move refusal leaves root.
        for refuse_at in (1..=6).map(Some).chain(std::iter::once(None)) {
            let temp = tempfile::tempdir().unwrap();
            let target = temp.path().join("target");
            let root = temp.path().join("tombstone");
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join("sentinel"), b"preserve").unwrap();
            let identity = observe_administrative_dir_incarnation(&target).unwrap();
            let mut executor = RecordingWorktreeMutationExecutor {
                refuse_at,
                ..Default::default()
            };
            let in_effect = std::sync::Arc::clone(&executor.in_effect);
            let mut bound_objects = Vec::new();

            let result = super::remove_identity_bound_directory_with_deadline_with_executor(
                &target,
                Some(&root),
                &identity,
                observe_administrative_dir_incarnation,
                |_| assert!(!in_effect.load(SeqCst)),
                |_, _, object| {
                    assert!(!in_effect.load(SeqCst), "nested durable writer admission");
                    bound_objects.push(object.is_some());
                    Ok(())
                },
                |_, _| {
                    assert!(!in_effect.load(SeqCst));
                    Ok(())
                },
                "test directory",
                None,
                &mut executor,
            );

            if let Some(refused) = refuse_at {
                assert_eq!(result.unwrap_err(), "test mutation admission refused");
                assert_eq!(executor.attempts, refused);
                assert_eq!(executor.executed, refused - 1);
                assert_eq!(target.exists(), refused <= 2);
                assert_eq!(root.exists(), refused >= 2);
                assert_eq!(root.join("object").exists(), (3..=5).contains(&refused));
                if refused <= 4 {
                    let preserved = match refused {
                        1 | 2 => target.join("sentinel"),
                        3 => root.join("object/sentinel"),
                        _ => preserved_private_slot(&root.join("object")),
                    };
                    assert_eq!(std::fs::read(preserved).unwrap(), b"preserve");
                }
                let expected_bindings = match refused {
                    1 => vec![],
                    2 => vec![false],
                    _ => vec![false, true],
                };
                assert_eq!(bound_objects, expected_bindings);
            } else {
                result.unwrap();
                assert_eq!(executor.attempts, 6);
                assert_eq!(executor.executed, 6);
                assert_eq!(bound_objects, vec![false, true]);
                assert!(!target.exists());
                assert!(!root.exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn compensating_tombstone_cleanup_requires_its_own_mutation_admission() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let root = temp.path().join("tombstone");
        std::fs::create_dir(&target).unwrap();
        let mut executor = RecordingWorktreeMutationExecutor {
            refuse_at: Some(2),
            ..Default::default()
        };

        let error = super::remove_identity_bound_directory_with_deadline_with_executor(
            &target,
            Some(&root),
            "unused",
            |_| panic!("failed custody bind must not move or inspect object"),
            |_| {},
            |_, _, _| Err("test durable bind failed".to_string()),
            |_, _| panic!("failed custody bind must not delete object"),
            "test directory",
            None,
            &mut executor,
        )
        .unwrap_err();

        assert_eq!(error, "test mutation admission refused");
        assert_eq!(executor.attempts, 2);
        assert_eq!(executor.executed, 1);
        assert!(target.is_dir());
        assert!(root.is_dir());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn missing_final_source_is_refused_before_reservation_or_any_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("missing-target");
        let root = temp.path().join("tombstone");
        let mut executor = RecordingWorktreeMutationExecutor::default();
        let error = super::remove_identity_bound_directory_with_deadline_with_executor(
            &target,
            Some(&root),
            "unused",
            |_| panic!("missing source must not inspect an object"),
            |_| {},
            |_, _, _| panic!("missing source must not bind a reservation"),
            |_, _| panic!("missing source must not delete an object"),
            "test directory",
            None,
            &mut executor,
        )
        .unwrap_err();
        assert!(error.contains("cannot revalidate deletion slot"));
        assert_eq!(executor.attempts, 0);
        assert_eq!(executor.executed, 0);
        assert!(!root.exists());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn exact_admin_deletion_threads_executor_through_quarantine_and_final_deletion() {
        // Admin quarantine adds one mutation before the six root/object leaves.
        for refuse_at in (1..=7).map(Some).chain(std::iter::once(None)) {
            let temp = tempfile::tempdir().unwrap();
            let registration = temp.path().join("registration");
            std::fs::create_dir(&registration).unwrap();
            std::fs::write(registration.join("sentinel"), b"preserve").unwrap();
            let identity = observe_administrative_dir_incarnation(&registration).unwrap();
            let quarantine =
                super::administrative_dir_quarantine_path(&registration, &identity).unwrap();
            let mut executor = RecordingWorktreeMutationExecutor {
                refuse_at,
                ..Default::default()
            };

            let result =
                super::remove_exact_worktree_administrative_dir_with_hook_and_deadline_with_executor(
                    &registration,
                    &identity,
                    |_| {},
                    None,
                    &mut executor,
                );

            if let Some(refused) = refuse_at {
                assert_eq!(result.unwrap_err(), "test mutation admission refused");
                assert_eq!(executor.attempts, refused);
                assert_eq!(executor.executed, refused - 1);
                assert_eq!(registration.exists(), refused == 1);
                assert_eq!(quarantine.exists(), (2..=3).contains(&refused));
                if refused <= 3 {
                    let preserved = if refused == 1 {
                        &registration
                    } else {
                        &quarantine
                    };
                    assert_eq!(
                        std::fs::read(preserved.join("sentinel")).unwrap(),
                        b"preserve"
                    );
                }
            } else {
                result.unwrap();
                assert_eq!(executor.attempts, 7);
                assert_eq!(executor.executed, 7);
                assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn expired_directory_deletion_deadline_preserves_entries_before_inspection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("sentinel"), "preserve\n").unwrap();
        let descriptor = open_directory(&root);

        let error = super::remove_directory_contents_at_with_hook_and_deadline(
            &descriptor,
            &mut |_, _| panic!("expired deletion must not inspect an entry"),
            Some(std::time::Instant::now()),
        )
        .unwrap_err();

        assert!(error.contains("deadline"));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(std::fs::read(root.join("sentinel")).unwrap(), b"preserve\n");
    }

    #[cfg(unix)]
    #[test]
    fn deadline_directory_deletion_recurses_without_following_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let external = temp.path().join("external");
        std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("sentinel"), "preserve\n").unwrap();
        std::fs::write(root.join("nested/deeper/owned"), "remove\n").unwrap();
        std::os::unix::fs::symlink(&external, root.join("nested/external-link")).unwrap();
        let descriptor = open_directory(&root);

        super::remove_directory_contents_at_with_hook_and_deadline(
            &descriptor,
            &mut |_, _| {},
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        )
        .unwrap();

        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(
            std::fs::read(external.join("sentinel")).unwrap(),
            b"preserve\n"
        );
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
    fn native_driver_preserves_bound_root_after_failed_final_move() {
        struct Driver {
            source: std::path::PathBuf,
            displaced: std::path::PathBuf,
            effects: usize,
        }
        impl super::WorktreeMutationExecutor for Driver {
            fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
            where
                T: Send + 'static,
                F: FnOnce() -> Result<T, String> + Send + 'static,
            {
                self.effects += 1;
                effect()
            }
        }
        impl super::WorktreeDeletionDriver for Driver {
            fn allows_existing_root(&self) -> bool {
                false
            }
            fn cleanup_after_failure(&self) -> bool {
                false
            }
            fn before_move(&mut self, _: &std::path::Path) -> Result<(), String> {
                Ok(())
            }
            fn bind_root(
                &mut self,
                _: &std::path::Path,
                _: (u64, u64),
                object: Option<(u64, u64)>,
            ) -> Result<(), String> {
                assert!(object.is_none());
                // Model an external change after the reservation is bound but
                // before the native driver's one direct final rename.
                std::fs::rename(&self.source, &self.displaced).unwrap();
                Ok(())
            }
            fn observe_identity(&mut self, _: &std::path::Path) -> Result<String, String> {
                panic!("failed final move must not observe or delete an object")
            }
            fn after_move(
                &mut self,
                _: &std::path::Path,
                _: &std::path::Path,
                _: Option<&super::SelfDeletionDescriptorBorrow<'_>>,
            ) -> Result<(), String> {
                panic!("failed final move must not continue the deletion graph")
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("target");
        let displaced = temp.path().join("externally-displaced");
        let root = temp.path().join("final-root");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("sentinel"), b"preserve\n").unwrap();
        let mut driver = Driver {
            source: source.clone(),
            displaced: displaced.clone(),
            effects: 0,
        };
        let error = super::remove_identity_bound_directory_with_driver(
            &source,
            Some(&root),
            "test-identity",
            "test directory",
            None,
            &mut driver,
        )
        .unwrap_err();

        assert!(error.contains("cannot revalidate deletion slot"));
        assert_eq!(driver.effects, 2, "no compensating rmdir may be admitted");
        assert!(
            root.is_dir(),
            "bound empty final root remains recovery evidence"
        );
        assert_eq!(
            std::fs::read(displaced.join("sentinel")).unwrap(),
            b"preserve\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn expired_identity_bound_deletion_refuses_before_hook_or_tombstone_effect() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let tombstone_root = temp.path().join("tombstone");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), "preserve\\n").unwrap();

        let error = super::remove_identity_bound_directory_with_deadline(
            &target,
            Some(&tombstone_root),
            "synthetic-identity",
            |_| panic!("expired deletion must not observe repository identity"),
            |_| panic!("expired deletion must not run pre-move hook"),
            |_, _, _| panic!("expired deletion must not bind tombstone"),
            |_, _| panic!("expired deletion must not run identity callback"),
            "test directory",
            Some(std::time::Instant::now()),
        )
        .unwrap_err();

        assert!(error.contains("deadline"));
        assert!(!tombstone_root.exists());
        assert_eq!(
            std::fs::read(target.join("sentinel")).unwrap(),
            b"preserve\\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn expired_error_cleanup_preserves_reserved_tombstone() {
        let temp = tempfile::tempdir().unwrap();
        let tombstone = temp.path().join("tombstone");
        std::fs::create_dir(&tombstone).unwrap();
        super::remove_empty_tombstone_before_deadline(&tombstone, Some(std::time::Instant::now()));
        assert!(tombstone.is_dir());
        assert_eq!(std::fs::read_dir(&tombstone).unwrap().count(), 0);
        super::remove_empty_tombstone_before_deadline(&tombstone, None);
        assert!(!tombstone.exists());
    }

    #[cfg(unix)]
    #[test]
    fn future_deadline_removes_exact_identity_bound_directory_without_following_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let tombstone_root = temp.path().join("tombstone");
        let external = temp.path().join("external");
        std::fs::create_dir_all(target.join("nested/deeper")).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::write(target.join("nested/deeper/owned"), "remove\\n").unwrap();
        std::fs::write(external.join("sentinel"), "preserve\\n").unwrap();
        std::os::unix::fs::symlink(&external, target.join("nested/external-link")).unwrap();

        super::remove_identity_bound_directory_with_deadline(
            &target,
            Some(&tombstone_root),
            "synthetic-identity",
            |path| {
                assert_eq!(path, tombstone_root.join("object"));
                Ok("synthetic-identity".to_string())
            },
            |_| {},
            |_, _, _| Ok(()),
            |_, _| Ok(()),
            "test directory",
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        )
        .unwrap();

        assert!(!target.exists());
        assert!(!tombstone_root.exists());
        assert_eq!(
            std::fs::read(external.join("sentinel")).unwrap(),
            b"preserve\\n"
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

        assert!(error.contains("slot was replaced"));
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

        assert!(error.contains("slot was replaced"));
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

    #[test]
    fn common_directory_registry_query_lists_linked_worktree_without_nested_git() {
        let temp = tempfile::tempdir().unwrap();
        let linked = linked_close_worktree(&temp);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let common = super::exact_worktree_common_git_dir_until(&linked, deadline).unwrap();
        assert!(!common.join(".git").exists());
        let listed =
            super::list_registered_worktrees_in_common_directory_until(&common, deadline).unwrap();
        assert!(listed.status.success(), "{listed:?}");
        let target = std::fs::canonicalize(&linked).unwrap();
        assert!(listed.stdout.split(|byte| *byte == 0).any(|field| {
            field
                .strip_prefix(b"worktree ")
                .map(super::path_buf_from_git_bytes)
                .and_then(|path| std::fs::canonicalize(path).ok())
                .as_ref()
                == Some(&target)
        }));
        assert!(linked.exists());
    }

    #[test]
    fn expired_common_directory_registry_query_refuses_before_directory_access() {
        let temp = tempfile::tempdir().unwrap();
        let absent = temp.path().join("not-a-git-directory");
        let error = super::list_registered_worktrees_in_common_directory_until(
            &absent,
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert!(!absent.exists());
    }

    #[cfg(unix)]
    #[test]
    fn ignored_content_changes_fingerprint_and_is_an_exact_linked_worktree_blocker() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        std::fs::write(target.join(".gitignore"), "cache/\n").unwrap();
        run_git(&target, &["add", ".gitignore"]);
        run_git(&target, &["commit", "--quiet", "-m", "ignore cache"]);
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        std::fs::create_dir(target.join("cache")).unwrap();
        std::fs::write(target.join("cache/unique"), "preserve").unwrap();
        std::fs::write(target.join("cache/another"), "also preserve").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (fresh, losses) = runtime.block_on(inspect_worktree(&identity)).unwrap();
        assert_ne!(fresh.fingerprint(), confirmed.fingerprint());
        let ignored_paths = losses
            .iter()
            .filter_map(|loss| match loss {
                CloseLossItem::IgnoredPath(path) => Some(path.as_bytes()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ignored_paths,
            [b"cache/another".as_slice(), b"cache/unique".as_slice()]
        );
        assert_eq!(losses.len(), 2);
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let administrative = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&administrative).unwrap();
        let result = super::inspect_and_remove_exact_worktree(
            runtime.handle(),
            &identity,
            &confirmed,
            &administrative,
            &incarnation,
            None,
            injected_close_worktree_effects_rejecting_scans(|_| {
                panic!("ignored data must prevent fsmonitor effects")
            }),
            |_, _, _| Ok(()),
        )
        .unwrap();
        let super::ExactWorktreeRemoval::ReinspectionRequired { detail: error } = result else {
            panic!("ignored data must require fresh confirmation before deletion");
        };
        assert!(
            error.contains("fresh confirmation is required"),
            "changed ignored inventory must refuse deletion: {error}"
        );
        assert!(target.join("cache/unique").exists());
        assert!(target.join("cache/another").exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn named_nested_submodule_reports_exact_ignored_blocker_and_changes_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let subrepo = temp.path().join("subrepo");
        initialize_repository(&subrepo);
        std::fs::write(subrepo.join(".gitignore"), "build/\n").unwrap();
        run_git(&subrepo, &["add", ".gitignore"]);
        run_git(&subrepo, &["commit", "--quiet", "-m", "ignore build"]);
        let grandrepo = temp.path().join("grandrepo");
        initialize_repository(&grandrepo);
        run_git(
            &subrepo,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                "--name",
                "logical-grandchild",
                grandrepo.to_str().unwrap(),
                "vendor/grandchild",
            ],
        );
        run_git(
            &subrepo,
            &["commit", "--quiet", "-am", "add nested submodule"],
        );
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
                "--name",
                "logical-child",
                subrepo.to_str().unwrap(),
                "deps/child",
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
                "--recursive",
                "--quiet",
            ],
        );
        let identity = inspection_identity(&target);
        let confirmed = test_worktree_snapshot(&identity, &target);
        std::fs::create_dir(target.join("deps/child/build")).unwrap();
        std::fs::write(target.join("deps/child/build/unique"), "preserve").unwrap();
        assert!(super::ensure_no_ignored_content(&target)
            .unwrap_err()
            .contains("ignored content"));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (fresh, losses) = runtime.block_on(inspect_worktree(&identity)).unwrap();
        assert_ne!(fresh.fingerprint(), confirmed.fingerprint());
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::IgnoredPath(path) if path.as_bytes() == b"deps/child/build/unique"
        )));
        assert!(target.join("deps/child/build/unique").exists());
    }

    #[cfg(unix)]
    #[test]
    fn expired_original_deadline_does_not_spawn_git_or_a_sentinel_effect() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("must-not-be-created");
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "printf invoked > \"$1\"", "deadline-sentinel"]);
        command.arg(&marker);
        let error = super::run_bounded_spawned_git(
            command,
            std::time::Instant::now(),
            "expired initial Close probe",
        )
        .unwrap_err();
        assert_eq!(error, "expired initial Close probe exceeded its deadline");
        assert!(
            !marker.exists(),
            "expired budget cannot admit a subprocess effect"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_child_waits_for_pipe_eof_until_deadline_after_direct_child_exits() {
        use std::os::unix::process::CommandExt as _;

        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("descendant-started");
        let mut command = std::process::Command::new("/bin/sh");
        command
            .args([
                "-c",
                "(/bin/sleep 60) & printf ready > \"$1\"; exit 0",
                "pipe-retention-fixture",
            ])
            .arg(&marker)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        // Direct-child exit is the causal boundary under test, not a scheduling
        // assumption within the deadline. Its descendant still owns the pipes.
        let status = child.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);

        let error =
            super::wait_bounded_child_output(child, deadline, "pipe retention probe").unwrap_err();

        assert_eq!(error, "pipe retention probe exceeded its deadline");
        assert!(status.success());
        assert!(
            marker.exists(),
            "fixture handshake confirms descendant launch"
        );
    }

    // These fixtures author only filesystem metadata and synthetic Git outputs;
    // no Git process, SQLite admission or native executor is involved.
    #[cfg(unix)]
    fn synthetic_inspection_repository() -> (tempfile::TempDir, WorktreeIdentity) {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        std::fs::write(temp.path().join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::fs::write(temp.path().join(".git/index"), b"synthetic-index").unwrap();
        // Deliberately model a repository with no HEAD reflog. A missing log is
        // legitimate; separate real-Git fixtures exercise both raw endpoints.
        assert!(!temp.path().join(".git/logs/HEAD").exists());
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("supplied-leaf-inspection").unwrap(),
            WorktreeFingerprint::parse(observe_worktree_fingerprint(temp.path()).unwrap()).unwrap(),
            GitPathIdentity::from_bytes(b"synthetic-inspection-path".to_vec()),
        );
        (temp, identity)
    }

    #[cfg(unix)]
    fn synthetic_inspection_output(stdout: Vec<u8>, success: bool) -> std::process::Output {
        synthetic_inspection_output_raw(stdout, Vec::new(), if success { 0 } else { 1 << 8 })
    }

    #[cfg(unix)]
    fn synthetic_inspection_output_raw(
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        raw_status: i32,
    ) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(raw_status),
            stdout,
            stderr,
        }
    }

    #[cfg(unix)]
    fn synthetic_inspection_succeeds(command: &std::process::Command, operation: &str) -> bool {
        operation != "Git ref backend inspection"
            && !command.get_args().any(|argument| argument == "show-ref")
    }

    #[cfg(unix)]
    #[test]
    fn supplied_leaf_full_inspection_matches_legacy_canonical_clean_snapshot() {
        let (temp, identity) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut snapshots = Vec::new();
        for oid in [
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
        ] {
            let index = format!("100644 {oid} 0\ttracked\0").into_bytes();
            let mut results = Vec::new();
            for native in [false, true] {
                let mut calls = Vec::new();
                let mut leaf = |command: std::process::Command, leaf_deadline, operation: &str| {
                    assert_eq!(leaf_deadline, deadline);
                    assert_eq!(command.get_current_dir(), Some(temp.path()));
                    if native {
                        assert!(!command
                            .get_envs()
                            .any(|(key, value)| { key == "GIT_INDEX_FILE" && value.is_some() }));
                        assert!(!command.get_args().any(|arg| arg == "update-index"));
                    }
                    calls.push(operation.to_string());
                    let stdout = match operation {
                        "Git index path inspection" => b".git/index\n".to_vec(),
                        "detached head inspection" => {
                            if command
                                .get_args()
                                .any(|argument| argument == "symbolic-ref")
                            {
                                b"refs/heads/main\n".to_vec()
                            } else {
                                b"1111111111111111111111111111111111111111\n".to_vec()
                            }
                        }
                        "detached commit reachability" => {
                            if command.get_args().any(|argument| argument == "--git-path") {
                                b".git/logs/HEAD".to_vec()
                            } else {
                                Vec::new()
                            }
                        }
                        "gitlink inspection" => index.clone(),
                        _ => Vec::new(),
                    };
                    let success = synthetic_inspection_succeeds(&command, operation);
                    Ok(synthetic_inspection_output(stdout, success))
                };
                let result = if native {
                    super::inspect_worktree_at_with_git_leaf_until(
                        &identity,
                        temp.path(),
                        deadline,
                        &mut leaf,
                    )
                } else {
                    super::inspect_worktree_at_with_policy_until(
                        &identity,
                        temp.path(),
                        deadline,
                        super::WorktreeInspectionPolicy::Legacy,
                        &mut leaf,
                    )
                }
                .unwrap();
                assert!(result.1.is_empty());
                let mut expected_calls =
                    vec!["Git status inspection", "exact ignored-path inspection"];
                if !native {
                    expected_calls.push("Git index path inspection");
                }
                expected_calls.extend([
                    "hidden-index inspection",
                    "detached head inspection",
                    "detached head inspection",
                    "Git ref backend inspection",
                    "detached commit reachability",
                    "detached commit reachability",
                    "detached commit reachability",
                    "gitlink inspection",
                ]);
                assert_eq!(calls, expected_calls);
                results.push(result.0);
            }
            // Independent byte fixture pins the pre-existing framing, including
            // the root gitlink-index record even when there are no gitlinks.
            let mut canonical = b"HEAD_SYMBOLIC\0refs/heads/main\0HEAD\0".to_vec();
            canonical.extend_from_slice(b"1111111111111111111111111111111111111111\0");
            canonical.extend_from_slice(b"DETACHED_HISTORY\0\0SUBMODULE_GITLINK_INDEX\0\0");
            canonical.extend_from_slice(&index);
            canonical.push(0);
            assert_eq!(results[0], snapshot_for(&canonical));
            assert_eq!(results[1], snapshot_for(&canonical));
            snapshots.push(results.pop().unwrap());
        }
        assert_ne!(
            snapshots[0], snapshots[1],
            "clean status alone is not a snapshot"
        );
        assert_synthetic_index_unchanged(temp.path());
    }

    #[cfg(unix)]
    #[test]
    fn supplied_leaf_inspection_refuses_unsupported_state_without_scratch_mutation() {
        let (temp, identity) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        for (stop, stdout, success, reason) in [
            (
                "Git status inspection",
                b" M tracked\0".as_slice(),
                true,
                "dirty status",
            ),
            (
                "Git status inspection",
                b"?? untracked\0".as_slice(),
                true,
                "dirty status",
            ),
            (
                "hidden-index inspection",
                b"h tracked\0".as_slice(),
                true,
                "hidden index suppression",
            ),
            (
                "hidden-index inspection",
                b"S tracked\0".as_slice(),
                true,
                "hidden index suppression",
            ),
            (
                "hidden-index inspection",
                b"s tracked\0".as_slice(),
                true,
                "hidden index suppression",
            ),
            (
                "hidden-index inspection",
                b"m tracked\0".as_slice(),
                true,
                "hidden index suppression",
            ),
            (
                "gitlink inspection",
                b"160000 1111111111111111111111111111111111111111 0\tmissing-module\0".as_slice(),
                true,
                "gitlinks",
            ),
            (
                "gitlink inspection",
                b"160000 1111111111111111111111111111111111111111 2\tconflicted-module\0"
                    .as_slice(),
                true,
                "gitlinks",
            ),
        ] {
            let refusal_stage = if stop == "Git status inspection" {
                "exact ignored-path inspection"
            } else {
                stop
            };
            let mut stopped = false;
            let error = super::inspect_worktree_at_with_git_leaf_until(
                &identity,
                temp.path(),
                deadline,
                &mut |command: std::process::Command, leaf_deadline, operation: &str| {
                    assert_eq!(leaf_deadline, deadline);
                    assert_unsupported_inspection_is_read_only(&command, stopped);
                    stopped = operation == refusal_stage;
                    if operation == stop {
                        return Ok(synthetic_inspection_output(stdout.to_vec(), success));
                    }
                    let stdout = match operation {
                        "hidden-index inspection" => b"H tracked\0".to_vec(),
                        "detached head inspection" => {
                            if command
                                .get_args()
                                .any(|argument| argument == "symbolic-ref")
                            {
                                b"refs/heads/main\n".to_vec()
                            } else {
                                b"1111111111111111111111111111111111111111\n".to_vec()
                            }
                        }
                        "detached commit reachability" => {
                            if command.get_args().any(|argument| argument == "--git-path") {
                                b".git/logs/HEAD".to_vec()
                            } else {
                                Vec::new()
                            }
                        }
                        _ => Vec::new(),
                    };
                    let success = synthetic_inspection_succeeds(&command, operation);
                    Ok(synthetic_inspection_output(stdout, success))
                },
            )
            .unwrap_err();
            assert!(stopped);
            assert!(error.contains(reason), "{error}");
            assert_synthetic_index_unchanged(temp.path());
        }
    }

    #[cfg(unix)]
    fn assert_unsupported_inspection_is_read_only(command: &std::process::Command, stopped: bool) {
        assert!(!stopped, "inspection continued after unsupported state");
        assert!(!command
            .get_envs()
            .any(|(key, value)| key == "GIT_INDEX_FILE" && value.is_some()));
        assert!(!command.get_args().any(|arg| arg == "update-index"));
    }

    #[cfg(unix)]
    fn assert_synthetic_index_unchanged(repository: &Path) {
        assert_eq!(
            std::fs::read(repository.join(".git/index")).unwrap(),
            b"synthetic-index"
        );
        assert_eq!(
            std::fs::read_dir(repository.join(".git")).unwrap().count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn shared_inspection_keeps_legacy_dirty_content_observation() {
        let (temp, identity) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let index = b"100644 1111111111111111111111111111111111111111 0\ttracked\0";
        let mut snapshots = Vec::new();
        for contents in [b"first".as_slice(), b"second".as_slice()] {
            std::fs::write(temp.path().join("tracked"), contents).unwrap();
            let mut dirty_index_observed = false;
            let (snapshot, losses) = super::inspect_worktree_at_with_policy_until(
                &identity,
                temp.path(),
                deadline,
                super::WorktreeInspectionPolicy::Legacy,
                &mut |command: std::process::Command, leaf_deadline, operation: &str| {
                    assert_eq!(leaf_deadline, deadline);
                    let stdout = match operation {
                        "Git status inspection" => b" M tracked\0".to_vec(),
                        "Git index path inspection" => b".git/index\n".to_vec(),
                        "hidden-index inspection" => b"H tracked\0".to_vec(),
                        "detached head inspection" => {
                            if command
                                .get_args()
                                .any(|argument| argument == "symbolic-ref")
                            {
                                b"refs/heads/main\n".to_vec()
                            } else {
                                b"1111111111111111111111111111111111111111\n".to_vec()
                            }
                        }
                        "detached commit reachability" => {
                            if command.get_args().any(|argument| argument == "--git-path") {
                                b".git/logs/HEAD".to_vec()
                            } else {
                                Vec::new()
                            }
                        }
                        "gitlink inspection" => index.to_vec(),
                        "dirty index inspection" => {
                            dirty_index_observed = true;
                            index.to_vec()
                        }
                        _ => Vec::new(),
                    };
                    let success = synthetic_inspection_succeeds(&command, operation);
                    Ok(synthetic_inspection_output(stdout, success))
                },
            )
            .unwrap();
            assert!(dirty_index_observed);
            assert_eq!(
                losses,
                vec![CloseLossItem::UnstagedTrackedPath(
                    GitPathIdentity::from_bytes(b"tracked".to_vec()),
                )],
            );
            snapshots.push(snapshot);
        }
        assert_ne!(snapshots[0], snapshots[1]);
    }

    #[cfg(unix)]
    #[test]
    fn supplied_leaf_inspection_checks_administrative_incarnation_before_git() {
        let (temp, identity) = synthetic_inspection_repository();
        std::fs::write(temp.path().join(".git/HEAD"), b"ref: refs/heads/replaced\n").unwrap();
        let error = super::inspect_worktree_at_with_git_leaf_until(
            &identity,
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(30),
            &mut |_, _, _| panic!("changed incarnation entered the Git leaf"),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "captured worktree administrative incarnation changed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn supplied_leaf_inspection_propagates_refusal_at_every_git_leaf() {
        let (temp, identity) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        for stop in [
            "Git status inspection",
            "exact ignored-path inspection",
            "hidden-index inspection",
            "detached head inspection",
            "Git ref backend inspection",
            "gitlink inspection",
        ] {
            let mut stopped = false;
            let error = super::inspect_worktree_at_with_git_leaf_until(
                &identity,
                temp.path(),
                deadline,
                &mut |command: std::process::Command, leaf_deadline, operation: &str| {
                    assert_eq!(leaf_deadline, deadline);
                    assert!(!stopped);
                    if operation == stop {
                        stopped = true;
                        return Err("supplied leaf refused".to_string());
                    }
                    let stdout = match operation {
                        "hidden-index inspection" => b"H tracked\0".to_vec(),
                        "detached head inspection" => {
                            if command
                                .get_args()
                                .any(|argument| argument == "symbolic-ref")
                            {
                                b"refs/heads/main\n".to_vec()
                            } else {
                                b"1111111111111111111111111111111111111111\n".to_vec()
                            }
                        }
                        "detached commit reachability" => {
                            if command.get_args().any(|argument| argument == "--git-path") {
                                b".git/logs/HEAD".to_vec()
                            } else {
                                Vec::new()
                            }
                        }
                        _ => Vec::new(),
                    };
                    let success = synthetic_inspection_succeeds(&command, operation);
                    Ok(synthetic_inspection_output(stdout, success))
                },
            )
            .unwrap_err();
            assert!(stopped);
            assert_eq!(error, "supplied leaf refused");
        }
    }

    #[cfg(unix)]
    #[test]
    fn supplied_leaf_inspection_does_not_treat_symbolic_ref_errors_as_detached_head() {
        use std::os::unix::process::ExitStatusExt as _;

        let (temp, identity) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let error = super::inspect_worktree_at_with_git_leaf_until(
            &identity,
            temp.path(),
            deadline,
            &mut |command: std::process::Command, _, operation: &str| {
                if operation == "detached head inspection"
                    && command
                        .get_args()
                        .any(|argument| argument == "symbolic-ref")
                {
                    return Ok(std::process::Output {
                        status: std::process::ExitStatus::from_raw(128 << 8),
                        stdout: Vec::new(),
                        stderr: b"symbolic-ref failed".to_vec(),
                    });
                }
                let stdout = match operation {
                    "hidden-index inspection" => b"H tracked\0".to_vec(),
                    _ => Vec::new(),
                };
                Ok(synthetic_inspection_output(
                    stdout,
                    synthetic_inspection_succeeds(&command, operation),
                ))
            },
        )
        .unwrap_err();
        assert!(error.contains("cannot determine whether HEAD is detached"));
        assert!(error.contains("symbolic-ref failed"));
    }

    #[test]
    fn supplied_leaf_inspection_rejects_expired_deadline_before_repository_access() {
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("expired-supplied-inspection").unwrap(),
            WorktreeFingerprint::parse("captured-incarnation").unwrap(),
            GitPathIdentity::from_bytes(b"unopened-repository".to_vec()),
        );
        let error = super::inspect_worktree_at_with_git_leaf_until(
            &identity,
            Path::new("must-not-access-a-repository"),
            std::time::Instant::now(),
            &mut |_, _, _| panic!("expired inspection entered the Git leaf"),
        )
        .unwrap_err();
        assert_eq!(error, "worktree inspection exceeded its deadline");
    }

    #[test]
    fn expired_git_leaf_does_not_enter_the_caller() {
        let deadline = std::time::Instant::now();
        let mut calls = 0;
        let error = super::execute_worktree_git_leaf_until(
            std::process::Command::new("must-not-spawn"),
            deadline,
            "supervised probe",
            &mut |_, _, _| {
                calls += 1;
                Err("must not enter the callback".to_string())
            },
        )
        .unwrap_err();
        assert_eq!(error, "supervised probe exceeded its deadline");
        assert_eq!(calls, 0);
    }

    #[test]
    fn expired_legacy_git_wrappers_do_not_access_a_repository() {
        let repository = Path::new("must-not-access-a-repository");
        let deadline = std::time::Instant::now();
        assert!(super::index_gitlinks(repository, deadline).is_err());
        assert!(super::git_index_path(repository, deadline).is_err());
        assert!(super::git_sparse_checkout_cone_mode(repository, deadline).is_err());
        assert!(super::run_bounded_git_paths_until(
            repository,
            &["update-index"],
            &[],
            true,
            Path::new("must-not-access-an-index"),
            deadline,
            "expired path probe",
        )
        .is_err());
        assert!(super::sparse_checkout_absent_paths(
            repository,
            &[super::HiddenIndexCandidate {
                flag: super::HiddenIndexFlag::SkipWorktree,
                path: b"absent".to_vec(),
            }],
            deadline,
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn recursive_worktree_inspections_share_each_git_leaf_and_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let module = root.join("module");
        let nested = module.join("nested");
        for repository in [root, module.as_path(), nested.as_path()] {
            std::fs::create_dir_all(repository.join(".git")).unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let oid = "1111111111111111111111111111111111111111";
        let mut calls = Vec::new();
        // No Git process is needed: every output comes from this one leaf.
        let mut git_leaf = |command: std::process::Command, leaf_deadline, operation: &str| {
            assert_eq!(leaf_deadline, deadline);
            let repository = command.get_current_dir().unwrap().to_path_buf();
            calls.push((repository.clone(), operation.to_string()));
            let stdout = match operation {
                "gitlink inspection" if repository == root => {
                    format!("160000 {oid} 0\tmodule\0").into_bytes()
                }
                "gitlink inspection" if repository == module => {
                    format!("160000 {oid} 0\tnested\0").into_bytes()
                }
                "Git status inspection" => b"?? untracked\0".to_vec(),
                "Git index path inspection" => b".git/missing-index\n".to_vec(),
                "Git submodule HEAD inspection" => {
                    if command
                        .get_args()
                        .any(|argument| argument == "symbolic-ref")
                    {
                        b"refs/heads/main\n".to_vec()
                    } else {
                        format!("{oid}\n").into_bytes()
                    }
                }
                "detached head inspection" => {
                    if command
                        .get_args()
                        .any(|argument| argument == "symbolic-ref")
                    {
                        b"refs/heads/main\n".to_vec()
                    } else {
                        b"1111111111111111111111111111111111111111\n".to_vec()
                    }
                }
                "detached commit reachability" => {
                    if command.get_args().any(|argument| argument == "--git-path") {
                        b".git/logs/HEAD".to_vec()
                    } else {
                        Vec::new()
                    }
                }
                _ => Vec::new(),
            };
            let success = synthetic_inspection_succeeds(&command, operation);
            Ok(synthetic_inspection_output(stdout, success))
        };
        let mut observation = Vec::new();
        super::observe_initialized_submodules_with_leaf_until(
            root,
            &[],
            &mut observation,
            deadline,
            &mut std::collections::HashSet::new(),
            &mut git_leaf,
        )
        .unwrap();
        super::ensure_no_ignored_content_with_git_leaf_until(root, deadline, &mut git_leaf)
            .unwrap();
        let bindings =
            super::capture_git_directory_bindings_with_leaf_until(root, deadline, &mut git_leaf)
                .unwrap();
        assert_eq!(bindings.len(), 3);
        assert!(bindings.contains_key(Path::new("module/nested")));
        assert_eq!(calls.len(), 32);
        assert_eq!(
            calls
                .iter()
                .filter(|(_, operation)| operation == "exact ignored-path inspection")
                .count(),
            2
        );
        assert_eq!(
            calls
                .iter()
                .filter(|(_, operation)| operation == "gitlink inspection")
                .count(),
            9
        );
        assert_eq!(
            calls
                .iter()
                .filter(|(_, operation)| operation == "Git ref backend inspection")
                .count(),
            2
        );
        assert!(observation
            .iter()
            .any(|(key, _)| { key == b"SUBMODULE_LOSS\0untracked\0module/nested/untracked" }));
    }

    #[cfg(unix)]
    #[test]
    fn ignored_inspection_stops_at_a_refused_git_leaf() {
        use std::os::unix::process::ExitStatusExt as _;

        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut calls = Vec::new();
        let error = super::ensure_no_ignored_content_with_git_leaf_until(
            temp.path(),
            deadline,
            &mut |_, leaf_deadline, operation| {
                assert_eq!(leaf_deadline, deadline);
                calls.push(operation.to_string());
                if operation == "gitlink inspection" {
                    return Err("original worker admission refused".to_string());
                }
                Ok(std::process::Output {
                    status: std::process::ExitStatus::from_raw(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            },
        )
        .unwrap_err();
        assert_eq!(error, "original worker admission refused");
        assert_eq!(calls, ["Git status inspection", "gitlink inspection"]);
    }

    #[cfg(unix)]
    #[test]
    fn registry_common_directory_and_fsmonitor_use_individual_git_leaves() {
        use std::os::unix::process::ExitStatusExt as _;

        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let expected_common = temp.path().join(".git").canonicalize().unwrap();
        let mut common_output = expected_common.as_os_str().as_encoded_bytes().to_vec();
        common_output.push(b'\n');
        let mut calls = Vec::new();
        let mut git_leaf = |command: std::process::Command, leaf_deadline, operation: &str| {
            assert_eq!(leaf_deadline, deadline);
            assert!(!command.get_args().any(|argument| argument == "foreach"));
            calls.push(operation.to_string());
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: if operation == "Git common-directory inspection" {
                    common_output.clone()
                } else {
                    Vec::new()
                },
                stderr: Vec::new(),
            })
        };
        let common = super::exact_worktree_common_git_dir_with_leaf_until(
            temp.path(),
            deadline,
            &mut git_leaf,
        )
        .unwrap();
        assert_eq!(common, expected_common);
        super::list_registered_worktrees_in_common_directory_with_leaf_until(
            &common,
            deadline,
            &mut git_leaf,
        )
        .unwrap();
        assert_eq!(
            super::stop_bound_fsmonitor_with_leaf_until(temp.path(), deadline, &mut git_leaf)
                .unwrap(),
            super::FsmonitorStop::Stopped,
        );
        assert_eq!(
            calls,
            [
                "Git common-directory inspection",
                "exact Worktree registration inspection",
                "worktree fsmonitor shutdown",
            ]
        );
    }

    #[test]
    fn expired_original_fsmonitor_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("not-a-repository");
        let error =
            super::stop_bound_fsmonitor_daemons_until(&nonexistent, std::time::Instant::now())
                .unwrap_err();
        assert_eq!(error, "submodule fsmonitor shutdown exceeded its deadline");
        assert!(!nonexistent.exists());
    }

    #[test]
    fn expired_hidden_worktree_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("not-a-repository");
        let error =
            super::observe_hidden_worktree_changes_until(&nonexistent, std::time::Instant::now())
                .unwrap_err();
        assert_eq!(error, "hidden worktree inspection exceeded its deadline");
        assert!(!nonexistent.exists());
    }

    #[test]
    fn expired_dirty_index_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("not-a-repository");
        let error = super::staged_index_entries_for_paths_until(
            &nonexistent,
            &[b"tracked".to_vec()],
            std::time::Instant::now(),
        )
        .unwrap_err();
        assert_eq!(error, "dirty index inspection exceeded its deadline");
        assert!(!nonexistent.exists());
    }

    #[test]
    fn expired_dirty_content_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("not-a-repository");
        let losses = [CloseLossItem::UnstagedTrackedPath(
            GitPathIdentity::from_bytes(b"tracked".to_vec()),
        )];
        let error =
            super::observe_dirty_content_until(&nonexistent, &losses, std::time::Instant::now())
                .unwrap_err();
        assert_eq!(error, "dirty content inspection exceeded its deadline");
        assert!(!nonexistent.exists());
    }

    #[test]
    fn expired_ignored_content_deadline_refuses_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("not-a-repository");
        let error = super::ensure_no_ignored_content_until(&nonexistent, std::time::Instant::now())
            .unwrap_err();
        assert_eq!(error, "ignored-content inspection exceeded its deadline");
        assert!(!nonexistent.exists());
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
    #[test]
    fn fsmonitor_unsupported_platform_requires_the_exact_git_source_diagnostic() {
        use std::os::unix::process::ExitStatusExt as _;
        let output = |code, stdout: &[u8], stderr: &[u8]| std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        };
        // Git v2.47.0 builtin/fsmonitor--daemon.c's unsupported-backend die().
        let unsupported = b"fatal: fsmonitor--daemon not supported on this platform\n";
        assert_eq!(
            super::classify_fsmonitor_stop(&output(128, b"", unsupported)).unwrap(),
            super::FsmonitorStop::NotRunning,
        );
        for stderr in [
            b"fatal: fsmonitor--daemon is not supported on this platform\n".as_slice(),
            b"fatal: fsmonitor--daemon not supported\n".as_slice(),
            b"fatal: permission denied\n".as_slice(),
            b"fatal: fsmonitor--daemon not supported on this platform\nwarning: unknown failure\n"
                .as_slice(),
            b" fatal: fsmonitor--daemon not supported on this platform\n".as_slice(),
            b"fatal: fsmonitor--daemon not supported on this platform\n\n".as_slice(),
        ] {
            assert!(super::classify_fsmonitor_stop(&output(128, b"", stderr)).is_err());
        }
        for code in [1, 127, 129] {
            assert!(super::classify_fsmonitor_stop(&output(code, b"", unsupported)).is_err());
        }
        assert!(super::classify_fsmonitor_stop(&output(128, b"unexpected", unsupported)).is_err());
    }

    #[test]
    fn external_writer_detector_gap_does_not_mask_independent_evidence_or_target_failure() {
        use super::{CloseWriterDetector, CloseWriterGap, ExternalWriterEvidence};

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let no_evidence = || Ok(ExternalWriterEvidence::NoPositiveEvidence);
        let gap = || {
            Ok(super::writer_gap(
                CloseWriterDetector::MacDescriptor,
                Some(741),
                CloseWriterGap::Unavailable,
            ))
        };
        let unknown =
            super::scan_writer_detectors(deadline, gap, no_evidence, no_evidence).unwrap();
        assert_eq!(unknown, gap().unwrap());
        assert!(!unknown.is_positive());
        assert!(unknown.blocks_retirement(), "strict paths still block");

        let positive = super::scan_writer_detectors(deadline, gap, no_evidence, || {
            Ok(injected_writer_evidence())
        })
        .unwrap();
        assert_eq!(positive, injected_writer_evidence());
        assert!(positive.is_positive());

        let failure = super::scan_writer_detectors(
            deadline,
            gap,
            || Err("cannot bind target".to_owned()),
            || panic!("target failure must stop the scan"),
        );
        assert_eq!(failure.unwrap_err(), "cannot bind target");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn external_writer_entry_gap_does_not_mask_an_independent_match() {
        use super::{CloseWriterGap, WriterScanError};

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let inspect = |entry| match entry {
            0 => Err(WriterScanError::Gap(CloseWriterGap::Unavailable)),
            1 => Ok(None),
            2 => Ok(Some(entry)),
            _ => panic!("matched entry should end the scan"),
        };
        let result = super::scan_writer_entries([0, 1, 2, 3], deadline, inspect);
        assert_eq!(result.unwrap(), Some(2));
        assert!(matches!(
            super::scan_writer_entries([0, 1], deadline, inspect),
            Err(WriterScanError::Gap(CloseWriterGap::Unavailable))
        ));
        assert!(matches!(
            super::scan_writer_entries([2], std::time::Instant::now(), inspect),
            Err(WriterScanError::Stopped(_))
        ));
    }

    fn injected_writer_evidence() -> super::ExternalWriterEvidence {
        super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority {
            detector: super::CloseWriterDetector::MacDescriptor,
            process: super::CloseWriterProcess {
                pid: 123,
                incarnation: OpaqueIdentity::parse("birth-123").unwrap(),
                executable: PathBuf::from("/test/writer"),
            },
            matched_path: PathBuf::from("/test/quarantine/file"),
            authority: super::CloseWriterAuthority::WritableFile(
                super::CloseFileWriteAccess::ReadWrite,
            ),
        })
    }

    /// Deterministic effects for tests: inject fsmonitor shutdown and report no writers.
    pub(super) fn injected_close_worktree_effects(
        stop_fsmonitor: impl FnOnce(&Path) -> Result<super::FsmonitorStop, String> + Send + 'static,
    ) -> super::CloseWorktreeEffects {
        super::CloseWorktreeEffects {
            removal_entry: super::WorktreeRemovalEntry::Test,
            stop_fsmonitor: Box::new(move |path, _deadline, _admission| stop_fsmonitor(path)),
            first_submodule_spawn_admission: None,
            observe_open_descriptors: Box::new(|_, _| {
                Ok(super::ExternalWriterEvidence::NoPositiveEvidence)
            }),
            observe_external_writer: Box::new(|_, _| Ok(false)),
        }
    }

    /// Deterministic effects that additionally reject any writer scan, proving a
    /// control path returns before the quarantine descriptor/writer probes.
    fn injected_close_worktree_effects_rejecting_scans(
        stop_fsmonitor: impl FnOnce(&Path) -> Result<super::FsmonitorStop, String> + Send + 'static,
    ) -> super::CloseWorktreeEffects {
        super::CloseWorktreeEffects {
            removal_entry: super::WorktreeRemovalEntry::Test,
            stop_fsmonitor: Box::new(move |path, _deadline, _admission| stop_fsmonitor(path)),
            first_submodule_spawn_admission: None,
            observe_open_descriptors: Box::new(|_, _| {
                panic!("must not scan for open descriptors before quarantine commits")
            }),
            observe_external_writer: Box::new(|_, _| {
                panic!("must not scan for an external writer before quarantine commits")
            }),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worktree_effect_receives_the_original_absolute_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let snapshot = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut effects = injected_close_worktree_effects_rejecting_scans(|_| {
            panic!("replaced by deadline-aware effect")
        });
        effects.stop_fsmonitor = Box::new(move |_, received, _admission| {
            assert_eq!(received, deadline);
            Err("fixture stopped before fsmonitor effect".to_string())
        });

        let result = super::quarantine_and_remove_exact_worktree_with_effects_until(
            &identity,
            &snapshot,
            admin,
            incarnation,
            None,
            |_, _, _| panic!("must not bind a deletion tombstone"),
            |_| panic!("must not quarantine after failed fsmonitor stop"),
            effects,
            deadline,
        )
        .await
        .unwrap();

        assert!(matches!(result, ExactWorktreeRemoval::StopFailed { detail }
            if detail == "fixture stopped before fsmonitor effect"));
        assert!(target.exists());
        assert!(!worktree_quarantine_path(&identity).unwrap().exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn quarantine_rejects_live_linux_descriptor_writer_before_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let target = linked_close_worktree(&temp);
        let identity = inspection_identity(&target);
        let snapshot = inspect_worktree(&identity).await.unwrap().0;
        let common = super::exact_worktree_common_git_dir(&target).unwrap();
        let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
        let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .open(target.join("tracked"))
            .unwrap();
        let writer_pid = std::process::id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut effects = injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning));
        effects.observe_open_descriptors = Box::new(move |quarantine, deadline| {
            let evidence = super::quarantine_has_open_descriptors_until(quarantine, deadline)?;
            assert!(
                matches!(
                    &evidence,
                    super::ExternalWriterEvidence::Observed(
                        super::CloseWriterObservation::Authority {
                            detector: super::CloseWriterDetector::LinuxDescriptor,
                            process,
                            matched_path,
                            authority: super::CloseWriterAuthority::WritableFile(
                                super::CloseFileWriteAccess::WriteOnly
                                    | super::CloseFileWriteAccess::ReadWrite
                            ),
                        }
                    ) if process.pid == writer_pid
                        && matched_path == &quarantine.join("tracked")
                ),
                "expected real /proc descriptor attribution to the held writer: {evidence:?}"
            );
            Ok(evidence)
        });
        effects.observe_external_writer = Box::new(|_, _| {
            panic!("descriptor evidence must refuse retirement before the next writer scan")
        });

        let outcome = super::quarantine_and_remove_exact_worktree_with_effects_until(
            &identity,
            &snapshot,
            admin.clone(),
            incarnation,
            None,
            |_, _, _| panic!("must not bind deletion after live writer evidence"),
            |_| {},
            effects,
            deadline,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, ExactWorktreeRemoval::Residual { .. }));
        let retained = worktree_quarantine_path(&identity).unwrap();
        assert!(!target.exists());
        assert_eq!(
            std::fs::read_to_string(retained.join("tracked")).unwrap(),
            "initial\n"
        );
        assert!(admin.exists());
        drop(writer);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quarantine_rejects_positive_or_failed_writer_observations_before_deletion() {
        for observation in 0..4 {
            let temp = tempfile::tempdir().unwrap();
            let target = linked_close_worktree(&temp);
            let identity = inspection_identity(&target);
            let snapshot = inspect_worktree(&identity).await.unwrap().0;
            let common = super::exact_worktree_common_git_dir(&target).unwrap();
            let admin = super::exact_worktree_administrative_dir(&target, &common).unwrap();
            let incarnation = observe_administrative_dir_incarnation(&admin).unwrap();
            let mut effects =
                injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning));
            effects.observe_open_descriptors = Box::new(move |_, _| match observation {
                0 => Ok(injected_writer_evidence()),
                1 => Err("injected descriptor observation failure".into()),
                _ => Ok(super::ExternalWriterEvidence::NoPositiveEvidence),
            });
            effects.observe_external_writer = Box::new(move |_, _| match observation {
                2 => Ok(true),
                3 => Err("injected writer observation failure".into()),
                _ => panic!("later writer observation must not run after descriptor rejection"),
            });
            let outcome = super::quarantine_and_remove_exact_worktree_with_effects(
                &identity,
                &snapshot,
                admin.clone(),
                incarnation,
                None,
                |_, _, _| panic!("must not bind deletion after rejected writer evidence"),
                |_| {},
                effects,
            )
            .await;
            match observation {
                0 | 2 => assert!(matches!(
                    outcome.unwrap(),
                    ExactWorktreeRemoval::Residual { .. }
                )),
                _ => assert!(matches!(outcome, Err(ref detail) if detail.contains("injected"))),
            }
            let retained = worktree_quarantine_path(&identity).unwrap();
            assert_eq!(
                std::fs::read_to_string(retained.join("tracked")).unwrap(),
                "initial\n"
            );
            assert!(admin.exists());
        }
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
        let result = super::quarantine_and_remove_exact_worktree_with_effects(
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
            injected_close_worktree_effects_rejecting_scans(|_| {
                Err("injected fsmonitor timeout".into())
            }),
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
        let result = super::quarantine_and_remove_exact_worktree_with_effects(
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
            injected_close_worktree_effects_rejecting_scans(move |_| {
                std::fs::write(&tracked, "changed while stopping\n").unwrap();
                Ok(super::FsmonitorStop::Stopped)
            }),
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

    #[test]
    fn retry_writer_precondition_rejects_when_a_target_holds_a_positive_writer() {
        let original = Path::new("/captured/worktree");
        let quarantine = Path::new("/captured/quarantine");
        let error = super::probe_retry_worktree_writers_with(original, quarantine, None, |path| {
            Ok(if path == quarantine {
                super::RetryTargetObservation::Present(injected_writer_evidence())
            } else {
                super::RetryTargetObservation::Absent
            })
        })
        .expect_err("a positive external writer must reject the retry");
        let message = error.to_string();
        assert!(
            message.contains("external writer") && message.contains("no new run admitted"),
            "rejection must name the external writer: {message}"
        );
        assert!(
            message.contains(&quarantine.display().to_string()),
            "rejection must name the holding target: {message}"
        );
    }

    #[test]
    fn retry_writer_inspection_gap_keeps_exact_target_and_typed_details() {
        let original = Path::new("/captured/worktree");
        let quarantine = Path::new("/captured/quarantine");
        let mut observed = Vec::new();
        let error = super::probe_retry_worktree_writers_with(original, quarantine, None, |path| {
            observed.push(path.to_path_buf());
            Ok(super::RetryTargetObservation::Present(
                super::ExternalWriterEvidence::Observed(
                    super::CloseWriterObservation::InspectionGap {
                        detector: super::CloseWriterDetector::LinuxDescriptor,
                        pid: None,
                        reason: super::CloseWriterGap::Unavailable,
                    },
                ),
            ))
        })
        .expect_err("an inspection gap must block before later retry targets");
        let super::RetryAssessmentError::WriterInspectionGap(gap) = error else {
            panic!("inspection gap lost its typed classification");
        };
        assert_eq!(gap.path, original);
        assert_eq!(gap.detector, super::CloseWriterDetector::LinuxDescriptor);
        assert_eq!(gap.reason, super::CloseWriterGap::Unavailable);
        assert_eq!(gap.pid, None);
        assert_eq!(observed, [original.to_path_buf()]);
    }

    #[test]
    fn retry_writer_precondition_rejects_when_observing_a_target_fails() {
        let original = Path::new("/captured/worktree");
        let quarantine = Path::new("/captured/quarantine");
        let error = super::probe_retry_worktree_writers_with(original, quarantine, None, |path| {
            if path == original {
                Err("cannot probe retry writer target: injected".to_string())
            } else {
                Ok(super::RetryTargetObservation::Absent)
            }
        })
        .expect_err("a probe failure must be fail-closed, not proof of absence");
        assert!(
            error
                .to_string()
                .contains("cannot probe retry writer target"),
            "probe error must propagate as the rejection: {error}"
        );
    }

    #[test]
    fn retry_writer_precondition_accepts_when_no_target_holds_a_writer() {
        let original = Path::new("/captured/worktree");
        let quarantine = Path::new("/captured/quarantine");
        let tombstone = CloseWorktreeFinalTombstone {
            root: std::path::PathBuf::from("/captured/tombstone"),
            device: 0,
            inode: 0,
            object_device: None,
            object_inode: None,
        };
        let mut observed = Vec::new();
        super::probe_retry_worktree_writers_with(original, quarantine, Some(&tombstone), |path| {
            observed.push(path.to_path_buf());
            Ok(if path == original {
                // Absent targets cannot be held; the precondition ignores them.
                super::RetryTargetObservation::Absent
            } else {
                super::RetryTargetObservation::Present(
                    super::ExternalWriterEvidence::NoPositiveEvidence,
                )
            })
        })
        .expect("absent or writer-free targets must pass the retry writer precondition");
        assert_eq!(
            observed,
            vec![
                original.to_path_buf(),
                quarantine.to_path_buf(),
                tombstone.root.join("object"),
            ],
            "the precondition must observe each candidate once in original, quarantine, object order"
        );
    }

    #[cfg(unix)]
    fn resume_tombstone_with_no_external_writer(
        tombstone: &CloseWorktreeFinalTombstone,
        identity: &WorktreeIdentity,
        confirmed: &phoenix_core::domain::close::CloseRetirementSnapshot,
    ) -> FinalTombstoneRecovery {
        super::resume_final_worktree_tombstone_with_writer_probe(
            tombstone,
            identity,
            confirmed,
            |_| Ok(false),
        )
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
            resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed),
            FinalTombstoneRecovery::Residual(_)
        ));
        assert!(recorded.root.join("object/original").exists());
        std::fs::write(recorded.root.join("object/original"), "original\n").unwrap();
        std::fs::write(recorded.root.join("object/.gitignore"), "cache/\n").unwrap();
        std::fs::create_dir(recorded.root.join("object/cache")).unwrap();
        std::fs::write(recorded.root.join("object/cache/unique"), "unique").unwrap();
        assert!(matches!(
            resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed),
            FinalTombstoneRecovery::Residual(_)
        ));
        assert!(recorded.root.join("object/cache/unique").exists());
        std::fs::remove_dir_all(recorded.root.join("object/cache")).unwrap();
        std::fs::remove_file(recorded.root.join("object/.gitignore")).unwrap();
        for observation in [Ok(true), Err("writer observation unavailable".to_string())] {
            let retained = super::resume_final_worktree_tombstone_with_writer_probe(
                &recorded,
                &identity,
                &confirmed,
                |object| {
                    assert_eq!(object, recorded.root.join("object"));
                    observation
                },
            );
            assert!(matches!(retained, FinalTombstoneRecovery::Residual(_)));
            assert_eq!(
                std::fs::read_to_string(recorded.root.join("object/original")).unwrap(),
                "original\n"
            );
        }
        let recovery = resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed);
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

        let recovery = resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed);
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
        super::reserve_private_tombstone(&root, "test worktree").unwrap();
        let metadata = std::fs::symlink_metadata(&root).unwrap();
        let recorded = CloseWorktreeFinalTombstone {
            root,
            device: metadata.dev(),
            inode: metadata.ino(),
            object_device: None,
            object_inode: None,
        };

        let recovery = resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed);
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
        super::reserve_private_tombstone(&root, "test worktree").unwrap();
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
            resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed),
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

        let recovery = resume_tombstone_with_no_external_writer(&recorded, &identity, &confirmed);
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

    #[cfg(unix)]
    #[test]
    fn administrative_final_tombstone_symlink_is_not_absence_or_adopted_custody() {
        let temp = tempfile::tempdir().unwrap();
        let registration = temp.path().join("absent-registration");
        let incarnation = "captured-incarnation";
        let quarantine =
            super::administrative_dir_quarantine_path(&registration, incarnation).unwrap();
        let root = super::administrative_dir_final_tombstone_path(&quarantine);
        let absent = temp.path().join("absent-target");
        std::os::unix::fs::symlink(&absent, &root).unwrap();

        let error = super::remove_exact_worktree_administrative_dir(&registration, incarnation)
            .unwrap_err();

        assert!(error.contains("unresolved final tombstone"), "{error}");
        assert_eq!(std::fs::read_link(root).unwrap(), absent);
        assert!(!registration.exists());
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

    #[cfg(unix)]
    #[test]
    fn restart_refuses_unresolved_administrative_final_tombstone() {
        for fresh_root in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let linked = linked_close_worktree(&temp);
            let identity = inspection_identity(&linked);
            let common = super::exact_worktree_common_git_dir(&linked).unwrap();
            let administrative = exact_worktree_administrative_dir(&linked, &common).unwrap();
            let incarnation = observe_administrative_dir_incarnation(&administrative).unwrap();
            let quarantine =
                super::administrative_dir_quarantine_path(&administrative, &incarnation).unwrap();
            let root = if fresh_root {
                super::fresh_administrative_dir_final_tombstone_path(&quarantine)
            } else {
                super::administrative_dir_final_tombstone_path(&quarantine)
            };
            std::fs::create_dir(&root).unwrap();
            std::fs::rename(&administrative, root.join("object")).unwrap();
            std::fs::remove_dir_all(&linked).unwrap();

            for phase in 0..4 {
                let error = complete_persisted_worktree_administrative_cleanup(
                    &identity,
                    &administrative,
                    &incarnation,
                )
                .unwrap_err();
                assert!(error.contains("unresolved final tombstone"), "{error}");
                assert!(!administrative.exists());
                assert!(!quarantine.exists());
                match phase {
                    0 => {
                        assert!(root.join("object/gitdir").exists());
                        std::fs::remove_dir_all(root.join("object")).unwrap();
                    }
                    1 => {
                        assert!(root.is_dir());
                        std::fs::remove_dir(&root).unwrap();
                        std::os::unix::fs::symlink(temp.path().join("absent-target"), &root)
                            .unwrap();
                    }
                    2 => {
                        assert!(std::fs::symlink_metadata(&root)
                            .unwrap()
                            .file_type()
                            .is_symlink());
                        assert_eq!(
                            std::fs::read_link(&root).unwrap(),
                            temp.path().join("absent-target")
                        );
                        std::fs::remove_file(&root).unwrap();
                        std::fs::write(&root, b"preserve final tombstone file").unwrap();
                    }
                    _ => {
                        assert!(root.is_file());
                        assert_eq!(
                            std::fs::read(&root).unwrap(),
                            b"preserve final tombstone file"
                        );
                    }
                }
            }
        }
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
                injected_close_worktree_effects(|_| Ok(super::FsmonitorStop::NotRunning)),
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

    #[test]
    fn read_only_clean_inspection_accepts_a_detached_head_preserved_by_a_branch() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["checkout", "--quiet", "--detach"]);
        let identity = inspection_identity(temp.path());
        let (_, losses) = super::inspect_worktree_at_with_policy_until(
            &identity,
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(30),
            super::WorktreeInspectionPolicy::ReadOnlyClean,
            &mut super::run_bounded_spawned_git,
        )
        .unwrap();
        assert!(losses.is_empty());
    }

    #[test]
    fn target_head_reflog_roots_include_both_old_and_new_endpoints() {
        let old = "a".repeat(40);
        let new = "b".repeat(40);
        let log = format!("{old} {new} User <user@example.test> 1 +0000\tcheckout\n");

        let roots = super::parse_target_head_reflog_roots(
            log.as_bytes(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(roots, vec![old, new]);
    }

    #[test]
    fn target_head_reflog_rejects_malformed_records() {
        let log = format!(
            "{} short User <user@example.test> 1 +0000\tbroken\n",
            "0".repeat(40)
        );

        assert!(super::parse_target_head_reflog_roots(
            log.as_bytes(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        )
        .is_err());
    }

    #[test]
    fn target_head_reflog_root_parsing_observes_deadline_and_distinct_root_cap() {
        use std::fmt::Write as _;

        let log = format!(
            "{} {} User <user@example.test> 1 +0000\tcheckout\n",
            "a".repeat(40),
            "b".repeat(40)
        );
        let expired = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(
            super::parse_target_head_reflog_roots(log.as_bytes(), expired)
                .unwrap_err()
                .contains("exceeded its deadline")
        );

        let zero = "0".repeat(40);
        let mut log = String::new();
        for oid in 1..=1025 {
            writeln!(
                log,
                "{oid:040x} {zero} User <user@example.test> 1 +0000\tcheckout"
            )
            .unwrap();
        }
        let error = super::parse_target_head_reflog_roots(
            log.as_bytes(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.contains("1024 distinct-root limit"));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reflog_reader_rejects_symlink_fifo_and_oversized_files_but_allows_missing() {
        use std::os::unix::{ffi::OsStrExt as _, fs::symlink};

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::write(&target, b"regular").unwrap();
        symlink(&target, &link).unwrap();
        assert!(super::read_bounded_regular_file_nofollow(&link, 1024).is_err());

        let fifo = temp.path().join("fifo");
        let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let (sender, receiver) = std::sync::mpsc::channel();
        let fifo_reader = fifo.clone();
        let reader = std::thread::spawn(move || {
            sender
                .send(super::read_bounded_regular_file_nofollow(
                    &fifo_reader,
                    1024,
                ))
                .unwrap();
        });
        match receiver.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(result) => assert!(result.is_err()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Release a broken blocking open so the test can fail cleanly.
                let _writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
                reader.join().unwrap();
                panic!("FIFO reflog read blocked instead of rejecting the non-regular file");
            }
            Err(error) => panic!("FIFO reader thread failed: {error}"),
        }
        reader.join().unwrap();

        let oversized = temp.path().join("oversized");
        std::fs::write(&oversized, b"12345").unwrap();
        assert!(super::read_bounded_regular_file_nofollow(&oversized, 4).is_err());

        let missing = temp.path().join("missing");
        let error = super::read_bounded_regular_file_nofollow(&missing, 1024).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let absent_logs = temp.path().join("absent-logs").join("HEAD");
        let error = super::read_bounded_regular_file_nofollow(&absent_logs, 1024).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let real_logs = temp.path().join("real-logs");
        std::fs::create_dir(&real_logs).unwrap();
        let dangling_logs = temp.path().join("dangling-logs");
        symlink(temp.path().join("missing-target"), &dangling_logs).unwrap();
        let error = super::read_bounded_regular_file_nofollow(&dangling_logs.join("HEAD"), 1024)
            .unwrap_err();
        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);

        std::fs::write(real_logs.join("HEAD"), b"redirected").unwrap();
        let redirect_logs = temp.path().join("redirect-logs");
        symlink(&real_logs, &redirect_logs).unwrap();
        assert!(
            super::read_bounded_regular_file_nofollow(&redirect_logs.join("HEAD"), 1024).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn reftable_backend_stops_before_reading_reflog_and_missing_config_is_allowed() {
        let (temp, _) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut calls = 0;
        let error = super::target_head_reflog_roots_with_leaf_until(
            temp.path(),
            deadline,
            &mut |_, leaf_deadline, operation: &str| {
                assert_eq!(leaf_deadline, deadline);
                calls += 1;
                assert_eq!(operation, "Git ref backend inspection");
                Ok(synthetic_inspection_output(b"reftable\n".to_vec(), true))
            },
        )
        .unwrap_err();
        assert!(error.contains("unsupported Git ref storage backend: reftable"));
        assert_eq!(
            calls, 1,
            "unsupported backend must refuse the next Git leaf"
        );

        let mut calls = 0;
        let result = super::ensure_files_ref_storage_with_leaf_until(
            temp.path(),
            deadline,
            &mut |_, leaf_deadline, operation: &str| {
                assert_eq!(leaf_deadline, deadline);
                assert_eq!(operation, "Git ref backend inspection");
                calls += 1;
                Ok(synthetic_inspection_output(Vec::new(), false))
            },
        );
        assert!(
            result.is_ok(),
            "config exit status 1 means unset and allowed"
        );
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn removing_confirmed_durable_detached_rescue_changes_fingerprint_and_reports_loss() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["checkout", "--quiet", "--detach"]);
        std::fs::write(temp.path().join("tracked"), "detached commit\n").unwrap();
        run_git(temp.path(), &["commit", "--quiet", "-am", "detached C"]);
        let detached = run_git_output(temp.path(), &["rev-parse", "HEAD"]);
        let detached_oid = super::GitOidIdentity::parse_hex(detached.clone()).unwrap();
        run_git(temp.path(), &["update-ref", "refs/heads/rescue", &detached]);

        let (confirmed, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(!losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::DetachedUnreachableCommit(oid) if oid == &detached_oid
        )));

        run_git(temp.path(), &["update-ref", "-d", "refs/heads/rescue"]);
        let (fresh, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert_ne!(confirmed.fingerprint(), fresh.fingerprint());
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::DetachedUnreachableCommit(oid) if oid == &detached_oid
        )));
    }

    #[tokio::test]
    async fn target_head_reflog_abandoned_commit_is_loss_until_a_durable_ref_rescues_it() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let safe_a = run_git_output(temp.path(), &["rev-parse", "HEAD"]);
        run_git(temp.path(), &["checkout", "--quiet", "--detach", &safe_a]);
        std::fs::write(temp.path().join("tracked"), "abandoned commit\n").unwrap();
        run_git(temp.path(), &["commit", "--quiet", "-am", "abandoned C"]);
        let abandoned_c = run_git_output(temp.path(), &["rev-parse", "HEAD"]);
        let abandoned_oid = super::GitOidIdentity::parse_hex(abandoned_c.clone()).unwrap();
        run_git(temp.path(), &["checkout", "--quiet", "--detach", &safe_a]);
        // Keep C solely as the old endpoint of a realistic C -> A HEAD reflog entry.
        std::fs::write(
            temp.path().join(".git/logs/HEAD"),
            format!("{abandoned_c} {safe_a} User <user@example.test> 1 +0000\tcheckout: moving from {abandoned_c} to {safe_a}\n"),
        )
        .unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::DetachedUnreachableCommit(oid)
                if oid == &abandoned_oid
        )));

        run_git(
            temp.path(),
            &["update-ref", "refs/heads/rescue", &abandoned_c],
        );
        let (_, rescued_losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(!rescued_losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::DetachedUnreachableCommit(oid)
                if oid == &abandoned_oid
        )));
    }

    #[tokio::test]
    async fn stash_prefix_ref_does_not_rescue_detached_history() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["checkout", "--quiet", "--detach"]);
        std::fs::write(temp.path().join("tracked"), "detached\n").unwrap();
        run_git(temp.path(), &["commit", "--quiet", "-am", "detached"]);
        let detached = run_git_output(temp.path(), &["rev-parse", "HEAD"]);
        let detached_oid = super::GitOidIdentity::parse_hex(detached.clone()).unwrap();
        run_git(temp.path(), &["update-ref", "refs/stash/extra", &detached]);

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .unwrap();
        assert!(losses.iter().any(|loss| matches!(
            loss,
            CloseLossItem::DetachedUnreachableCommit(oid)
                if oid == &detached_oid
        )));
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
    async fn worktree_inspection_rejects_expired_deadline_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent_repository = temp.path().join("does-not-exist");
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("expired-inspection-worktree").unwrap(),
            WorktreeFingerprint::parse("captured-incarnation").unwrap(),
            GitPathIdentity::from_bytes(b"unopened-repository".to_vec()),
        );
        let deadline = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(1))
            .unwrap();

        let error = super::inspect_worktree_at_until(&identity, nonexistent_repository, deadline)
            .await
            .unwrap_err();

        assert_eq!(error, "worktree inspection exceeded its deadline");
    }

    #[test]
    fn sparse_checkout_rule_inspection_rejects_expired_deadline_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent_repository = temp.path().join("does-not-exist");
        let deadline = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(1))
            .unwrap();

        let error = super::git_sparse_checkout_selected_paths(
            &nonexistent_repository,
            &[b"tracked/path".to_vec()],
            false,
            deadline,
        )
        .unwrap_err();

        assert_eq!(
            error,
            "sparse-checkout rule inspection exceeded its deadline"
        );
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

    #[test]
    fn detached_unreachable_commits_until_rejects_expired_deadline_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("does-not-exist");

        let error = super::detached_unreachable_commits_until(
            &nonexistent,
            b"invalid head is not parsed before expiration",
            std::time::Instant::now(),
        )
        .unwrap_err();

        assert!(error.contains("exceeded its deadline"));
    }

    #[test]
    fn detached_reachability_evidence_until_rejects_expired_deadline_before_repository_access() {
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("does-not-exist");

        let error = super::detached_reachability_evidence_until(
            &nonexistent,
            b"invalid head is not parsed before expiration",
            std::time::Instant::now(),
        )
        .unwrap_err();

        assert!(error.contains("exceeded its deadline"));
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
    async fn empty_unborn_symbolic_head_has_no_detached_commit_loss() {
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
    async fn unborn_symbolic_head_retains_old_only_head_reflog_commit_as_loss() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        run_git(temp.path(), &["tag", "close-test-safe-base"]);
        std::fs::write(temp.path().join("tracked"), "unreachable history\n").unwrap();
        run_git(
            temp.path(),
            &["commit", "--quiet", "-am", "reflog-only commit"],
        );
        let old_head = run_git_output(temp.path(), &["rev-parse", "HEAD"]);
        let branch = run_git_output(temp.path(), &["symbolic-ref", "--short", "HEAD"]);
        let branch_ref = format!("refs/heads/{branch}");
        run_git(temp.path(), &["update-ref", "-d", &branch_ref]);
        run_git(
            temp.path(),
            &["symbolic-ref", "HEAD", "refs/heads/unborn-close-test"],
        );
        std::fs::write(
            temp.path().join(".git/logs/HEAD"),
            format!(
                "{old_head} {} Synthetic <test@example.com> 0 +0000\tcheckout: old-only\n",
                "0".repeat(old_head.len())
            ),
        )
        .unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(temp.path()))
            .await
            .expect("unborn symbolic HEAD remains inspectable");
        let old_oid = super::GitOidIdentity::parse_hex(old_head).unwrap();

        let old_only_losses = losses
            .iter()
            .filter(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_)))
            .collect::<Vec<_>>();
        assert_eq!(old_only_losses.len(), 1);
        assert!(matches!(
            old_only_losses[0],
            CloseLossItem::DetachedUnreachableCommit(oid) if oid == &old_oid
        ));
    }

    #[test]
    fn unborn_head_requires_an_absent_branch_ref_not_arbitrary_symbolic_ref() {
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init", "--quiet"]);
        run_git(
            temp.path(),
            &["symbolic-ref", "HEAD", "refs/remotes/missing-close-test"],
        );

        let error = super::inspect_head_with_leaf_until(
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            "test HEAD inspection",
            &mut super::run_bounded_spawned_git,
        )
        .unwrap_err();

        assert!(error.contains("does not name a branch ref"));
    }

    #[cfg(unix)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum UnbornHeadScenario {
        GenuineUnborn,
        FatalHead,
        SignaledHead,
        DiagnosticMissingHead,
        DiagnosticAbsentRef,
        DanglingBranchRef,
        ReboundSymbolicHead,
        NewlyResolvedHead,
    }

    #[cfg(unix)]
    #[derive(Default)]
    struct HeadReadCounts {
        symbolic: usize,
        head: usize,
    }

    #[cfg(unix)]
    fn synthetic_head_inspection_output(
        command: &std::process::Command,
        deadline: std::time::Instant,
        operation: &str,
        scenario: UnbornHeadScenario,
        reads: &mut HeadReadCounts,
    ) -> std::process::Output {
        assert!(deadline > std::time::Instant::now());
        assert_eq!(operation, "test HEAD inspection");
        let args = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let subcommand = args.iter().map(String::as_str).find(|argument| {
            matches!(
                *argument,
                "symbolic-ref" | "rev-parse" | "check-ref-format" | "show-ref"
            )
        });
        match subcommand {
            Some("symbolic-ref") => {
                reads.symbolic += 1;
                let branch =
                    if scenario == UnbornHeadScenario::ReboundSymbolicHead && reads.symbolic > 1 {
                        b"refs/heads/other\n".to_vec()
                    } else {
                        b"refs/heads/main\n".to_vec()
                    };
                synthetic_inspection_output(branch, true)
            }
            Some("rev-parse") => {
                reads.head += 1;
                match scenario {
                    UnbornHeadScenario::FatalHead if reads.head == 1 => {
                        synthetic_inspection_output_raw(
                            Vec::new(),
                            b"fatal: broken repository\n".to_vec(),
                            128 << 8,
                        )
                    }
                    UnbornHeadScenario::SignaledHead if reads.head == 1 => {
                        synthetic_inspection_output_raw(Vec::new(), Vec::new(), 9)
                    }
                    UnbornHeadScenario::DiagnosticMissingHead if reads.head == 1 => {
                        synthetic_inspection_output_raw(
                            Vec::new(),
                            b"warning: uncertain\n".to_vec(),
                            1 << 8,
                        )
                    }
                    UnbornHeadScenario::NewlyResolvedHead if reads.head > 1 => {
                        synthetic_inspection_output_raw(
                            b"1111111111111111111111111111111111111111\n".to_vec(),
                            Vec::new(),
                            0,
                        )
                    }
                    _ => synthetic_inspection_output_raw(Vec::new(), Vec::new(), 1 << 8),
                }
            }
            Some("check-ref-format") => synthetic_inspection_output(Vec::new(), true),
            Some("show-ref") => {
                assert!(deadline > std::time::Instant::now());
                assert!(command.get_envs().any(|(name, value)| {
                    name == "LC_ALL" && value == Some(std::ffi::OsStr::new("C"))
                }));
                match scenario {
                    UnbornHeadScenario::DanglingBranchRef => {
                        synthetic_inspection_output_raw(Vec::new(), Vec::new(), 0)
                    }
                    UnbornHeadScenario::DiagnosticAbsentRef => synthetic_inspection_output_raw(
                        Vec::new(),
                        b"error: reference does not exist\nwarning: incomplete ref lookup\n"
                            .to_vec(),
                        2 << 8,
                    ),
                    _ => synthetic_inspection_output_raw(
                        Vec::new(),
                        b"error: reference does not exist\n".to_vec(),
                        2 << 8,
                    ),
                }
            }
            _ => panic!("unexpected HEAD inspection command: {args:?}"),
        }
    }

    #[cfg(unix)]
    fn assert_unborn_head_scenario(scenario: UnbornHeadScenario) {
        let (temp, _) = synthetic_inspection_repository();
        let mut reads = HeadReadCounts::default();
        let mut leaf =
            |command: std::process::Command, deadline: std::time::Instant, operation: &str| {
                Ok(synthetic_head_inspection_output(
                    &command, deadline, operation, scenario, &mut reads,
                ))
            };
        let result = super::inspect_head_with_leaf_until(
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            "test HEAD inspection",
            &mut leaf,
        );
        if scenario == UnbornHeadScenario::GenuineUnborn {
            assert!(matches!(result, Ok(super::InspectedHead::Unborn { .. })));
        } else {
            assert!(
                result.is_err(),
                "{scenario:?} must not be classified as unborn"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn unborn_head_classification_fails_closed_on_uncertain_or_changed_state() {
        for scenario in [
            UnbornHeadScenario::GenuineUnborn,
            UnbornHeadScenario::FatalHead,
            UnbornHeadScenario::SignaledHead,
            UnbornHeadScenario::DiagnosticMissingHead,
            UnbornHeadScenario::DiagnosticAbsentRef,
            UnbornHeadScenario::DanglingBranchRef,
            UnbornHeadScenario::ReboundSymbolicHead,
            UnbornHeadScenario::NewlyResolvedHead,
        ] {
            assert_unborn_head_scenario(scenario);
        }
    }

    #[test]
    fn detached_or_unreadable_head_failure_is_not_classified_as_unborn() {
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init", "--quiet"]);
        std::fs::write(temp.path().join(".git/HEAD"), "not-a-symbolic-ref\n").unwrap();

        let error = super::inspect_head_with_leaf_until(
            temp.path(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            "test HEAD inspection",
            &mut super::run_bounded_spawned_git,
        )
        .unwrap_err();

        assert!(!error.contains("branch ref is absent"));
        assert!(!error.contains("does not name a branch ref"));
    }

    #[tokio::test]
    async fn symbolic_submodule_head_reflog_old_endpoint_is_reported_as_loss() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child-symbolic-history");
        let parent = temp.path().join("parent-symbolic-history");
        initialize_repository(&child);
        initialize_repository(&parent);
        let safe_a = run_git_output(&child, &["rev-parse", "HEAD"]);
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

        let submodule = parent.join("deps/child");
        std::fs::write(submodule.join("tracked"), "abandoned symbolic commit\n").unwrap();
        run_git(&submodule, &["commit", "--quiet", "-am", "abandoned C"]);
        let abandoned = run_git_output(&submodule, &["rev-parse", "HEAD"]);
        let abandoned_oid = super::GitOidIdentity::parse_hex(abandoned.clone()).unwrap();
        run_git(&submodule, &["reset", "--quiet", "--hard", &safe_a]);
        std::fs::write(
            std::path::PathBuf::from(run_git_output(
                &submodule,
                &["rev-parse", "--absolute-git-dir"],
            ))
            .join("logs/HEAD"),
            format!("{abandoned} {safe_a} Synthetic <test@example.com> 0 +0000\treset: old-only\n"),
        )
        .unwrap();

        let (_, losses) = inspect_worktree(&inspection_identity(&parent))
            .await
            .unwrap();
        let old_only_losses = losses
            .iter()
            .filter(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_)))
            .collect::<Vec<_>>();
        assert_eq!(old_only_losses.len(), 1);
        assert!(matches!(
            old_only_losses[0],
            CloseLossItem::DetachedUnreachableCommit(oid) if oid == &abandoned_oid
        ));
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
        let (_confirmed, losses) = inspect_worktree(&identity).await.unwrap();
        assert!(losses
            .iter()
            .any(|loss| matches!(loss, CloseLossItem::DetachedUnreachableCommit(_))));
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_vnode_fd_read_unavailable_is_not_a_changed_sample() {
        use std::os::fd::AsRawFd as _;

        let file = tempfile::NamedTempFile::new().unwrap();
        let pid = i32::try_from(std::process::id()).unwrap();
        assert!(super::mac_vnode_fd_info(pid, file.as_file().as_raw_fd(), false).is_ok());
        assert!(matches!(
            super::mac_vnode_fd_info(pid, -1, false),
            Err(super::CloseWriterGap::Unavailable)
        ));
        assert!(matches!(
            super::mac_vnode_fd_info(pid, -1, true),
            Err(super::CloseWriterGap::Unavailable)
        ));
    }

    #[test]
    fn mac_descriptor_flags_distinguish_readers_and_namespace_authority() {
        use super::{
            CloseDescriptorAccess as Access, CloseFileWriteAccess as Write,
            CloseWriterAuthority as Authority,
        };
        assert_eq!(super::mac_descriptor_authority(0o100_644, 1), Ok(None));
        assert_eq!(
            super::mac_descriptor_authority(0o100_644, 2),
            Ok(Some(Authority::WritableFile(Write::WriteOnly)))
        );
        assert_eq!(
            super::mac_descriptor_authority(0o100_644, 3),
            Ok(Some(Authority::WritableFile(Write::ReadWrite)))
        );
        assert_eq!(
            super::mac_descriptor_authority(0o040_755, 1),
            Ok(Some(Authority::DirectoryDescriptor(Access::ReadOnly)))
        );
        assert!(super::mac_descriptor_authority(0o100_644, 0).is_err());
        for share_mode in 0..10 {
            assert!(!super::mac_mapping_is_writer(1, share_mode));
            assert_eq!(
                super::mac_mapping_is_writer(3, share_mode),
                matches!(share_mode, 4 | 5 | 7)
            );
        }
    }

    #[test]
    fn process_disappearance_and_pid_reuse_are_gaps_not_writers_or_clean() {
        let super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority {
            process,
            ..
        }) = injected_writer_evidence()
        else {
            panic!("fixture")
        };
        let mut reused = process.clone();
        reused.incarnation = OpaqueIdentity::parse("different-birth").unwrap();
        for after in [
            Ok(None),
            Err(super::CloseWriterGap::Unavailable),
            Ok(Some(reused)),
        ] {
            for observed in [
                None,
                Some((
                    PathBuf::from("/target"),
                    super::CloseWriterAuthority::WorkingDirectory,
                )),
            ] {
                assert!(matches!(
                    super::stable_writer_observation(
                        super::CloseWriterDetector::MacCwd,
                        process.clone(),
                        observed,
                        after.clone()
                    ),
                    super::ExternalWriterEvidence::Observed(
                        super::CloseWriterObservation::InspectionGap {
                            reason: super::CloseWriterGap::ProcessChanged,
                            ..
                        }
                    )
                ));
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_writer_process_expired_deadline_precedes_missing_or_malformed_identity() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["17", "not-a-pid"] {
            let result = super::linux_writer_process_until(
                &temp.path().join(name),
                "test-boot",
                std::time::Instant::now(),
            );
            assert!(matches!(result, Err(super::WriterScanError::Stopped(_))));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_process_owner_expired_deadline_precedes_missing_or_malformed_status() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("17");
        std::fs::create_dir(&path).unwrap();
        let process = std::fs::read_dir(temp.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        for status in [None, Some("malformed status")] {
            if let Some(status) = status {
                std::fs::write(path.join("status"), status).unwrap();
            }
            let result =
                super::linux_process_owner_until(&process, 1, "test", std::time::Instant::now());
            assert!(matches!(result, Err(super::WriterScanError::Stopped(_))));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_zombie_expired_deadline_precedes_missing_tasks_or_malformed_status() {
        let temp = tempfile::tempdir().unwrap();
        for status in [
            "malformed status",
            "State:\tZ (zombie)\nThreads:\t1\nUid:\t1\t1\t1\t1\n",
        ] {
            let result = super::linux_zombie_has_no_sibling_threads_until(
                &temp.path().join("17"),
                status,
                std::time::Instant::now(),
            );
            assert!(matches!(result, Err(super::WriterScanError::Stopped(_))));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_writer_scan_expired_deadline_stops_without_persisting_a_gap() {
        let called = std::cell::Cell::new(false);
        let result = super::linux_writer_scan_until(
            Path::new("/not-accessed"),
            Path::new("/not-accessed"),
            0,
            super::CloseWriterDetector::LinuxMapping,
            std::time::Instant::now(),
            |_, _| {
                called.set(true);
                Ok(None)
            },
        );
        assert!(matches!(result, Err(message) if message.contains("deadline")));
        assert!(!called.get());
    }

    #[cfg(target_os = "linux")]
    fn fake_linux_process(proc_root: &Path, pid: u32, uid: u32, start: u64) -> PathBuf {
        let process = proc_root.join(pid.to_string());
        std::fs::create_dir_all(process.join("fd")).unwrap();
        std::fs::create_dir(process.join("fdinfo")).unwrap();
        std::fs::create_dir_all(proc_root.join("sys/kernel/random")).unwrap();
        std::fs::write(proc_root.join("sys/kernel/random/boot_id"), "test-boot").unwrap();
        std::fs::write(
            process.join("status"),
            format!("State:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        let mut fields = vec!["0".to_owned(); 20];
        fields[0] = "S".into();
        fields[19] = start.to_string();
        std::fs::write(
            process.join("stat"),
            format!("{pid} (fake process) {}\n", fields.join(" ")),
        )
        .unwrap();
        std::os::unix::fs::symlink("/test/executable", process.join("exe")).unwrap();
        std::os::unix::fs::symlink("/outside", process.join("cwd")).unwrap();
        std::fs::write(process.join("maps"), "").unwrap();
        process
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_writer_scan_propagates_detector_stop_without_gap_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&target).unwrap();
        fake_linux_process(&proc_root, 17, 1, 42);
        let inspected = std::cell::Cell::new(false);
        let result = super::linux_writer_scan_until(
            &target,
            &proc_root,
            1,
            super::CloseWriterDetector::LinuxMapping,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
            |_, _| {
                inspected.set(true);
                Err(super::WriterScanError::Stopped("fixture stop".to_owned()))
            },
        );
        assert!(inspected.get());
        assert!(matches!(result, Err(message) if message == "fixture stop"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_writer_scan_keeps_searching_after_an_unavailable_process() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&target).unwrap();
        fake_linux_process(&proc_root, 17, 1, 42);
        fake_linux_process(&proc_root, 18, 1, 43);
        let mut visited = Vec::new();
        let evidence = super::linux_writer_scan(
            &target,
            &proc_root,
            1,
            super::CloseWriterDetector::LinuxCwd,
            |process, canonical| {
                visited.push(process.file_name().unwrap().to_owned());
                if visited.len() == 1 {
                    Err(super::CloseWriterGap::Unavailable)
                } else {
                    Ok(Some((
                        canonical.to_path_buf(),
                        super::CloseWriterAuthority::WorkingDirectory,
                    )))
                }
            },
        )
        .unwrap();
        assert_eq!(visited.len(), 2);
        assert!(matches!(
            evidence,
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority {
                process,
                matched_path,
                ..
            }) if visited[1] == process.pid.to_string().as_str() && matched_path == target.canonicalize().unwrap()
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_writer_scan_missing_pre_owner_status_is_attributed_gap_for_each_detector() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&target).unwrap();
        let process = fake_linux_process(&proc_root, 17, 1, 42);
        std::fs::remove_file(process.join("status")).unwrap();

        for detector in [
            super::CloseWriterDetector::LinuxDescriptor,
            super::CloseWriterDetector::LinuxCwd,
            super::CloseWriterDetector::LinuxMapping,
        ] {
            let inspected = std::cell::Cell::new(false);
            let result = super::linux_writer_scan_until(
                &target,
                &proc_root,
                1,
                detector,
                std::time::Instant::now() + std::time::Duration::from_secs(30),
                |_, _| {
                    inspected.set(true);
                    Ok(None)
                },
            )
            .unwrap();

            assert!(
                !inspected.get(),
                "inspected process with missing owner status"
            );
            assert!(matches!(
                result,
                super::ExternalWriterEvidence::Observed(
                    super::CloseWriterObservation::InspectionGap {
                        detector: observed_detector,
                        pid: Some(17),
                        reason: super::CloseWriterGap::Unavailable,
                        ..
                    }
                ) if observed_detector == detector
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_zombie_classification_requires_unambiguous_kernel_state() {
        assert_eq!(
            super::linux_process_is_zombie("State:\tZ (zombie)\n"),
            Ok(true)
        );
        assert_eq!(
            super::linux_process_is_zombie("State:\tS (sleeping)\nName: zombie\n"),
            Ok(false)
        );
        for status in [
            "",
            "State:\n",
            "State: unknown\n",
            "State: ZZ\n",
            "State: Z\nState: S\n",
        ] {
            assert!(super::linux_process_is_zombie(status).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_zombie_without_executable_does_not_hide_writers_or_live_gaps() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let zombie = fake_linux_process(&proc_root, 3, 1, 42);
        std::fs::write(
            zombie.join("status"),
            "State:\tZ (zombie)\nThreads:\t1\nUid:\t1\t1\t1\t1\n",
        )
        .unwrap();
        std::fs::create_dir_all(zombie.join("task/3")).unwrap();
        let stat = std::fs::read_to_string(zombie.join("stat")).unwrap();
        std::fs::write(zombie.join("stat"), stat.replace(") S ", ") Z ")).unwrap();
        for name in ["exe", "cwd", "maps"] {
            std::fs::remove_file(zombie.join(name)).unwrap();
        }
        for detector in [
            super::CloseWriterDetector::LinuxDescriptor,
            super::CloseWriterDetector::LinuxCwd,
            super::CloseWriterDetector::LinuxMapping,
        ] {
            let scan = || {
                super::linux_writer_scan(&quarantine, &proc_root, 1, detector, |_, _| {
                    Ok(Some((
                        quarantine.clone(),
                        super::CloseWriterAuthority::WorkingDirectory,
                    )))
                })
                .unwrap()
            };
            assert!(matches!(
                scan(),
                super::ExternalWriterEvidence::NoPositiveEvidence
            ));
            let live = fake_linux_process(&proc_root, 4, 1, 43);
            assert!(
                matches!(scan(), super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority { process, .. }) if process.pid == 4)
            );
            std::fs::remove_file(live.join("exe")).unwrap();
            assert!(matches!(
                scan(),
                super::ExternalWriterEvidence::Observed(
                    super::CloseWriterObservation::InspectionGap { pid: Some(4), .. }
                )
            ));
            std::fs::remove_dir_all(live).unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_zombie_leader_with_sibling_threads_remains_a_gap() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let leader = fake_linux_process(&proc_root, 3, 1, 42);
        std::fs::create_dir_all(leader.join("task/3")).unwrap();
        std::fs::create_dir(leader.join("task/4")).unwrap();
        std::fs::remove_file(leader.join("exe")).unwrap();
        for threads in ["2", "1", "unknown"] {
            std::fs::write(
                leader.join("status"),
                format!("State:\tZ (zombie)\nThreads:\t{threads}\nUid:\t1\t1\t1\t1\n"),
            )
            .unwrap();
            let evidence = super::linux_writer_scan(
                &quarantine,
                &proc_root,
                1,
                super::CloseWriterDetector::LinuxCwd,
                |_, _| Ok(None),
            )
            .unwrap();
            assert!(matches!(
                evidence,
                super::ExternalWriterEvidence::Observed(
                    super::CloseWriterObservation::InspectionGap { pid: Some(3), .. }
                )
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_live_to_zombie_during_observation_is_a_gap() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let process = fake_linux_process(&proc_root, 3, 1, 42);
        let evidence = super::linux_writer_scan(
            &quarantine,
            &proc_root,
            1,
            super::CloseWriterDetector::LinuxCwd,
            |_, _| {
                std::fs::write(
                    process.join("status"),
                    "State:\tZ (zombie)\nThreads:\t1\nUid:\t1\t1\t1\t1\n",
                )
                .unwrap();
                Ok(None)
            },
        )
        .unwrap();
        assert!(matches!(
            evidence,
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                reason: super::CloseWriterGap::ProcessChanged,
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_descriptor_scan_classifies_access_and_retains_identity() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let file = quarantine.join("open-file");
        std::fs::write(&file, "data").unwrap();
        // SAFETY: geteuid has no preconditions and does not inspect processes.
        let uid = unsafe { libc::geteuid() };
        let process = fake_linux_process(&proc_root, 1274, uid, 44);
        std::os::unix::fs::symlink(&file, process.join("fd/3")).unwrap();
        std::fs::write(process.join("fdinfo/3"), "pos:\t0\nflags:\t0100000\n").unwrap();
        assert_eq!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::NoPositiveEvidence
        );
        for (flags, expected) in [
            ("0100001", super::CloseFileWriteAccess::WriteOnly),
            ("0100002", super::CloseFileWriteAccess::ReadWrite),
        ] {
            std::fs::write(process.join("fdinfo/3"), format!("flags:\t{flags}\n")).unwrap();
            assert!(
                matches!(super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
                super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority { detector: super::CloseWriterDetector::LinuxDescriptor, process, matched_path, authority: super::CloseWriterAuthority::WritableFile(access) })
                if process.pid == 1274 && process.incarnation.as_str() == "test-boot:44" && process.executable == Path::new("/test/executable") && matched_path == file && access == expected)
            );
        }
        std::fs::remove_file(process.join("fd/3")).unwrap();
        std::os::unix::fs::symlink(&quarantine, process.join("fd/3")).unwrap();
        std::fs::write(process.join("fdinfo/3"), "flags:\t0100000\n").unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority {
                authority: super::CloseWriterAuthority::DirectoryDescriptor(
                    super::CloseDescriptorAccess::ReadOnly
                ),
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_descriptor_scan_exempts_only_the_exact_live_self_borrow() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let other = temp.path().join("other");
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::create_dir(&other).unwrap();
        let witness = capture_linux_descriptor_test_slot(&quarantine, "quarantine");
        let borrow = witness.self_descriptor_borrow().unwrap();
        // SAFETY: geteuid and getpid have no preconditions.
        let uid = unsafe { libc::geteuid() };
        let pid = unsafe { libc::getpid() };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

        let make_proc = |root: &Path, process_pid: u32, fd: i32, target: &Path| {
            fake_linux_readonly_descriptor(root, process_pid, uid, fd, target);
        };

        let ordinary_proc = temp.path().join("ordinary-proc");
        make_proc(&ordinary_proc, pid.cast_unsigned(), borrow.fd, &quarantine);
        assert_linux_unscoped_directory_authority(&quarantine, &ordinary_proc, deadline);

        let exact_proc = temp.path().join("exact-proc");
        make_proc(&exact_proc, pid.cast_unsigned(), borrow.fd, &quarantine);
        assert_linux_scoped_descriptor_repeatedly_exempt(
            &quarantine,
            &exact_proc,
            deadline,
            &borrow,
        );
        // An exempt slot must not stop inventory inspection before other authority.
        let mixed_process = exact_proc.join(pid.to_string());
        let extra_fd = borrow.fd + 100;
        std::os::unix::fs::symlink(&quarantine, mixed_process.join(format!("fd/{extra_fd}")))
            .unwrap();
        std::fs::write(
            mixed_process.join(format!("fdinfo/{extra_fd}")),
            "flags:\t0100000\n",
        )
        .unwrap();
        assert_linux_scoped_descriptor_authority(&quarantine, &exact_proc, deadline, &borrow);

        let duplicate_proc = temp.path().join("duplicate-proc");
        make_proc(
            &duplicate_proc,
            pid.cast_unsigned(),
            borrow.fd + 1,
            &quarantine,
        );
        assert_linux_scoped_descriptor_authority(&quarantine, &duplicate_proc, deadline, &borrow);

        let other_pid_proc = temp.path().join("other-pid-proc");
        make_proc(
            &other_pid_proc,
            pid.cast_unsigned() + 1,
            borrow.fd,
            &quarantine,
        );
        assert_linux_scoped_descriptor_authority(&quarantine, &other_pid_proc, deadline, &borrow);

        let other_witness = capture_linux_descriptor_test_slot(&other, "other");
        let other_borrow = other_witness.self_descriptor_borrow().unwrap();
        let mismatched_borrow = super::SelfDeletionDescriptorBorrow {
            witness: &other_witness,
            pid: borrow.pid,
            fd: borrow.fd,
            device: other_borrow.device,
            inode: other_borrow.inode,
            kind: other_borrow.kind,
        };
        let mismatch_proc = temp.path().join("mismatch-proc");
        make_proc(&mismatch_proc, pid.cast_unsigned(), borrow.fd, &quarantine);
        assert_linux_scoped_descriptor_authority(
            &quarantine,
            &mismatch_proc,
            deadline,
            &mismatched_borrow,
        );

        // Keeping the FD alive must not excuse replacement of its witnessed slot.
        let retained_path = temp.path().join("retained-quarantine");
        std::fs::rename(&quarantine, &retained_path).unwrap();
        std::fs::create_dir(&quarantine).unwrap();
        let replaced_proc = temp.path().join("replaced-proc");
        make_proc(
            &replaced_proc,
            pid.cast_unsigned(),
            borrow.fd,
            &retained_path,
        );
        assert_linux_scoped_descriptor_authority(&retained_path, &replaced_proc, deadline, &borrow);

        let uncertain_proc = temp.path().join("uncertain-proc");
        let uncertain = fake_linux_process(&uncertain_proc, pid.cast_unsigned(), uid, 44);
        std::os::unix::fs::symlink(&quarantine, uncertain.join(format!("fd/{}", borrow.fd)))
            .unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_with_info_reader_until_scoped(
                &quarantine,
                &uncertain_proc,
                deadline,
                |path| std::fs::read_to_string(path),
                Some(&borrow),
            )
            .unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                reason: super::CloseWriterGap::DescriptorChanged,
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    fn fake_linux_readonly_descriptor(
        root: &Path,
        process_pid: u32,
        uid: libc::uid_t,
        fd: i32,
        target: &Path,
    ) {
        let process = fake_linux_process(root, process_pid, uid, 44);
        std::os::unix::fs::symlink(target, process.join(format!("fd/{fd}"))).unwrap();
        std::fs::write(
            process.join(format!("fdinfo/{fd}")),
            "pos:\t0\nflags:\t0100000\n",
        )
        .unwrap();
    }

    #[cfg(target_os = "linux")]
    fn capture_linux_descriptor_test_slot(
        path: &Path,
        slot_name: &str,
    ) -> super::OwnedDeletionSlot {
        let parent = super::open_deletion_parent(path.parent().unwrap()).unwrap();
        let name = std::ffi::CString::new(slot_name).unwrap();
        let identity = super::DeletionEntryIdentity::from_stat(
            &super::deletion_slot_stat(&parent, &name).unwrap(),
        );
        super::OwnedDeletionSlot::capture(&parent, &name, identity).unwrap()
    }

    #[cfg(target_os = "linux")]
    fn assert_linux_unscoped_directory_authority(
        quarantine: &Path,
        proc_root: &Path,
        deadline: std::time::Instant,
    ) {
        assert!(matches!(
            super::quarantine_has_open_descriptors_with_info_reader_until(
                quarantine,
                proc_root,
                deadline,
                |path| std::fs::read_to_string(path),
            )
            .unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority {
                authority: super::CloseWriterAuthority::DirectoryDescriptor(_),
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    fn assert_linux_scoped_descriptor_repeatedly_exempt(
        quarantine: &Path,
        proc_root: &Path,
        deadline: std::time::Instant,
        borrow: &super::SelfDeletionDescriptorBorrow<'_>,
    ) {
        // Check the initial scan and two repeats against the same live witness.
        for _ in 0..3 {
            assert_eq!(
                super::quarantine_has_open_descriptors_with_info_reader_until_scoped(
                    quarantine,
                    proc_root,
                    deadline,
                    |path| std::fs::read_to_string(path),
                    Some(borrow),
                )
                .unwrap(),
                super::ExternalWriterEvidence::NoPositiveEvidence
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn assert_linux_scoped_descriptor_authority(
        quarantine: &Path,
        proc_root: &Path,
        deadline: std::time::Instant,
        borrow: &super::SelfDeletionDescriptorBorrow<'_>,
    ) {
        assert!(matches!(
            super::quarantine_has_open_descriptors_with_info_reader_until_scoped(
                quarantine,
                proc_root,
                deadline,
                |path| std::fs::read_to_string(path),
                Some(borrow),
            )
            .unwrap(),
            super::ExternalWriterEvidence::Observed(
                super::CloseWriterObservation::Authority { .. }
            )
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_self_descriptor_device_preserves_unsigned_libproc_bits() {
        // SAFETY: libc::stat is plain integer storage, initialized before use.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        stat.st_dev = 0x80000001_u32 as i32;
        assert_eq!(super::deletion_descriptor_device(&stat), 0x80000001_u64);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_readonly_descriptor_position_changes_are_not_writer_gaps() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let file = quarantine.join("read-only-file");
        std::fs::write(&file, "data").unwrap();
        // SAFETY: geteuid has no preconditions and does not inspect processes.
        let process = fake_linux_process(&proc_root, 1274, unsafe { libc::geteuid() }, 44);
        std::os::unix::fs::symlink(&file, process.join("fd/3")).unwrap();
        std::fs::write(process.join("fdinfo/3"), "pos:\t0\nflags:\t0100000\n").unwrap();
        let mut reads = 0;
        let evidence = super::quarantine_has_open_descriptors_with_info_reader(
            &quarantine,
            &proc_root,
            |path| {
                let info = std::fs::read_to_string(path)?;
                reads += 1;
                std::fs::write(path, "pos:\t4\nflags:\t0100000\n")?;
                Ok(info)
            },
        )
        .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(evidence, super::ExternalWriterEvidence::NoPositiveEvidence);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_descriptor_scan_retains_inventory_and_descriptor_gaps() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                pid: None,
                ..
            })
        ));
        // SAFETY: geteuid has no preconditions.
        let process = fake_linux_process(&proc_root, 1274, unsafe { libc::geteuid() }, 44);
        std::os::unix::fs::symlink(&quarantine, process.join("fd/3")).unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                reason: super::CloseWriterGap::DescriptorChanged,
                ..
            })
        ));
        std::fs::write(process.join("fdinfo/3"), "flags:\tnot-octal\n").unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                reason: super::CloseWriterGap::Malformed,
                ..
            })
        ));
        std::fs::remove_file(process.join("fd/3")).unwrap();
        std::fs::write(process.join("fd/3"), "not a link").unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                reason: super::CloseWriterGap::DescriptorChanged,
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_descriptor_scan_retains_attributed_unavailable_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        // SAFETY: geteuid has no preconditions.
        let process = fake_linux_process(&proc_root, 1274, unsafe { libc::geteuid() }, 44);
        std::fs::remove_dir(process.join("fd")).unwrap();
        assert!(matches!(
            super::quarantine_has_open_descriptors_in(&quarantine, &proc_root).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                detector: super::CloseWriterDetector::LinuxDescriptor,
                pid: Some(1274),
                reason: super::CloseWriterGap::Unavailable,
            })
        ));
        assert!(quarantine.is_dir());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_cwd_scan_proves_namespace_authority_and_fails_closed_on_gaps() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let process = fake_linux_process(&proc_root, 3, 1, 42);
        std::fs::remove_file(process.join("cwd")).unwrap();
        assert!(matches!(
            super::quarantine_has_process_cwd_in(&quarantine, &proc_root, 1).unwrap(),
            super::ExternalWriterEvidence::Observed(
                super::CloseWriterObservation::InspectionGap { .. }
            )
        ));
        std::os::unix::fs::symlink(&quarantine, process.join("cwd")).unwrap();
        assert!(matches!(
            super::quarantine_has_process_cwd_in(&quarantine, &proc_root, 1).unwrap(),
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority {
                authority: super::CloseWriterAuthority::WorkingDirectory,
                ..
            })
        ));
        std::fs::write(process.join("status"), "malformed credentials").unwrap();
        assert!(matches!(
            super::quarantine_has_process_cwd_in(&quarantine, &proc_root, 1).unwrap(),
            super::ExternalWriterEvidence::Observed(
                super::CloseWriterObservation::InspectionGap { .. }
            )
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_mapping_scan_requires_writable_shared_authority() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let process = fake_linux_process(&proc_root, 3, 1, 42);
        let mapped = quarantine.join("mapped file");
        for permissions in ["r--s", "rw-p", "rw-s"] {
            let mut maps = b"1000-2000 rw-s 00000000 00:00 1 /unrelated/\xff\n".to_vec();
            maps.extend_from_slice(
                format!(
                    "0000-1000 {permissions} 00000000 00:00 1 {}\n",
                    mapped.display()
                )
                .as_bytes(),
            );
            std::fs::write(process.join("maps"), maps).unwrap();
            let evidence =
                super::quarantine_has_writable_mappings_in(&quarantine, &proc_root, 1).unwrap();
            if permissions == "rw-s" {
                assert!(
                    matches!(evidence, super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::Authority { matched_path, authority: super::CloseWriterAuthority::WritableSharedMapping, .. }) if matched_path == mapped)
                );
            } else {
                assert_eq!(evidence, super::ExternalWriterEvidence::NoPositiveEvidence);
            }
        }
        std::fs::remove_file(process.join("maps")).unwrap();
        assert!(matches!(
            super::quarantine_has_writable_mappings_in(&quarantine, &proc_root, 1).unwrap(),
            super::ExternalWriterEvidence::Observed(
                super::CloseWriterObservation::InspectionGap { .. }
            )
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cwd_and_mapping_scans_treat_unavailable_proc_inventory_as_gap() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::write(&proc_root, b"not a proc directory").unwrap();

        for evidence in [
            super::quarantine_has_process_cwd_in(&quarantine, &proc_root, 1).unwrap(),
            super::quarantine_has_writable_mappings_in(&quarantine, &proc_root, 1).unwrap(),
        ] {
            assert!(matches!(
                evidence,
                super::ExternalWriterEvidence::Observed(
                    super::CloseWriterObservation::InspectionGap { .. }
                )
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_pid_reuse_during_observation_retains_gap() {
        let temp = tempfile::tempdir().unwrap();
        let quarantine = temp.path().join("quarantine");
        let proc_root = temp.path().join("proc");
        std::fs::create_dir(&quarantine).unwrap();
        let process = fake_linux_process(&proc_root, 3, 1, 42);
        let evidence = super::linux_writer_scan(
            &quarantine,
            &proc_root,
            1,
            super::CloseWriterDetector::LinuxCwd,
            |_, _| {
                let stat = std::fs::read_to_string(process.join("stat")).unwrap();
                std::fs::write(process.join("stat"), stat.replace(" 42", " 43")).unwrap();
                Ok(Some((
                    quarantine.clone(),
                    super::CloseWriterAuthority::WorkingDirectory,
                )))
            },
        )
        .unwrap();
        assert!(matches!(
            evidence,
            super::ExternalWriterEvidence::Observed(super::CloseWriterObservation::InspectionGap {
                reason: super::CloseWriterGap::ProcessChanged,
                ..
            })
        ));
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
    fn porcelain_loss_parser_distinguishes_loss_categories_and_leaves_collapsed_ignored_paths_for_enumeration(
    ) {
        let losses =
            parse_status_losses(b"M  staged\0 M unstaged\0MM both\0?? untracked\0!! ignored/\0");
        assert_eq!(losses.len(), 5);
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::StagedTrackedPath(path) if path.as_bytes() == b"staged")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"unstaged")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::StagedTrackedPath(path) if path.as_bytes() == b"both")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::UnstagedTrackedPath(path) if path.as_bytes() == b"both")));
        assert!(losses.iter().any(|loss| matches!(loss, CloseLossItem::UntrackedNonIgnoredPath(path) if path.as_bytes() == b"untracked")));
    }

    #[cfg(unix)]
    #[test]
    fn ignored_directory_inventory_preserves_each_leaf_identity() {
        let (temp, _) = synthetic_inspection_repository();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut leaf = |command: std::process::Command, _, operation: &str| {
            assert_eq!(operation, "exact ignored-path inspection");
            let args = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert!(args.ends_with(
                &[
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "-z",
                    "--"
                ]
                .map(str::to_owned)
            ));
            Ok(synthetic_inspection_output(
                b"out/cache/a\0out/cache/b\0".to_vec(),
                true,
            ))
        };
        let paths =
            observe_ignored_paths_with_leaf_until(temp.path(), deadline, &mut leaf).unwrap();
        assert_eq!(
            paths
                .iter()
                .map(GitPathIdentity::as_bytes)
                .collect::<Vec<_>>(),
            [b"out/cache/a".as_slice(), b"out/cache/b".as_slice()]
        );
    }

    #[test]
    fn ignored_status_and_exact_paths_change_canonical_snapshot() {
        let clean = snapshot_for(&canonical_status_observation(b""));
        let collapsed = canonical_status_observation(b"!! target/\0");
        assert_ne!(clean, snapshot_for(&collapsed));

        let exact = vec![GitPathIdentity::from_bytes(b"target/a.log".to_vec())];
        let mut with_exact = collapsed;
        append_ignored_paths(&mut with_exact, &exact);
        assert_ne!(clean, snapshot_for(&with_exact));
        assert!(with_exact
            .windows(b"IGNORED_PATH\0target/a.log\0".len())
            .any(|window| window == b"IGNORED_PATH\0target/a.log\0"));
    }

    #[test]
    fn canonical_status_preserves_rename_source_record() {
        assert_eq!(
            canonical_status_observation(b"R  new-name.txt\0old-name.txt\0!! ignored\0"),
            b"R  new-name.txt\0old-name.txt\0!! ignored\0"
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

        let paths = [b"tracked".to_vec()];
        let entries = staged_index_entries_for_paths_until(
            temp.path(),
            &paths,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .unwrap();
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
