//! Retained original-context physical retirement for clean linked worktrees.
//! Git children, metadata writes and individual filesystem mutations enter their
//! own original source gate; inspection and descriptor traversal do not hold a
//! `SQLite` writer. Unsupported states remain preserved, never successful absence.

use std::{
    fmt::Write as _,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use sha2::{Digest, Sha256};
use tokio::sync::watch;

use phoenix_core::{
    domain::close::{
        CapturedWorktreeIdentity, CloseAttemptId, CloseInspection, CloseRetirementSnapshot,
        LossItemIdentity, RetiredResourceKind,
    },
    work_scope::WorkScopeId,
};
use phoenix_db::validate_close_retirement_inspection_aggregate;

use std::path::{Path, PathBuf};

use super::super::super::{
    ensure_no_ignored_content_with_status_leaf_until, inspect_worktree_at_with_git_leaf_until,
    observe_administrative_dir_incarnation,
    quarantine_has_external_writer_until_with_self_descriptor,
    remove_identity_bound_directory_with_driver, stop_bound_fsmonitor_with_leaf_until,
    worktree_quarantine_path, CloseWriterProbe, SelfDeletionDescriptorBorrow,
    WorktreeDeletionDriver, WorktreeMutationExecutor,
};
use super::git_leaf::{
    bind_shared_root_command, require_absent, run_git_leaf, GitLeafTarget, RetainedRootLocation,
};
use super::mutation::OriginalWorktreeMutationExecutor;
use super::{NativeCloseIntent, NativeWorktreeGitContinuation, WorkerAuthority, WorkerCustody};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreePhysicalOutcome {
    Retired,
    StopFailed { detail: String },
    ReinspectionRequired { detail: String },
    Residual { detail: String },
}

/// Observation has no execution authority. Dropping an observer (or an observe
/// future) detaches it; it cannot cancel the already installed native worker.
/// Results are cloned, not taken, so observation is repeatable.
pub(crate) struct WorktreePhysicalObservation {
    state: Arc<PhysicalState>,
    ready: watch::Receiver<bool>,
    fatal: Arc<crate::runtime::FatalLocalAuthorityFence>,
    _worker: Option<JoinHandle<()>>,
}

struct PhysicalState {
    result: Mutex<Option<Result<WorktreePhysicalOutcome, String>>>,
    custody: Mutex<Option<PhysicalCustody>>,
    ready: watch::Sender<bool>,
    #[cfg(test)]
    retry_hooks: Option<crate::runtime::executor::native_close_retry::test_hooks::Hooks>,
}

struct PhysicalCustody {
    authority: WorkerAuthority,
    git: WorkerCustody,
    writer_probe: CloseWriterProbe,
    // Per-Worktree content confirmation, never the aggregate metadata authority.
    confirmed_worktree_snapshot: Option<CloseRetirementSnapshot>,
    location: Option<RetainedRootLocation>,
}

impl NativeWorktreeGitContinuation {
    /// Consumes the owner and its original context. No controller, deadline,
    /// repository lock, scope or receipt is minted or reacquired for this handoff.
    pub(crate) fn spawn_physical(
        self,
        runtime: tokio::runtime::Handle,
        mut writer_probe: CloseWriterProbe,
    ) -> WorktreePhysicalObservation {
        writer_probe
            .persistence_uncertain
            .get_or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let (ready_tx, ready) = watch::channel(false);
        let fatal = Arc::clone(&self.original_effect.fatal);
        let observation_fatal = Arc::clone(&fatal);
        // Custody exists before installing the thread, not inside an async body.
        // The shared slot also preserves it if OS thread installation fails.
        let state = Arc::new(PhysicalState {
            result: Mutex::new(None),
            custody: Mutex::new(Some(PhysicalCustody {
                authority: WorkerAuthority { fatal, armed: true },
                git: WorkerCustody {
                    prepared: Some(self.owner),
                    original_effect: Some(self.original_effect),
                    child: Mutex::new(None),
                    stdout_reader: None,
                    stderr_reader: None,
                },
                writer_probe,
                confirmed_worktree_snapshot: None,
                location: None,
            })),
            ready: ready_tx,
            #[cfg(test)]
            retry_hooks: crate::runtime::executor::native_close_retry::test_hooks::Hooks::current(),
        });
        let worker_state = Arc::clone(&state);
        let worker = std::thread::Builder::new()
            .name("native-close-worktree-physical".into())
            .spawn(move || run_physical_worker(&worker_state, &runtime));
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                // No installed worker can retain the original authority here.
                // Keep custody and the unsignalled observer channel permanently;
                // publishing an ordinary failure would falsely release authority.
                let _retained = Arc::into_raw(Arc::clone(&state));
                let slot = state
                    .custody
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(custody) = slot.as_ref() {
                    custody
                        .authority
                        .fatal
                        .close("native_close_initial_worktree_physical_install_failed");
                }
                tracing::error!(%error, "cannot install native physical Worktree worker; custody retained");
                None
            }
        };
        WorktreePhysicalObservation {
            state,
            ready,
            fatal: observation_fatal,
            _worker: worker,
        }
    }
}

