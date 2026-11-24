//! Original native Worktree custody across individually admitted Git, metadata
//! and physical mutation leaves, retaining one source context and deadline.

use std::{
    path::{Component, Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Mutex},
    time::Instant,
};

use phoenix_core::{
    domain::{
        close::{
            CloseRetirementSnapshot, LossItemIdentity, RetiredResourceIdentity,
            RetiredResourceKind, WorktreeIdentity,
        },
        sm_state::ConvState,
    },
    work_scope::WorkScopeId,
};
use phoenix_db::{
    native_close::{
        NativeCloseCommand, NativeCloseInitialEntryRelease, NativeCloseInitialFirstEffectError,
        NativeCloseIntent, NativeCloseReceipt,
    },
    Database,
};

use phoenix_core::git::observe_linked_worktree_fingerprint;

use super::super::{
    observe_administrative_dir_incarnation, worktree_path, PreparedPhysicalWorktreeRemoval,
    RepositoryMutationLock,
};
use crate::runtime::{
    executor::{SourceEntryWorker, WorkerEntryResult},
    FatalLocalAuthorityFence,
};

const STDOUT_LIMIT: usize = 8 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

/// A non-cloneable lock and exact worktree binding, obtained under the original
/// absolute deadline. Preparation does not authorize any post-lock Git child.
pub(crate) struct NativeWorktreeGit {
    prepared: PreparedPhysicalWorktreeRemoval,
    git_dir: PathBuf,
    git_dir_incarnation: String,
}

pub(crate) fn prepare_original_worktree_git_until(
    identity: &WorktreeIdentity,
    original_deadline: Instant,
) -> Result<Option<NativeWorktreeGit>, String> {
    check_deadline(original_deadline)?;
    let inspection_path = worktree_path(identity);
    if !inspection_path
        .try_exists()
        .map_err(|error| error.to_string())?
    {
        return Ok(None);
    }
    let git_dir = captured_git_directory(&inspection_path, identity)?;
    let common_git_dir = common_directory(&git_dir)?;
    verify_linked_registration(&git_dir, &common_git_dir, &inspection_path)?;
    let git_dir_incarnation = observe_administrative_dir_incarnation(&git_dir)?;
    let common_git_dir_incarnation = observe_administrative_dir_incarnation(&common_git_dir)?;
    check_deadline(original_deadline)?;
    let repository_lock =
        RepositoryMutationLock::acquire_common_directory_until(&common_git_dir, original_deadline)
            .map_err(|(message, _)| message)?;
    check_deadline(original_deadline)?;
    if captured_git_directory(&inspection_path, identity)? != git_dir
        || common_directory(&git_dir)? != common_git_dir
        || verify_linked_registration(&git_dir, &common_git_dir, &inspection_path).is_err()
        || observe_administrative_dir_incarnation(&git_dir)? != git_dir_incarnation
        || observe_administrative_dir_incarnation(&common_git_dir)? != common_git_dir_incarnation
    {
        return Err("captured Worktree Git registration changed while acquiring lock".into());
    }
    check_deadline(original_deadline)?;
    Ok(Some(NativeWorktreeGit {
        prepared: PreparedPhysicalWorktreeRemoval {
            identity: identity.clone(),
            inspection_path,
            common_git_dir,
            common_git_dir_incarnation,
            deadline: original_deadline,
            repository_lock,
        },
        git_dir,
        git_dir_incarnation,
    }))
}

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("original Worktree Git deadline expired".into())
    } else {
        Ok(())
    }
}