impl WorktreePhysicalObservation {
    /// Nonblocking actor progress check; an unfinished worker must not prevent
    /// Stop/Shutdown or fatal-fence servicing in the source actor.
    pub(crate) fn is_ready(&self) -> bool {
        *self.ready.borrow()
    }

    /// A latched fence means the worker retained unknown custody; waiting for a
    /// classified observation cannot make that authority releasable.
    pub(crate) fn has_fatal_custody(&self) -> bool {
        self.fatal.fatal_authority_loss_is_latched()
    }

    pub(crate) async fn observe(&self) -> Result<WorktreePhysicalOutcome, String> {
        let mut ready = self.ready.clone();
        while !*ready.borrow() {
            ready.changed().await.map_err(|_| {
                "native physical Worktree worker ended without classification".to_string()
            })?;
        }
        self.state
            .result
            .lock()
            .map_err(|_| "native physical Worktree result mutex poisoned".to_string())?
            .clone()
            .ok_or_else(|| "native physical Worktree worker has no classification".to_string())?
    }
}

fn run_physical_worker(state: &PhysicalState, runtime: &tokio::runtime::Handle) {
    let mut slot = state
        .custody
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let custody = slot
        .as_mut()
        .expect("physical custody installed before spawn");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        retire_clean_linked_worktree(custody, runtime)
    }));
    let Ok(result) = result else {
        retain_unclassified_physical(custody);
    };
    // A result is not publishable while any child/pipe custody is unclassified.
    // Git leaves retain on unknown DB release or incomplete cleanup.
    let child_pending = match custody.git.child.lock() {
        Ok(child) => child.is_some(),
        Err(_) => retain_unclassified_physical(custody),
    };
    let release_classified = custody
        .git
        .original_effect
        .as_ref()
        .is_some_and(|context| !context.leaf_release_pending_classification);
    if child_pending
        || custody.git.stdout_reader.is_some()
        || custody.git.stderr_reader.is_some()
        || !release_classified
        || custody
            .writer_probe
            .persistence_uncertain
            .as_ref()
            .is_some_and(|uncertain| uncertain.load(std::sync::atomic::Ordering::SeqCst))
    {
        retain_unclassified_physical(custody);
    }
    let Ok(mut result_slot) = state.result.lock() else {
        retain_unclassified_physical(custody);
    };
    custody.authority.armed = false;
    // Every dispatched child, mutation and metadata release is classified.
    drop(slot.take());
    *result_slot = Some(Ok(result));
    drop(result_slot);
    drop(slot);
    #[cfg(test)]
    runtime.block_on(crate::runtime::executor::native_close_retry::test_hooks::checkpoint(
        state.retry_hooks.as_ref(),
        crate::runtime::executor::native_close_retry::test_hooks::Point::PhysicalBeforeObservation,
    ));
    state.ready.send_replace(true);
}

fn retire_clean_linked_worktree(
    custody: &mut PhysicalCustody,
    runtime: &tokio::runtime::Handle,
) -> WorktreePhysicalOutcome {
    if let Err(detail) = validate_physical_binding(custody) {
        return WorktreePhysicalOutcome::Residual { detail };
    }
    custody.confirmed_worktree_snapshot = match read_confirmed_worktree_snapshot(custody, runtime) {
        Ok(snapshot) => Some(snapshot),
        Err(detail) => return WorktreePhysicalOutcome::Residual { detail },
    };
    let owner = custody
        .git
        .prepared
        .as_ref()
        .expect("original physical owner");
    let path = owner.prepared.inspection_path.clone();
    let identity = owner.prepared.identity.clone();
    let administrative_dir = owner.git_dir.clone();
    let administrative_incarnation = owner.git_dir_incarnation.clone();
    let expected = identity.fingerprint().as_str().to_string();
    let deadline = owner.prepared.deadline;
    // This is an original captured-location operation, not a resume adapter.
    // Even a dangling link in the legacy quarantine namespace blocks it.
    let qualification = (|| {
        require_absent(&worktree_quarantine_path(&identity)?)?;
        ensure_no_administrative_residue(
            &administrative_dir,
            &administrative_incarnation,
            deadline,
        )?;
        RetainedRootLocation::capture(owner)
    })();
    custody.location = match qualification {
        Ok(location) => Some(location),
        Err(detail) => return WorktreePhysicalOutcome::Residual { detail },
    };
    let mut driver = PhysicalDeletionDriver {
        custody,
        runtime,
        administrative: false,
        root: None,
        failure: None,
        mutation_refusal: None,
    };
    if let Err(detail) = driver.verify_confirmed_snapshot() {
        return driver
            .failure
            .take()
            .unwrap_or(WorktreePhysicalOutcome::Residual { detail });
    }
    driver.custody.authority.armed = true;
    if let Err(detail) = driver.original().record_cleanup_plan(
        runtime,
        administrative_dir.clone(),
        administrative_incarnation.clone(),
    ) {
        return WorktreePhysicalOutcome::Residual { detail };
    }
    let deletion = remove_identity_bound_directory_with_driver(
        &path,
        None,
        &expected,
        "native captured worktree",
        Some(deadline),
        &mut driver,
    );
    if let Err(detail) = deletion {
        return driver
            .failure
            .take()
            .unwrap_or_else(|| WorktreePhysicalOutcome::Residual {
                detail: driver.retained_detail(&detail),
            });
    }
    // No Git is spawned after checkout deletion. Administrative removal uses
    // the immutable registration bytes, captured common-dir identity and same lock.
    driver.administrative = true;
    driver.root = None;
    let administrative_root = administrative_dir
        .parent()
        .expect("captured registration parent")
        .join(format!(
            "{}{}",
            administrative_final_residue_prefix(&administrative_incarnation),
            uuid::Uuid::new_v4()
        ));
    if let Err(detail) = remove_identity_bound_directory_with_driver(
        &administrative_dir,
        Some(&administrative_root),
        &administrative_incarnation,
        "native worktree administrative directory",
        Some(deadline),
        &mut driver,
    ) {
        return WorktreePhysicalOutcome::Residual {
            detail: driver.retained_detail(&detail),
        };
    }
    if let Err(detail) = driver.verify_retired_paths(&administrative_dir) {
        return WorktreePhysicalOutcome::Residual { detail };
    }
    WorktreePhysicalOutcome::Retired
}