// The fingerprint embeds the normalized .git pointer bytes; decode those
// bytes instead of asking Git for the registration before source authority.
fn captured_pointer(identity: &WorktreeIdentity) -> Result<Vec<u8>, String> {
    let fingerprint = identity.fingerprint().as_str();
    let encoded = fingerprint
        .strip_prefix("git_admin_incarnation_v2:")
        .and_then(|rest| rest.rsplit_once(':'))
        .map(|(_, encoded)| encoded)
        .ok_or("captured Worktree fingerprint has no registration binding")?;
    if encoded.len() % 2 != 0
        || encoded.is_empty()
        || encoded.len() > (libc::PATH_MAX as usize + 10) * 2
    {
        return Err("invalid captured Worktree registration encoding".into());
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    for pair in encoded.as_bytes().chunks_exact(2) {
        let hex = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
        bytes.push(u8::from_str_radix(hex, 16).map_err(|error| error.to_string())?);
    }
    if bytes.contains(&0) {
        return Err("captured Worktree registration contains NUL".into());
    }
    Ok(bytes)
}

pub(in crate::runtime::close_retirement) fn validate_captured_registration(
    identity: &WorktreeIdentity,
) -> Result<(), String> {
    let path = worktree_path(identity);
    let git_dir = captured_git_directory(&path, identity)?;
    let common_git_dir = common_directory(&git_dir)?;
    verify_linked_registration(&git_dir, &common_git_dir, &path)
}

fn captured_git_directory(path: &Path, identity: &WorktreeIdentity) -> Result<PathBuf, String> {
    let marker = std::fs::symlink_metadata(path.join(".git"))
        .map_err(|error| format!("cannot inspect captured Worktree .git: {error}"))?;
    if !marker.is_file() || marker.len() > libc::PATH_MAX as u64 + 10 {
        return Err("captured Worktree .git is not a bounded registration pointer".into());
    }
    if observe_linked_worktree_fingerprint(path).as_deref() != Some(identity.fingerprint().as_str())
    {
        return Err("captured Worktree .git fingerprint changed".into());
    }
    let pointer = super::super::path_buf_from_git_bytes(&captured_pointer(identity)?);
    let candidate = if pointer.is_absolute() {
        pointer
    } else {
        path.join(pointer)
    };
    candidate
        .canonicalize()
        .map_err(|error| format!("cannot resolve captured Worktree registration: {error}"))
}

fn common_directory(git_dir: &Path) -> Result<PathBuf, String> {
    let (_, bytes) = phoenix_core::git::read_worktree_registration_file(&git_dir.join("commondir"))
        .map_err(|error| format!("cannot read captured Worktree commondir: {error}"))?;
    // Strip only the record terminator, never significant pathname whitespace.
    let bytes = registration_record(&bytes);
    if bytes.is_empty() || bytes.contains(&0) {
        return Err("invalid captured Worktree commondir".into());
    }
    let pointer = super::super::path_buf_from_git_bytes(bytes);
    let candidate = if pointer.is_absolute() {
        pointer
    } else {
        git_dir.join(pointer)
    };
    candidate
        .canonicalize()
        .map_err(|error| format!("cannot resolve captured Worktree common directory: {error}"))
}

fn registration_record(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes)
}