fn validate_physical_binding(custody: &PhysicalCustody) -> Result<(), String> {
    if custody
        .writer_probe
        .persistence_uncertain
        .as_ref()
        .is_some_and(|uncertain| uncertain.load(std::sync::atomic::Ordering::SeqCst))
    {
        retain_unclassified_physical(custody);
    }
    let original = custody
        .git
        .original_effect
        .as_ref()
        .expect("original physical context");
    let probe = &custody.writer_probe;
    if probe.run.attempt_id != original.receipt.admitted.attempt_id
        || probe.run.ordinal != original.receipt.admitted.run_ordinal
        || probe.scope != original.scope
        || probe.snapshot != original.snapshot
        || probe.resource != original.resource
    {
        return Err(
            "writer probe does not match original Worktree run binding; target preserved".into(),
        );
    }
    Ok(())
}

// This reads one immutable snapshot from the captured registration and checks
// its exact worktree identity; splitting the checks would obscure that proof.
#[allow(clippy::too_many_lines)]
fn read_confirmed_worktree_snapshot(
    custody: &PhysicalCustody,
    runtime: &tokio::runtime::Handle,
) -> Result<CloseRetirementSnapshot, String> {
    let original = custody
        .git
        .original_effect
        .as_ref()
        .expect("original physical context");
    let owner = custody
        .git
        .prepared
        .as_ref()
        .expect("original physical owner");
    super::check_deadline(original.deadline())?;
    if original.resource.kind() != RetiredResourceKind::Worktree
        || !matches!(original.resource.identity(), LossItemIdentity::Worktree(identity)
            if identity == &owner.prepared.identity)
    {
        return Err(
            "original Worktree resource does not match captured physical owner; target preserved"
                .into(),
        );
    }
    if matches!(
        original.command.intent(),
        NativeCloseIntent::ExplicitRetry { .. }
    ) {
        let context = runtime
            .block_on(original.db.read_native_close_retry_execution_context(
                &original.command,
                &original.receipt.admitted,
            ))
            .map_err(|error| error.to_string())?;
        if original.snapshot != context.snapshot {
            return Err(
                "native retry physical inspection snapshot changed; target preserved".into(),
            );
        }
        let mut scopes = context
            .scopes
            .iter()
            .filter(|scope| scope.scope == original.scope);
        match (scopes.next(), scopes.next()) {
            (Some(scope), None)
                if scope.attempt_id == original.receipt.admitted.attempt_id
                    && matches!(&scope.captured_worktree,
                        Some(CapturedWorktreeIdentity::Resolved(identity))
                        if identity == &owner.prepared.identity) => {}
            _ => return Err("native retry physical inspection lacks one exact captured Worktree binding; target preserved".into()),
        }
        let mut resources = context.resources.iter().filter(|expected| {
            expected.scope == original.scope && expected.resource == original.resource
        });
        match (resources.next(), resources.next()) {
            (Some(expected), None)
                if expected.attempt_id == original.receipt.admitted.attempt_id
                    && expected.snapshot == context.snapshot => {}
            _ => return Err("native retry physical inspection lacks one exact resource binding; target preserved".into()),
        }
        let selected = |effect: &&phoenix_db::CloseSafeRetryEffect| {
            effect.scope == original.scope && effect.resource == original.resource
        };
        if context.effects.iter().filter(selected).count() != 1
            || context.progress.pending.iter().filter(selected).count() != 1
            || context.progress.successful.iter().any(|effect| {
                effect.scope == original.scope && effect.resource == original.resource
            })
        {
            return Err("native retry physical inspection requires its exact pending Worktree effect; target preserved".into());
        }
        let mut snapshots = context
            .worktree_snapshots
            .iter()
            .filter(|(target, _)| target.scope == original.scope);
        let confirmed = match (snapshots.next(), snapshots.next()) {
            (Some((_, snapshot)), None) => snapshot.clone(),
            _ => return Err("native retry physical inspection requires one exact Worktree snapshot; target preserved".into()),
        };
        super::check_deadline(original.deadline())?;
        return Ok(confirmed);
    }
    let snapshot = runtime.block_on(async {
        let attempt = original.receipt.admitted.attempt_id.as_str();
        // CloseInspection contains only a scope, not a Worktree identity. Bind
        // that scope through the immutable attempt capture, not a live locator.
        let scopes = original.db.list_close_attempt_scopes(attempt).await
            .map_err(|error| error.to_string())?;
        let mut captured = scopes.iter().filter(|captured| captured.scope == original.scope);
        let captured = match (captured.next(), captured.next()) {
            (Some(captured), None) if captured.attempt_id.as_str() == attempt
                && matches!(&captured.captured_worktree,
                    Some(CapturedWorktreeIdentity::Resolved(identity))
                    if identity == &owner.prepared.identity) => captured,
            _ => return Err("original Close inspection lacks one exact captured Worktree binding; target preserved".into()),
        };
        let inspections = original.db.list_close_retirement_inspections(attempt).await
            .map_err(|error| error.to_string())?;
        let confirmed = select_confirmed_worktree_snapshot(
            &original.receipt.admitted.attempt_id, &captured.scope, &original.snapshot, &inspections,
        )?;
        // Capture seals the complete authorized inventory before minting this
        // immutable aggregate anchor. A current/replacement row or matching
        // content alone is not proof of the originally accepted inspection.
        // This is a read only: all effect/metadata admissions still use the
        // unchanged original.snapshot and recheck their own original gates.
        let anchored: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM close_original_retirement_anchors anchor
                JOIN close_obligations obligation ON obligation.attempt_id = anchor.attempt_id
                WHERE anchor.attempt_id = ?1
                  AND anchor.inspection_generation = ?2
                  AND anchor.inspection_fingerprint = ?3
                  AND anchor.inspection_generation = obligation.inspection_generation
                  AND anchor.inspection_fingerprint = obligation.inspection_fingerprint
            ) AND NOT EXISTS (
                SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = ?1
            )",
        )
        .bind(attempt)
        .bind(original.snapshot.generation())
        .bind(original.snapshot.fingerprint())
        .fetch_one(original.db.pool())
        .await
        .map_err(|error| error.to_string())?;
        if !anchored {
            return Err("Close inspection does not match the original accepted retirement anchor; target preserved".into());
        }
        Ok::<_, String>(confirmed)
    })?;
    super::check_deadline(original.deadline())?;
    Ok(snapshot)
}

fn select_confirmed_worktree_snapshot(
    attempt: &CloseAttemptId,
    scope: &WorkScopeId,
    original_snapshot: &CloseRetirementSnapshot,
    inspections: &[CloseInspection],
) -> Result<CloseRetirementSnapshot, String> {
    if inspections
        .iter()
        .any(|inspection| &inspection.attempt_id != attempt)
    {
        return Err("Close inspection belongs to a different attempt; target preserved".into());
    }
    // Verify BOTH generation and fingerprint for the entire original aggregate.
    // A later same-content reinspection must not become original confirmation.
    validate_close_retirement_inspection_aggregate(original_snapshot, inspections)
        .map_err(|error| error.to_string())?;
    let mut matching = inspections
        .iter()
        .filter(|inspection| &inspection.target.scope == scope);
    match (matching.next(), matching.next()) {
        (Some(inspection), None) => Ok(inspection.snapshot.clone()),
        _ => Err(
            "Close requires exactly one original confirmed Worktree inspection; target preserved"
                .into(),
        ),
    }
}

fn administrative_final_residue_prefix(incarnation: &str) -> String {
    let digest = Sha256::digest(incarnation.as_bytes());
    let mut suffix = String::with_capacity(16);
    for byte in &digest[..8] {
        write!(&mut suffix, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!(".phoenix-admin-delete-{suffix}-")
}

fn ensure_no_administrative_residue(
    administrative_dir: &Path,
    incarnation: &str,
    deadline: std::time::Instant,
) -> Result<(), String> {
    let registrations = administrative_dir
        .parent()
        .ok_or_else(|| "captured worktree administrative directory has no parent".to_string())?;
    if administrative_dir.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .starts_with(b".phoenix-admin-delete-")
    }) {
        return Err(
            "native physical retirement does not resume administrative final residues".into(),
        );
    }
    let prefix = administrative_final_residue_prefix(incarnation);
    for entry in std::fs::read_dir(registrations).map_err(|error| error.to_string())? {
        super::check_deadline(deadline)?;
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(prefix.as_bytes())
        {
            return Err(format!(
                "captured administrative final residue survives at {}; worktree preserved",
                entry.path().display()
            ));
        }
    }
    super::check_deadline(deadline)
}