fn lexical_absolute(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn verify_linked_registration(
    git_dir: &Path,
    common_git_dir: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let registrations = common_git_dir.join("worktrees");
    let metadata = std::fs::symlink_metadata(&registrations)
        .map_err(|error| format!("cannot inspect Worktree registrations: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("common Git Worktree registrations are not a real directory".into());
    }
    let registrations_real = registrations
        .canonicalize()
        .map_err(|error| format!("cannot resolve Worktree registrations: {error}"))?;
    if !registrations_real.starts_with(common_git_dir) {
        return Err("Worktree registrations escape common Git directory".into());
    }
    let name = git_dir
        .file_name()
        .ok_or("captured Worktree registration has no name")?;
    let registration = registrations.join(name);
    let registration_metadata = std::fs::symlink_metadata(&registration)
        .map_err(|error| format!("cannot inspect captured Worktree registration: {error}"))?;
    if !registration_metadata.is_dir() || registration_metadata.file_type().is_symlink() {
        return Err("captured Worktree registration is not a real directory".into());
    }
    let registration_real = registration
        .canonicalize()
        .map_err(|error| format!("cannot resolve captured Worktree registration: {error}"))?;
    if !registration_real.starts_with(&registrations_real)
        || registration_real != git_dir
        || git_dir.parent() != Some(registrations_real.as_path())
    {
        return Err("captured Worktree registration escapes common Git directory".into());
    }

    let (_, backlink_bytes) =
        phoenix_core::git::read_worktree_registration_file(&git_dir.join("gitdir"))
            .map_err(|error| format!("cannot read Worktree registration backlink: {error}"))?;
    let backlink = registration_record(&backlink_bytes);
    if backlink.is_empty() || backlink.contains(&0) {
        return Err("invalid Worktree registration backlink".into());
    }
    let backlink = super::super::path_buf_from_git_bytes(backlink);
    let backlink = if backlink.is_absolute() {
        backlink
    } else {
        git_dir.join(backlink)
    };
    let expected = worktree_path.join(".git");
    if lexical_absolute(&backlink)? != lexical_absolute(&expected)?
        || backlink.canonicalize().map_err(|error| error.to_string())?
            != expected.canonicalize().map_err(|error| error.to_string())?
    {
        return Err("Worktree registration backlink does not identify the captured source".into());
    }
    Ok(())
}

/// One retained operation per original Worktree resource. The caller must keep
/// this record keyed by that resource across observer cancellation/retry. Fresh
/// construction is not an at-most-once gate. Dropping an observer cannot cancel
/// the worker; dropping this record after completion loses continuation custody.
pub(crate) struct WorktreeGitObservation {
    state: Arc<Mutex<OperationState>>,
    ready: tokio::sync::watch::Receiver<bool>,
    _worker: std::thread::JoinHandle<()>,
}

struct OperationState {
    result: Option<Result<Output, String>>,
    continuation: Option<NativeWorktreeGitContinuation>,
}

/// The original source-entry capability and its unchanged deadline travel with
/// the prepared owner. This wrapper is deliberately affine even though the leaf
/// worker handle itself is cloneable elsewhere.
pub(crate) struct OriginalWorktreeEffectContext {
    db: Database,
    command: NativeCloseCommand,
    receipt: NativeCloseReceipt,
    live_source: ConvState,
    scope: WorkScopeId,
    snapshot: CloseRetirementSnapshot,
    resource: RetiredResourceIdentity,
    source_entry: SourceEntryWorker,
    deadline: Instant,
    fatal: Arc<FatalLocalAuthorityFence>,
    leaf_release_pending_classification: bool,
}

impl OriginalWorktreeEffectContext {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        db: Database,
        command: NativeCloseCommand,
        receipt: NativeCloseReceipt,
        live_source: ConvState,
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: RetiredResourceIdentity,
        source_entry: SourceEntryWorker,
        deadline: Instant,
        fatal: Arc<FatalLocalAuthorityFence>,
    ) -> Self {
        Self {
            db,
            command,
            receipt,
            live_source,
            scope,
            snapshot,
            resource,
            source_entry,
            deadline,
            fatal,
            leaf_release_pending_classification: false,
        }
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Admit exactly one bounded first-effect leaf against the original close tuple.
    /// The returned DB release must be classified before this context admits another
    /// leaf; callers that cannot prove confirmed release retain all worker custody.
    fn admitted_leaf<T>(
        &mut self,
        runtime: &tokio::runtime::Handle,
        effect: impl FnOnce() -> Result<T, String>,
    ) -> Result<
        (
            WorkerEntryResult<Result<T, String>>,
            NativeCloseInitialEntryRelease,
        ),
        NativeCloseInitialFirstEffectError,
    > {
        assert!(
            !self.leaf_release_pending_classification,
            "previous Worktree leaf release must be classified first"
        );
        self.leaf_release_pending_classification = true;
        let source_entry = self.source_entry.clone();
        let mut source_lease = None;
        let first_effect = || match source_entry.try_acquire_first_effect(&self.command) {
            WorkerEntryResult::Ran(lease) => {
                source_lease = Some(lease);
                WorkerEntryResult::Ran(check_deadline(self.deadline).and_then(|()| effect()))
            }
            WorkerEntryResult::Busy => WorkerEntryResult::Busy,
            WorkerEntryResult::Refused => WorkerEntryResult::Refused,
        };
        let result = match self.command.intent() {
            NativeCloseIntent::Initial { .. } => {
                runtime.block_on(self.db.with_native_close_initial_worktree_first_effect(
                    &self.command,
                    &self.receipt,
                    &self.live_source,
                    self.scope.clone(),
                    self.snapshot.clone(),
                    &self.resource,
                    first_effect,
                ))
            }
            NativeCloseIntent::ExplicitRetry { .. } => {
                runtime.block_on(self.db.with_native_close_retry_worktree_first_effect(
                    &self.command,
                    &self.receipt.admitted,
                    &self.live_source,
                    self.scope.clone(),
                    self.snapshot.clone(),
                    &self.resource,
                    first_effect,
                ))
            }
            NativeCloseIntent::Observe { .. } => {
                Err(NativeCloseInitialFirstEffectError::NotStarted(
                    phoenix_db::DbError::CloseFoundationPrecondition(
                        "observational Close cannot authorize a Worktree effect".into(),
                    ),
                ))
            }
        };
        match &result {
            Ok((_, NativeCloseInitialEntryRelease::Unconfirmed(_)))
            | Err(NativeCloseInitialFirstEffectError::Rejected {
                release: NativeCloseInitialEntryRelease::Unconfirmed(_),
                ..
            }) => {
                self.fatal
                    .close("native_close_worktree_unknown_writer_release");
                let _retained_result = (result, source_lease);
                loop {
                    std::thread::park();
                }
            }
            _ => {
                drop(source_lease);
                result
            }
        }
    }

    fn classify_confirmed_release(&mut self) {
        assert!(
            self.leaf_release_pending_classification,
            "no Worktree leaf release awaits classification"
        );
        self.leaf_release_pending_classification = false;
    }
}

/// Exact lock owner plus the original authority needed by a later bounded step.
pub(crate) struct NativeWorktreeGitContinuation {
    pub(crate) owner: NativeWorktreeGit,
    pub(crate) original_effect: OriginalWorktreeEffectContext,
}

impl WorktreeGitObservation {
    /// Reads a copy; neither the caller nor its cancelled future owns the lock.
    pub(crate) async fn observe(&self) -> Result<Output, String> {
        let mut ready = self.ready.clone();
        let is_ready = *ready.borrow_and_update();
        if !is_ready {
            ready
                .changed()
                .await
                .map_err(|_| "Worktree Git worker ended without result".to_string())?;
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .result
            .as_ref()
            .expect("worker published before notifying")
            .clone()
    }

    /// Synchronous one-time handoff of the exact prepared owner, only after a
    /// successful confirmed observation. No later effect is authorized here.
    pub(crate) fn take_for_continuation(&self) -> Option<NativeWorktreeGitContinuation> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(state.result, Some(Ok(_))) {
            return None;
        }
        state.continuation.take()
    }
}

struct WorkerCustody {
    prepared: Option<NativeWorktreeGit>,
    original_effect: Option<OriginalWorktreeEffectContext>,
    child: Mutex<Option<Child>>,
    stdout_reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr_reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
}

impl WorkerCustody {
    fn empty() -> Self {
        Self {
            prepared: None,
            original_effect: None,
            child: Mutex::new(None),
            stdout_reader: None,
            stderr_reader: None,
        }
    }
}

struct WorkerAuthority {
    fatal: Arc<FatalLocalAuthorityFence>,
    armed: bool,
}

impl Drop for WorkerAuthority {
    fn drop(&mut self) {
        if self.armed {
            self.fatal.close("native_close_initial_worktree_git_child");
        }
    }
}

impl NativeWorktreeGit {
    /// Starts the worker before yielding to the caller. Only the synchronous
    /// SQLite + original-source callback can call `Command::spawn`; neither a
    /// pre-lock probe nor the post-release wait can authorize another spawn.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_status(
        self,
        runtime: tokio::runtime::Handle,
        db: Database,
        command: NativeCloseCommand,
        receipt: NativeCloseReceipt,
        live_source: ConvState,
        scope: WorkScopeId,
        snapshot: CloseRetirementSnapshot,
        resource: RetiredResourceIdentity,
        source_entry: SourceEntryWorker,
        fatal: Arc<FatalLocalAuthorityFence>,
    ) -> WorktreeGitObservation {
        let original_deadline = self.prepared.deadline;
        let original_effect = OriginalWorktreeEffectContext {
            db,
            command,
            receipt,
            live_source,
            scope,
            snapshot,
            resource,
            source_entry,
            deadline: original_deadline,
            fatal,
            leaf_release_pending_classification: false,
        };
        NativeWorktreeGitContinuation {
            owner: self,
            original_effect,
        }
        .spawn_status(runtime)
    }
}

impl NativeWorktreeGitContinuation {
    /// Re-enters the same database and live-source gates after the preceding
    /// child and its pipe readers have completed. The original owner, receipt,
    /// source worker, and absolute deadline move intact into the next worker.
    pub(crate) fn spawn_status(self, runtime: tokio::runtime::Handle) -> WorktreeGitObservation {
        let state = Arc::new(Mutex::new(OperationState {
            result: None,
            continuation: None,
        }));
        let worker_state = Arc::clone(&state);
        let (ready_tx, ready) = tokio::sync::watch::channel(false);
        let worker = std::thread::spawn(move || {
            let mut authority = WorkerAuthority {
                fatal: Arc::clone(&self.original_effect.fatal),
                armed: true,
            };
            let mut custody = WorkerCustody {
                prepared: Some(self.owner),
                original_effect: Some(self.original_effect),
                child: Mutex::new(None),
                stdout_reader: None,
                stderr_reader: None,
            };
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_status(&mut custody, &runtime, &mut authority)
            })) {
                Ok(result) => result,
                Err(_) => {
                    authority
                        .fatal
                        .close("native_close_initial_worktree_worker_panic");
                    retain_unclassified(custody)
                }
            };
            {
                let mut state = worker_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if result.is_ok() {
                    state.continuation = Some(NativeWorktreeGitContinuation {
                        owner: custody
                            .prepared
                            .take()
                            .expect("successful status retains prepared owner"),
                        original_effect: custody
                            .original_effect
                            .take()
                            .expect("successful status retains original effect context"),
                    });
                }
                state.result = Some(result);
            }
            let _ = ready_tx.send(true);
        });
        WorktreeGitObservation {
            state,
            ready,
            _worker: worker,
        }
    }
}