/// One mutable owner orchestrates callback phases and per-syscall admission.
/// Each method borrows the original context briefly; no executor reference is
/// held across a canonical inspection or a durable metadata callback.
struct PhysicalDeletionDriver<'a> {
    custody: &'a mut PhysicalCustody,
    runtime: &'a tokio::runtime::Handle,
    administrative: bool,
    root: Option<PathBuf>,
    failure: Option<WorktreePhysicalOutcome>,
    // Per-leaf executors borrow the context only briefly. Keep their stop latch
    // on the graph owner so creating the next executor cannot renew admission.
    mutation_refusal: Option<String>,
}

impl PhysicalDeletionDriver<'_> {
    fn original(&mut self) -> &mut super::OriginalWorktreeEffectContext {
        self.custody
            .git
            .original_effect
            .as_mut()
            .expect("original physical context")
    }

    fn deadline(&self) -> std::time::Instant {
        self.custody
            .git
            .original_effect
            .as_ref()
            .expect("original physical context")
            .deadline()
    }

    fn location(&self) -> &RetainedRootLocation {
        self.custody
            .location
            .as_ref()
            .expect("captured relocation witness")
    }

    fn retained_detail(&self, detail: &str) -> String {
        format!(
            "{detail}; retained native cleanup target at {}",
            self.root
                .as_deref()
                .unwrap_or_else(|| self.location().path())
                .display()
        )
    }

    fn git_leaf(
        &mut self,
        mut command: std::process::Command,
        deadline: std::time::Instant,
        operation: &str,
    ) -> Result<std::process::Output, String> {
        let owner = self
            .custody
            .git
            .prepared
            .as_ref()
            .expect("original physical owner");
        let location = self
            .custody
            .location
            .as_ref()
            .expect("captured relocation witness");
        bind_shared_root_command(&mut command, owner, location)?;
        run_git_leaf(
            &mut self.custody.git,
            self.runtime,
            &mut self.custody.authority,
            command,
            deadline,
            operation,
            GitLeafTarget::RetainedRoot(location),
        )
    }

    fn verify_confirmed_snapshot(&mut self) -> Result<(), String> {
        let path = self.location().path().to_path_buf();
        let owner = self
            .custody
            .git
            .prepared
            .as_ref()
            .expect("original physical owner");
        self.location().validate(owner)?;
        let identity = owner.prepared.identity.clone();
        let confirmed = self
            .custody
            .confirmed_worktree_snapshot
            .as_ref()
            .ok_or_else(|| {
                "original confirmed Worktree inspection is unavailable; target preserved"
                    .to_string()
            })?
            .clone();
        let deadline = self.deadline();
        let (fresh, losses) = inspect_worktree_at_with_git_leaf_until(
            &identity,
            &path,
            deadline,
            &mut |command, deadline, operation| self.git_leaf(command, deadline, operation),
        )?;
        if !losses.is_empty() {
            return Err("native physical retirement does not support worktree loss state; worktree preserved".into());
        }
        if fresh.fingerprint() != confirmed.fingerprint() {
            let detail = self.retained_detail(
                "worktree changed after Close inspection confirmation; fresh confirmation is required",
            );
            self.failure = Some(WorktreePhysicalOutcome::ReinspectionRequired {
                detail: detail.clone(),
            });
            return Err(detail);
        }
        // Ignored leaves are fingerprinted and reported as losses during
        // inspection; this guard independently refuses physical deletion.
        ensure_no_ignored_content_with_status_leaf_until(&path, deadline, |_, deadline| {
            let owner = self
                .custody
                .git
                .prepared
                .as_ref()
                .expect("original physical owner");
            let command =
                super::status_command(&path, &owner.git_dir, &owner.prepared.common_git_dir);
            self.git_leaf(command, deadline, "ignored-content inspection")
        })?;
        self.location().validate(
            self.custody
                .git
                .prepared
                .as_ref()
                .expect("original physical owner"),
        )
    }

    fn check_external_writers(
        &mut self,
        object: &Path,
        self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
    ) -> Result<(), String> {
        let evidence = self.custody.writer_probe.observe_and_retain_until(
            object,
            self.deadline(),
            |path, deadline| {
                quarantine_has_external_writer_until_with_self_descriptor(
                    path,
                    deadline,
                    self_descriptor,
                )
            },
        );
        if self
            .custody
            .writer_probe
            .persistence_uncertain
            .as_ref()
            .is_some_and(|uncertain| uncertain.load(std::sync::atomic::Ordering::SeqCst))
        {
            retain_unclassified_physical(self.custody);
        }
        // Only positive external authority vetoes this admitted physical step.
        // Target checks and persistence errors are never process-inspection gaps.
        if evidence?.is_positive() {
            return Err(format!(
                "external writer holds confirmed worktree at {}; durable evidence retained",
                object.display()
            ));
        }
        Ok(())
    }

    fn verify_retired_paths(&self, administrative_dir: &Path) -> Result<(), String> {
        super::check_deadline(self.deadline())?;
        require_absent(self.location().original_path())?;
        require_absent(self.location().path())?;
        if let Some(checkout_root) = self.location().path().parent() {
            require_absent(checkout_root)?;
        }
        require_absent(administrative_dir)?;
        if let Some(root) = &self.root {
            require_absent(root)?;
        }
        super::check_deadline(self.deadline())
    }
}

impl WorktreeMutationExecutor for PhysicalDeletionDriver<'_> {
    fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        if let Some(refusal) = &self.mutation_refusal {
            return Err(refusal.clone());
        }
        self.custody.authority.armed = true;
        let runtime = self.runtime;
        let result =
            OriginalWorktreeMutationExecutor::new(self.original(), runtime).execute(effect);
        if let Err(detail) = &result {
            self.mutation_refusal = Some(detail.clone());
        }
        result
    }
}

impl WorktreeDeletionDriver for PhysicalDeletionDriver<'_> {
    fn allows_existing_root(&self) -> bool {
        false
    }

    fn cleanup_after_failure(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn rename_to_tombstone(
        &mut self,
        leaf: super::super::super::OwnedWorktreeRenameLeaf,
        deadline: Option<std::time::Instant>,
    ) -> Result<std::io::Result<()>, String> {
        let registration = self
            .administrative
            .then(|| self.location().registration().clone());
        let checkout = if self.administrative {
            None
        } else {
            Some(self.location().checkout_rename_witness()?)
        };
        let original_checkout = self.location().original_path().to_path_buf();
        let moved_checkout = self.location().path().to_path_buf();
        self.execute(move || {
            super::super::super::check_directory_deletion_deadline(deadline)?;
            // BOTH cases validate immutable native proof AFTER admission. The
            // shared owned leaf then rechecks source/root descriptor identities
            // immediately before its exclusive, descriptor-relative rename.
            if let Some(registration) = registration {
                require_absent(&original_checkout)?;
                require_absent(&moved_checkout)?;
                registration.validate_rename_source(&leaf.source_path)?;
            }
            if let Some(checkout) = checkout {
                checkout.validate_source(&leaf.source_path)?;
            }
            leaf.rename(deadline)
        })
    }

    fn before_move(&mut self, source: &Path) -> Result<(), String> {
        self.custody.authority.armed = true;
        if self.administrative {
            let owner = self
                .custody
                .git
                .prepared
                .as_ref()
                .expect("original physical owner");
            self.location()
                .registration()
                .validate_rename_source(source)?;
            ensure_no_administrative_residue(source, &owner.git_dir_incarnation, self.deadline())?;
            require_absent(self.location().original_path())?;
            return require_absent(self.location().path());
        }
        if source != self.location().path() {
            return Err("final move source differs from retained Worktree location".into());
        }
        let deadline = self.deadline();
        let stopped = stop_bound_fsmonitor_with_leaf_until(
            source,
            deadline,
            &mut |command, deadline, operation| self.git_leaf(command, deadline, operation),
        );
        if let Err(detail) = stopped {
            self.failure = Some(WorktreePhysicalOutcome::StopFailed {
                detail: detail.clone(),
            });
            return Err(detail);
        }
        self.verify_confirmed_snapshot()
    }

    fn bind_root(
        &mut self,
        root: &Path,
        identity: (u64, u64),
        object: Option<(u64, u64)>,
    ) -> Result<(), String> {
        self.custody.authority.armed = true;
        self.root = Some(root.to_path_buf());
        if self.administrative {
            return Ok(());
        }
        let runtime = self.runtime;
        if let Some((device, inode)) = object {
            // Preserve the observed final-object evidence even if subsequent
            // identity/snapshot proofs refuse deletion. Metadata refusal stops
            // here, without another effect or compensating mutation.
            self.original().bind_final_object(runtime, device, inode)?;
            let owner = self
                .custody
                .git
                .prepared
                .as_ref()
                .expect("original physical owner");
            self.custody
                .location
                .as_mut()
                .expect("captured relocation witness")
                .confirm_owned_move(owner, &root.join("object"), (device, inode))
        } else {
            self.original()
                .bind_final_root(runtime, root.to_path_buf(), identity.0, identity.1)
        }
    }

    fn observe_identity(&mut self, object: &Path) -> Result<String, String> {
        if self.administrative {
            observe_administrative_dir_incarnation(object)
        } else {
            self.location().validate(
                self.custody
                    .git
                    .prepared
                    .as_ref()
                    .expect("original physical owner"),
            )?;
            phoenix_core::git::observe_linked_worktree_fingerprint(object)
                .ok_or_else(|| "cannot observe retained Worktree fingerprint".into())
        }
    }

    fn after_move(
        &mut self,
        _root: &Path,
        object: &Path,
        self_descriptor: Option<&SelfDeletionDescriptorBorrow<'_>>,
    ) -> Result<(), String> {
        if self.administrative {
            let owner = self
                .custody
                .git
                .prepared
                .as_ref()
                .expect("original physical owner");
            require_absent(&owner.git_dir)?;
            require_absent(self.location().original_path())?;
            require_absent(self.location().path())?;
            // Compare the moved records verbatim. In particular, ../.. still
            // denotes the ORIGINAL common-dir, not object/../.. in the residue.
            return self.location().registration().validate_at(object);
        }
        self.check_external_writers(object, self_descriptor)?;
        self.verify_confirmed_snapshot()?;
        #[cfg(test)]
        {
            use crate::runtime::executor::native_close_retry::test_hooks::{checkpoint, Point};

            let hooks = self
                .custody
                .git
                .original_effect
                .as_ref()
                .and_then(|context| context.test_hooks.as_ref());
            self.runtime
                .block_on(checkpoint(hooks, Point::PhysicalBeforeCheckoutRemoval));
        }
        require_absent(self.location().original_path())
    }
}