fn run_status(
    custody: &mut WorkerCustody,
    runtime: &tokio::runtime::Handle,
    authority: &mut WorkerAuthority,
) -> Result<Output, String> {
    let owner = custody
        .prepared
        .as_ref()
        .expect("worker owns prepared lock");
    let command = status_command(
        &owner.prepared.inspection_path,
        &owner.git_dir,
        &owner.prepared.common_git_dir,
    );
    let deadline = owner.prepared.deadline;
    let output = git_leaf::run_git_leaf(
        custody,
        runtime,
        authority,
        command,
        deadline,
        "Git status",
        git_leaf::GitLeafTarget::CapturedRoot,
    )?;
    git_leaf::require_success(output, "Git status")
}

fn status_command(path: &Path, git_dir: &Path, common_git_dir: &Path) -> Command {
    const REPOSITORY_ENV: &[&str] = &[
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_PREFIX",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_QUARANTINE_PATH",
    ];
    let mut command = phoenix_core::git::command_with_config(&[("core.fsmonitor", "false")]);
    for name in REPOSITORY_ENV {
        command.env_remove(name);
    }
    command.env_remove("GIT_CONFIG");
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_CONFIG_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_DIR", git_dir)
        .env("GIT_COMMON_DIR", common_git_dir)
        .env("GIT_WORK_TREE", path)
        .current_dir(path)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

fn wait_confirmed_git(custody: &mut WorkerCustody, deadline: Instant) -> Result<Output, String> {
    let mut slot = custody
        .child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let child = slot.as_mut().expect("confirmed child in custody");
    // Check the pair before taking either handle: even malformed caller stdio
    // must not drop one pipe while retaining the other under unknown completion.
    if child.stdout.is_none() || child.stderr.is_none() {
        drop(slot);
        retain_incomplete_status(custody);
    }
    let stdout = child.stdout.take().expect("checked stdout pipe");
    let stderr = child.stderr.take().expect("checked stderr pipe");
    custody.stdout_reader = Some(std::thread::spawn(move || {
        super::super::read_git_pipe(stdout, Some(STDOUT_LIMIT))
    }));
    custody.stderr_reader = Some(std::thread::spawn(move || {
        super::super::read_git_pipe(stderr, Some(STDERR_LIMIT))
    }));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status))
                if custody
                    .stdout_reader
                    .as_ref()
                    .is_some_and(std::thread::JoinHandle::is_finished)
                    && custody
                        .stderr_reader
                        .as_ref()
                        .is_some_and(std::thread::JoinHandle::is_finished)
                    && Instant::now() < deadline =>
            {
                break Some(status)
            }
            Ok(_) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let Some(status) = status else {
        // Never wait unboundedly for a child or inherited pipe after the original
        // deadline. Keep every owner and close admission instead of dropping the
        // lock or publishing an error that would permit a later effect.
        drop(slot);
        retain_incomplete_status(custody);
    };
    let stdout = custody
        .stdout_reader
        .take()
        .expect("stdout reader in custody")
        .join();
    let stderr = custody
        .stderr_reader
        .take()
        .expect("stderr reader in custody")
        .join();
    if stdout.is_err() || stderr.is_err() {
        panic!("Worktree Git pipe reader panicked");
    }
    let stdout = stdout
        .expect("joined reader")
        .map_err(|error| error.to_string())?;
    let stderr = stderr
        .expect("joined reader")
        .map_err(|error| error.to_string())?;
    slot.take();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn retain_incomplete_status(custody: &mut WorkerCustody) -> ! {
    custody
        .original_effect
        .as_ref()
        .expect("worker retains original effect context")
        .fatal
        .close("native_close_initial_worktree_status_completion_unproven");
    retain_unclassified(std::mem::replace(custody, WorkerCustody::empty()))
}