fn retain_unclassified_physical(custody: &PhysicalCustody) -> ! {
    custody
        .authority
        .fatal
        .close("native_close_initial_worktree_physical_unclassified");
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(60));
        std::hint::black_box(custody);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspection(scope: &str, generation: &str, fingerprint: &str) -> CloseInspection {
        CloseInspection {
            attempt_id: CloseAttemptId::parse("original-attempt").unwrap(),
            target: phoenix_core::domain::close::CloseRetirementTarget {
                scope: WorkScopeId::parse(scope).unwrap(),
            },
            snapshot: CloseRetirementSnapshot::parse(generation, fingerprint).unwrap(),
            inspected_at: chrono::Utc::now(),
        }
    }

    fn aggregate(inspections: &[CloseInspection]) -> CloseRetirementSnapshot {
        let encode = |generation: bool| {
            let mut encoded = String::from("v1");
            for inspection in inspections {
                let scope = inspection.target.scope.as_str();
                let value = if generation {
                    inspection.snapshot.generation()
                } else {
                    inspection.snapshot.fingerprint()
                };
                let _ = write!(encoded, "{}:{scope}{}:{value}", scope.len(), value.len());
            }
            encoded
        };
        CloseRetirementSnapshot::parse(encode(true), encode(false)).unwrap()
    }

    #[test]
    fn confirmed_content_uses_exact_scoped_snapshot_not_metadata_aggregate() {
        let inspections = [
            inspection("scope-a", "g1", "content-a"),
            inspection("scope-b", "g1", "content-b"),
        ];
        let original = aggregate(&inspections);
        let unchanged_original = original.clone();
        let confirmed = select_confirmed_worktree_snapshot(
            &inspections[1].attempt_id,
            &inspections[1].target.scope,
            &original,
            &inspections,
        )
        .unwrap();
        assert_eq!(confirmed, inspections[1].snapshot);
        assert_ne!(confirmed.fingerprint(), original.fingerprint());
        assert_eq!(original, unchanged_original);
    }

    #[test]
    fn replacement_inspection_cannot_replace_original_confirmation() {
        let mut inspections = [inspection("scope-a", "g1", "same-content")];
        let original = aggregate(&inspections);
        for replacement in [
            CloseRetirementSnapshot::parse("g2", "same-content").unwrap(),
            CloseRetirementSnapshot::parse("g1", "changed-content").unwrap(),
        ] {
            inspections[0].snapshot = replacement;
            assert!(select_confirmed_worktree_snapshot(
                &inspections[0].attempt_id,
                &inspections[0].target.scope,
                &original,
                &inspections,
            )
            .is_err());
        }
    }

    #[test]
    fn confirmed_inspection_requires_exactly_one_scope_in_the_exact_attempt() {
        let first = inspection("scope-a", "g1", "content-a");
        let attempt = first.attempt_id.clone();
        let scope = first.target.scope.clone();
        let inspections = [first.clone()];
        let original = aggregate(&inspections);
        assert!(select_confirmed_worktree_snapshot(
            &attempt,
            &WorkScopeId::parse("missing-scope").unwrap(),
            &original,
            &inspections,
        )
        .is_err());
        assert!(select_confirmed_worktree_snapshot(
            &CloseAttemptId::parse("other-attempt").unwrap(),
            &scope,
            &original,
            &inspections,
        )
        .is_err());
        let duplicate = [first.clone(), first];
        assert!(select_confirmed_worktree_snapshot(
            &attempt,
            &scope,
            &aggregate(&duplicate),
            &duplicate,
        )
        .is_err());
        assert!(select_confirmed_worktree_snapshot(
            &attempt,
            &scope,
            &CloseRetirementSnapshot::parse("g1", "no-worktree").unwrap(),
            &[],
        )
        .is_err());
    }

    fn completed_observation() -> WorktreePhysicalObservation {
        let (ready_tx, ready) = watch::channel(true);
        WorktreePhysicalObservation {
            state: Arc::new(PhysicalState {
                result: Mutex::new(Some(Ok(WorktreePhysicalOutcome::Residual {
                    detail: "preserved test residual".to_string(),
                }))),
                custody: Mutex::new(None),
                ready: ready_tx,
                retry_hooks: None,
            }),
            ready,
            fatal: crate::runtime::FatalLocalAuthorityFence::new(),
            _worker: None,
        }
    }

    #[test]
    fn fatal_physical_custody_is_distinguished_from_pending_ordinary_join() {
        let observation = completed_observation();
        observation.state.ready.send_replace(false);
        assert!(!observation.is_ready());
        assert!(!observation.has_fatal_custody());
        observation.fatal.close("test_unknown_physical_custody");
        assert!(observation.has_fatal_custody());
    }

    #[test]
    fn administrative_final_prefix_is_stable_and_incarnation_bound() {
        let prefix = administrative_final_residue_prefix("v1:17:29");
        assert_eq!(prefix, administrative_final_residue_prefix("v1:17:29"));
        assert_ne!(prefix, administrative_final_residue_prefix("v1:17:30"));
        let suffix = prefix.strip_prefix(".phoenix-admin-delete-").unwrap();
        let digest = suffix.strip_suffix('-').unwrap();
        assert_eq!(digest.len(), 16);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn administrative_final_random_children_are_recognized() {
        let incarnation = "v1:17:29";
        let prefix = administrative_final_residue_prefix(incarnation);
        for random_suffix in [
            "8cf54d97-6d67-490a-b70d-df3c033fe744",
            "2793fd85-931e-437a-8f11-df2bc6e5d64a",
        ] {
            let name = format!("{prefix}{random_suffix}");
            assert!(name.as_bytes().starts_with(prefix.as_bytes()));
            let result = ensure_no_administrative_residue(
                &Path::new("/captured/common/worktrees").join(name),
                incarnation,
                std::time::Instant::now() + std::time::Duration::from_secs(30),
            );
            assert!(result.unwrap_err().contains("does not resume"));
        }
    }

    #[test]
    fn administrative_final_namespace_is_not_a_native_resume_candidate() {
        let result = ensure_no_administrative_residue(
            Path::new("/captured/common/worktrees/.phoenix-admin-delete-retained"),
            "v1:17:29",
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        assert!(result.unwrap_err().contains("does not resume"));
    }

    #[tokio::test]
    async fn physical_residual_observation_is_repeatable() {
        let observation = completed_observation();
        let first = observation.observe().await.unwrap();
        let second = observation.observe().await.unwrap();
        assert_eq!(first, second);
        assert!(matches!(first, WorktreePhysicalOutcome::Residual { .. }));
    }

    #[tokio::test]
    async fn dropping_a_pending_observe_future_does_not_take_the_eventual_result() {
        use std::future::Future as _;

        let observation = completed_observation();
        let result = observation.state.result.lock().unwrap().take();
        observation.state.ready.send_replace(false);
        let mut pending = Box::pin(observation.observe());
        let polled =
            std::future::poll_fn(|context| std::task::Poll::Ready(pending.as_mut().poll(context)))
                .await;
        assert!(polled.is_pending());
        drop(pending);
        *observation.state.result.lock().unwrap() = result;
        observation.state.ready.send_replace(true);
        assert!(matches!(
            observation.observe().await.unwrap(),
            WorktreePhysicalOutcome::Residual { .. }
        ));
    }

    #[cfg(unix)]
    fn status_output(code: i32, stdout: &[u8], stderr: &[u8]) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn expired_original_budget_does_not_enter_the_status_leaf() {
        let result = ensure_no_ignored_content_with_status_leaf_until(
            std::path::Path::new("captured-worktree"),
            std::time::Instant::now(),
            |_, _| panic!("expired original budget dispatched a leaf"),
        );
        assert!(result.unwrap_err().contains("deadline"));
    }

    #[test]
    fn refused_status_leaf_is_not_retried() {
        let calls = std::cell::Cell::new(0);
        let result = ensure_no_ignored_content_with_status_leaf_until(
            std::path::Path::new("captured-worktree"),
            std::time::Instant::now() + std::time::Duration::from_secs(60),
            |_, _| {
                calls.set(calls.get() + 1);
                Err("original source admission refused".to_string())
            },
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(result.unwrap_err(), "original source admission refused");
    }

    #[cfg(unix)]
    #[test]
    fn ignored_content_is_preserved_by_the_shared_native_and_legacy_leaf_check() {
        let path = std::path::Path::new("captured-worktree");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let result = ensure_no_ignored_content_with_status_leaf_until(path, deadline, |p, d| {
            assert_eq!(p, path);
            assert_eq!(d, deadline);
            Ok(status_output(0, b"!! ignored-file\0", b""))
        });
        assert!(result.unwrap_err().contains("no exact discard authority"));
    }

    #[cfg(unix)]
    #[test]
    fn unsuccessful_status_cannot_establish_ignored_content_absence() {
        let result = ensure_no_ignored_content_with_status_leaf_until(
            std::path::Path::new("captured-worktree"),
            std::time::Instant::now() + std::time::Duration::from_secs(60),
            |_, _| Ok(status_output(1, b"", b"cannot inspect captured index")),
        );
        assert!(result
            .unwrap_err()
            .contains("cannot inspect captured index"));
    }
}