fn retain_unclassified(custody: WorkerCustody) -> ! {
    loop {
        std::thread::park();
        std::hint::black_box(&custody);
    }
}

mod metadata;
mod physical;
pub(crate) use physical::{WorktreePhysicalObservation, WorktreePhysicalOutcome};

impl NativeWorktreeGit {
    pub(crate) fn into_continuation(
        self,
        original_effect: OriginalWorktreeEffectContext,
    ) -> Result<NativeWorktreeGitContinuation, String> {
        if self.prepared.deadline != original_effect.deadline
            || original_effect.resource.kind() != RetiredResourceKind::Worktree
            || !matches!(original_effect.resource.identity(),
                LossItemIdentity::Worktree(identity) if identity == &self.prepared.identity)
        {
            return Err("original Worktree continuation identity or deadline differs".into());
        }
        check_deadline(original_effect.deadline)?;
        Ok(NativeWorktreeGitContinuation {
            owner: self,
            original_effect,
        })
    }
}

mod git_leaf;
mod mutation;

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::executor::SourceEntryController;

    #[test]
    fn status_command_clears_inherited_repository_selectors() {
        let command = status_command(
            Path::new("/captured/worktree"),
            Path::new("/captured/git"),
            Path::new("/captured/common"),
        );
        let env = command
            .get_envs()
            .collect::<std::collections::HashMap<_, _>>();
        for name in ["GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_NAMESPACE"] {
            assert_eq!(env.get(std::ffi::OsStr::new(name)), Some(&None));
        }
        assert_eq!(
            env.get(std::ffi::OsStr::new("GIT_DIR")).copied().flatten(),
            Some(std::ffi::OsStr::new("/captured/git")),
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("GIT_COMMON_DIR"))
                .copied()
                .flatten(),
            Some(std::ffi::OsStr::new("/captured/common")),
        );
    }

    #[test]
    fn registration_reader_refuses_special_files_and_symlinks() {
        use std::os::unix::{ffi::OsStrExt as _, fs::symlink};

        let temp = tempfile::tempdir().unwrap();
        let fifo = temp.path().join("commondir");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // No writer is opened: a blocking open would never complete.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(phoenix_core::git::read_worktree_registration_file(&fifo).is_err());
        assert!(common_directory(temp.path()).is_err());
        let regular = temp.path().join("regular");
        std::fs::write(&regular, b"../..\n").unwrap();
        let link = temp.path().join("gitdir");
        symlink(&regular, &link).unwrap();
        assert!(phoenix_core::git::read_worktree_registration_file(&link).is_err());
        assert!(phoenix_core::git::read_worktree_registration_file(temp.path()).is_err());
        assert_eq!(
            phoenix_core::git::read_worktree_registration_file(&regular)
                .unwrap()
                .1,
            b"../..\n"
        );
    }

    #[test]
    fn common_directory_preserves_significant_pathname_whitespace() {
        let temp = tempfile::tempdir().unwrap();
        let admin = temp.path().join("admin");
        let different = temp.path().join("common");
        let exact = temp.path().join("common \t");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(&different).unwrap();
        std::fs::create_dir_all(&exact).unwrap();
        std::fs::write(admin.join("commondir"), b"../common \t\n").unwrap();
        assert_eq!(
            common_directory(&admin).unwrap(),
            exact.canonicalize().unwrap()
        );
        assert_ne!(
            common_directory(&admin).unwrap(),
            different.canonicalize().unwrap()
        );
        assert_eq!(registration_record(b"name \t\r\n"), b"name \t");
    }

    #[test]
    fn bounded_git_capture_drains_but_refuses_oversized_output() {
        let data = std::io::Cursor::new(vec![b'x'; STDERR_LIMIT + 1]);
        assert!(super::super::super::read_git_pipe(data, Some(STDERR_LIMIT)).is_err());
        let data = std::io::Cursor::new(vec![b'x'; STDERR_LIMIT]);
        assert_eq!(
            super::super::super::read_git_pipe(data, Some(STDERR_LIMIT))
                .unwrap()
                .len(),
            STDERR_LIMIT
        );
    }

    #[tokio::test]
    async fn preparation_uses_captured_pointer_and_commondir_without_git() {
        use phoenix_core::domain::close::{GitPathIdentity, WorktreeFingerprint, WorktreeId};

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let worktree = temp.path().join("worktree");
        let admin = root.join(".git/worktrees/wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir(&worktree).unwrap();
        std::fs::write(admin.join("commondir"), b"../..\n").unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .unwrap();
        let fingerprint = observe_linked_worktree_fingerprint(&worktree).unwrap();
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("wt").unwrap(),
            WorktreeFingerprint::parse(fingerprint).unwrap(),
            GitPathIdentity::from_bytes(worktree.to_string_lossy().as_bytes().to_vec()),
        );
        let prepared = prepare_original_worktree_git_until(
            &identity,
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            prepared.prepared.common_git_dir,
            root.join(".git").canonicalize().unwrap()
        );
        assert_eq!(prepared.git_dir, admin.canonicalize().unwrap());
        assert_eq!(
            prepared.git_dir_incarnation,
            observe_administrative_dir_incarnation(&admin).unwrap()
        );
        assert_eq!(
            captured_git_directory(&worktree, &identity).unwrap(),
            admin.canonicalize().unwrap()
        );

        assert!(verify_linked_registration(
            &prepared.git_dir,
            &prepared.prepared.common_git_dir,
            &worktree
        )
        .is_ok());
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", temp.path().join("other/.git").display()),
        )
        .unwrap();
        assert!(verify_linked_registration(
            &prepared.git_dir,
            &prepared.prepared.common_git_dir,
            &worktree
        )
        .is_err());
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let registrations = root.join(".git/worktrees");
            let renamed_admin = root.join(".git/worktrees/wt-original");
            let outside = temp.path().join("outside-registration");
            std::fs::create_dir(&outside).unwrap();
            std::fs::rename(&admin, &renamed_admin).unwrap();
            symlink(&outside, &admin).unwrap();
            assert!(verify_linked_registration(
                &prepared.git_dir,
                &prepared.prepared.common_git_dir,
                &worktree
            )
            .is_err());
            std::fs::remove_file(&admin).unwrap();
            std::fs::rename(&renamed_admin, &admin).unwrap();
            assert!(registrations.is_dir());

            use std::os::unix::process::ExitStatusExt as _;
            let output = Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: vec![1],
                stderr: vec![],
            };
            let state = Arc::new(Mutex::new(OperationState {
                result: None,
                continuation: None,
            }));
            let (notify, ready) = tokio::sync::watch::channel(false);
            let operation = WorktreeGitObservation {
                state: Arc::clone(&state),
                ready,
                _worker: std::thread::spawn(|| {}),
            };
            tokio::select! {
                biased;
                _ = operation.observe() => panic!("unpublished result cannot be observed"),
                _ = std::future::ready(()) => {}
            }
            let db = Database::open_in_memory().await.unwrap();
            let controller;
            {
                let mut state = state.lock().unwrap();
                let deadline = prepared.prepared.deadline;
                state.continuation = Some(NativeWorktreeGitContinuation {
                    owner: prepared,
                    original_effect: {
                        let command = NativeCloseCommand::new(
                            "test-transcript".into(),
                            phoenix_core::domain::db_schema::SourceToolCall {
                                message_id: "test-message".into(),
                                tool_use_id: "test-call".into(),
                            },
                            phoenix_db::native_close::NativeCloseIntent::Initial {
                                target: phoenix_core::domain::product_conversation::ProductConversationId::parse("test-target").unwrap(),
                            },
                        ).unwrap();
                        controller = SourceEntryController::new(command.clone(), deadline.into());
                        let target = phoenix_core::domain::product_conversation::ProductConversationId::parse("test-target").unwrap();
                        let receipt = NativeCloseReceipt {
                            request_id: uuid::Uuid::nil(),
                            admitted: phoenix_db::native_close::NativeCloseRunRef {
                                target,
                                attempt_id: phoenix_core::domain::close::CloseAttemptId::parse(
                                    "test-attempt",
                                )
                                .unwrap(),
                                run_ordinal: phoenix_core::domain::close::CloseRunOrdinal::parse(1)
                                    .unwrap(),
                            },
                        };
                        let resource = RetiredResourceIdentity::parse(
                            RetiredResourceKind::Worktree,
                            LossItemIdentity::Worktree(identity.clone()),
                        )
                        .unwrap();
                        OriginalWorktreeEffectContext {
                            db,
                            command,
                            receipt,
                            live_source: ConvState::Idle,
                            scope: WorkScopeId::parse("test-scope").unwrap(),
                            snapshot: CloseRetirementSnapshot::parse(
                                "test-generation",
                                "fingerprint",
                            )
                            .unwrap(),
                            resource,
                            source_entry: controller.worker(),
                            deadline,
                            fatal: FatalLocalAuthorityFence::new(),
                            leaf_release_pending_classification: false,
                        }
                    },
                });
                state.result = Some(Ok(output.clone()));
            }
            notify.send(true).unwrap();
            assert_eq!(operation.observe().await.unwrap().stdout, output.stdout);
            assert_eq!(operation.observe().await.unwrap().stdout, output.stdout);
            let continuation = operation
                .take_for_continuation()
                .expect("exact owner and original context retained");
            assert_eq!(
                continuation.owner.git_dir_incarnation,
                observe_administrative_dir_incarnation(&admin).unwrap()
            );
            assert_eq!(
                continuation.original_effect.deadline(),
                continuation.owner.prepared.deadline
            );
            assert!(operation.take_for_continuation().is_none());
            std::fs::rename(&admin, root.join(".git/worktrees/old")).unwrap();
            std::fs::create_dir(&admin).unwrap();
            std::fs::write(admin.join("commondir"), b"../..\n").unwrap();
            assert_ne!(
                continuation.owner.git_dir_incarnation,
                observe_administrative_dir_incarnation(&admin).unwrap()
            );
        }
        std::fs::write(worktree.join(".git"), b"gitdir: /other\n").unwrap();
        assert!(captured_git_directory(&worktree, &identity).is_err());
    }

    #[test]
    fn status_command_binds_captured_git_dir_without_probe() {
        let path = Path::new("/nonexistent-native-close-worktree");
        let admin = Path::new("/nonexistent-native-close-admin");
        let command = status_command(path, admin, Path::new("/captured/common"));
        assert_eq!(command.get_current_dir(), Some(path));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "GIT_DIR" && value == Some(admin.as_os_str())));
    }
}
