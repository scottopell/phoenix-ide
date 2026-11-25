//! One independently admitted Git child under retained original native custody.
//!
//! This is synchronous worker code, not an observer or an enclosing inspection
//! callback. It never creates a receipt, source worker, lock, or deadline.

use super::{
    captured_git_directory, check_deadline, common_directory,
    observe_administrative_dir_incarnation, retain_unclassified, verify_linked_registration,
    wait_confirmed_git, Command, Instant, LossItemIdentity, NativeCloseInitialEntryRelease,
    NativeCloseInitialFirstEffectError, NativeWorktreeGit, Output, RetiredResourceKind, Stdio,
    WorkerAuthority, WorkerCustody, WorkerEntryResult,
};

/// Root validation is built in. A nested/moved target needs a separate captured
/// binding, not just a path derived anew from the command's current environment.
/// The caller's validator must compare that binding to the command AND filesystem
/// each time it is called. It must be bounded by the supplied original deadline,
/// read-only, and must not spawn, wait for children, or enter `SQLite` itself.
/// Root registration/incarnation validation is unconditional in either mode.
/// The revalidator runs both before `SQLite` admission and inside the admitted
/// spawn callback, so a preflight-only submodule check is not sufficient.
pub(super) enum GitLeafTarget<'a> {
    #[cfg(test)]
    CapturedRoot,
    RetainedRoot(&'a RetainedRootLocation),
    // Native nested-target wiring is deliberately separate from this adapter.
    #[allow(dead_code)]
    RevalidateTarget(&'a mut dyn FnMut(&Command, Instant) -> Result<(), String>),
}

/// Admits one prepared command through the original `SQLite` + source-entry gates.
/// `custody` supplies the exact captured root binding and repository lock. The
/// deadline must equal BOTH retained deadlines; a shorter/fresh deadline is not
/// a replacement operation. Only confirmed DB release permits child/pipe waits.
///
/// Returns raw exit status (including nonzero) for the inspection to interpret.
/// The caller must stop its graph on error and retain this custody independently
/// of observer cancellation; unknown authority/completion never returns at all.
#[allow(clippy::too_many_arguments)]
// The callback and its post-release outcomes form one indivisible spawn protocol.
#[allow(clippy::too_many_lines)]
pub(super) fn run_git_leaf(
    custody: &mut WorkerCustody,
    runtime: &tokio::runtime::Handle,
    authority: &mut WorkerAuthority,
    mut git: Command,
    deadline: Instant,
    operation: &str,
    mut target: GitLeafTarget<'_>,
) -> Result<Output, String> {
    // A preceding confirmed leaf may have disarmed this same worker guard.
    authority.armed = true;
    assert!(
        custody
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
            && custody.stdout_reader.is_none()
            && custody.stderr_reader.is_none(),
        "previous Git child and pipes must be completed before another admission"
    );
    let owner = custody
        .prepared
        .as_ref()
        .expect("worker owns prepared lock");
    let original_effect = custody
        .original_effect
        .as_mut()
        .expect("worker owns original effect context");
    let resource = &original_effect.resource;
    let prepared = &owner.prepared;
    let validation = (|| {
        validate_original_deadline(deadline, original_effect.deadline(), prepared.deadline)?;
        if resource.kind() != RetiredResourceKind::Worktree
            || !matches!(resource.identity(), LossItemIdentity::Worktree(identity) if identity == &prepared.identity)
        {
            return Err("original Worktree Git resource does not match captured binding".into());
        }
        target.validate_binding(owner)?;
        target.validate(&git, owner, deadline)
    })();
    if let Err(error) = validation {
        authority.armed = false;
        return Err(error);
    }
    // Caller stdio cannot bypass bounded pipe custody or inherit an open stdin.
    git.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        git.process_group(0);
    }
    let child_slot = &custody.child;
    // The worker retains the original lock while the database waits for its
    // writer. Command construction cannot start fsmonitor or spawn Git.
    let entry = original_effect.admitted_leaf(runtime, || {
        check_deadline(deadline)?;
        target.validate_binding(owner)?;
        target.validate(&git, owner, deadline)?;
        check_deadline(deadline)?;
        let mut child_guard = child_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        git.spawn()
            .map(|child| {
                *child_guard = Some(child);
            })
            .map_err(|error| error.to_string())
    });
    let release_confirmed = matches!(
        &entry,
        Ok((_, NativeCloseInitialEntryRelease::Confirmed))
            | Err(NativeCloseInitialFirstEffectError::NotStarted(_)
                | NativeCloseInitialFirstEffectError::Rejected {
                    release: NativeCloseInitialEntryRelease::Confirmed,
                    ..
                })
    );
    if release_confirmed {
        original_effect.classify_confirmed_release();
    }
    match entry {
        Ok((WorkerEntryResult::Ran(Ok(())), NativeCloseInitialEntryRelease::Confirmed)) => {
            let result = wait_confirmed_git(custody, deadline);
            authority.armed = false;
            result
        }
        Ok((WorkerEntryResult::Ran(Ok(())), NativeCloseInitialEntryRelease::Unconfirmed(_))) => {
            // The writer's release is unknown. Never kill the child or unlock
            // the repository under unclassified authority, even at deadline.
            original_effect
                .fatal
                .close("native_close_initial_worktree_unknown_writer_release");
            retain_unclassified(&std::mem::replace(custody, WorkerCustody::empty()));
        }
        Ok((_, NativeCloseInitialEntryRelease::Unconfirmed(_)))
        | Err(NativeCloseInitialFirstEffectError::Rejected {
            release: NativeCloseInitialEntryRelease::Unconfirmed(_),
            ..
        }) => {
            original_effect
                .fatal
                .close("native_close_initial_worktree_unknown_writer_release");
            retain_unclassified(&std::mem::replace(custody, WorkerCustody::empty()))
        }
        Err(
            NativeCloseInitialFirstEffectError::NotStarted(error)
            | NativeCloseInitialFirstEffectError::Rejected {
                error,
                release: NativeCloseInitialEntryRelease::Confirmed,
            },
        ) => {
            authority.armed = false;
            Err(format!(
                "native Worktree Git authorization refused for {operation}: {error}"
            ))
        }
        Ok((WorkerEntryResult::Busy, NativeCloseInitialEntryRelease::Confirmed)) => {
            // Busy/Refused are returned before the admitted callback runs, so no child exists.
            authority.armed = false;
            Err("original Close source unavailable at Worktree Git spawn: source-entry busy".into())
        }
        Ok((WorkerEntryResult::Refused, NativeCloseInitialEntryRelease::Confirmed)) => {
            // Refusal likewise precedes spawn; preserve the source-unavailable diagnosis.
            authority.armed = false;
            Err(
                "original Close source unavailable at Worktree Git spawn: source-entry refused"
                    .into(),
            )
        }
        Ok((WorkerEntryResult::Ran(Err(error)), NativeCloseInitialEntryRelease::Confirmed)) => {
            authority.armed = false;
            Err(error)
        }
    }
}

fn validate_original_deadline(
    supplied: Instant,
    original: Instant,
    prepared: Instant,
) -> Result<(), String> {
    if supplied != original || supplied != prepared {
        return Err("Git leaf must use the unchanged original Worktree deadline".into());
    }
    check_deadline(supplied)
}

fn validate_root(owner: &NativeWorktreeGit) -> Result<(), String> {
    let prepared = &owner.prepared;
    let path = &prepared.inspection_path;
    if captured_git_directory(path, &prepared.identity).as_deref() != Ok(owner.git_dir.as_path())
        || observe_administrative_dir_incarnation(&owner.git_dir).as_deref()
            != Ok(owner.git_dir_incarnation.as_str())
        || common_directory(&owner.git_dir).as_deref() != Ok(prepared.common_git_dir.as_path())
        || verify_linked_registration(&owner.git_dir, &prepared.common_git_dir, path).is_err()
        || observe_administrative_dir_incarnation(&prepared.common_git_dir).as_deref()
            != Ok(prepared.common_git_dir_incarnation.as_str())
    {
        return Err("captured Worktree Git binding changed before authorized spawn".into());
    }
    Ok(())
}

impl GitLeafTarget<'_> {
    fn validate_binding(&self, owner: &NativeWorktreeGit) -> Result<(), String> {
        match self {
            Self::RetainedRoot(location) => location.validate(owner),
            #[cfg(test)]
            Self::CapturedRoot => validate_root(owner),
            Self::RevalidateTarget(_) => validate_root(owner),
        }
    }

    fn validate(
        &mut self,
        command: &Command,
        owner: &NativeWorktreeGit,
        deadline: Instant,
    ) -> Result<(), String> {
        if command.get_program() != std::ffi::OsStr::new("git") {
            return Err("native Worktree Git leaf requires a prepared Git command".into());
        }
        match self {
            #[cfg(test)]
            Self::CapturedRoot => validate_root_command(
                command,
                &owner.prepared.inspection_path,
                &owner.git_dir,
                &owner.prepared.common_git_dir,
            ),
            Self::RetainedRoot(location) => validate_root_command(
                command,
                location.path(),
                &owner.git_dir,
                &owner.prepared.common_git_dir,
            ),
            Self::RevalidateTarget(validate) => validate(command, deadline),
        }
    }
}

/// Immutable registration witness captured while the checkout is still bound.
/// After relocation, relative registration records must NOT be resolved against
/// the tombstone: their meaning remains anchored to the original registration.
#[derive(Debug, Clone)]
pub(super) struct CapturedRegistration {
    original_path: std::path::PathBuf,
    incarnation: String,
    common_path: std::path::PathBuf,
    common_incarnation: String,
    backlink: Vec<u8>,
    commondir: Vec<u8>,
}

impl CapturedRegistration {
    fn capture(owner: &NativeWorktreeGit) -> Result<Self, String> {
        let (_, backlink) =
            phoenix_core::git::read_worktree_registration_file(&owner.git_dir.join("gitdir"))
                .map_err(|error| error.to_string())?;
        let (_, commondir) =
            phoenix_core::git::read_worktree_registration_file(&owner.git_dir.join("commondir"))
                .map_err(|error| error.to_string())?;
        let witness = Self {
            original_path: owner.git_dir.clone(),
            incarnation: owner.git_dir_incarnation.clone(),
            common_path: owner.prepared.common_git_dir.clone(),
            common_incarnation: owner.prepared.common_git_dir_incarnation.clone(),
            backlink,
            commondir,
        };
        // Establish the interpretation at the ORIGINAL location, once only.
        if common_directory(&owner.git_dir)? != witness.common_path {
            return Err("captured Worktree common directory changed during binding".into());
        }
        witness.validate_at(&owner.git_dir)?;
        Ok(witness)
    }

    fn validate_records(
        &self,
        incarnation: &str,
        common_incarnation: &str,
        backlink: &[u8],
        commondir: &[u8],
    ) -> Result<(), String> {
        if incarnation != self.incarnation
            || common_incarnation != self.common_incarnation
            || backlink != self.backlink.as_slice()
            || commondir != self.commondir.as_slice()
        {
            return Err("captured Worktree registration witness changed; target preserved".into());
        }
        Ok(())
    }

    pub(super) fn validate_at(&self, path: &std::path::Path) -> Result<(), String> {
        let incarnation = observe_administrative_dir_incarnation(path)?;
        let common_incarnation = observe_administrative_dir_incarnation(&self.common_path)?;
        let (_, backlink) =
            phoenix_core::git::read_worktree_registration_file(&path.join("gitdir"))
                .map_err(|error| error.to_string())?;
        let (_, commondir) =
            phoenix_core::git::read_worktree_registration_file(&path.join("commondir"))
                .map_err(|error| error.to_string())?;
        self.validate_records(&incarnation, &common_incarnation, &backlink, &commondir)
    }

    pub(super) fn validate_rename_source(&self, path: &std::path::Path) -> Result<(), String> {
        if path != self.original_path {
            return Err("administrative rename source differs from captured registration".into());
        }
        self.validate_at(path)
    }
}

/// Read-only, immutable checkout proof carried into the admitted rename leaf.
/// It cannot advance relocation state or mint a replacement operation.
#[derive(Clone)]
pub(super) struct CheckoutRenameWitness {
    path: std::path::PathBuf,
    identity: (u64, u64),
    pointer: Vec<u8>,
    registration: CapturedRegistration,
}

impl CheckoutRenameWitness {
    pub(super) fn validate_source(&self, source: &std::path::Path) -> Result<(), String> {
        if source != self.path || directory_identity(source)? != self.identity {
            return Err(
                "checkout rename source differs from captured directory incarnation".into(),
            );
        }
        self.registration
            .validate_rename_source(&self.registration.original_path)?;
        let (_, pointer) = phoenix_core::git::read_worktree_registration_file(&source.join(".git"))
            .map_err(|error| error.to_string())?;
        if pointer != self.pointer {
            return Err("checkout rename source registration pointer changed".into());
        }
        Ok(())
    }
}

/// Affine relocation evidence owned by the physical worker, not a new Worktree
/// identity. The captured registration and its original backlink never change.
/// Only a confirmed owned rename with equal source/object dev+inode advances it.
#[derive(Debug)]
pub(super) struct RetainedRootLocation {
    original_path: std::path::PathBuf,
    current_path: std::path::PathBuf,
    current_identity: (u64, u64),
    checkout_pointer: Vec<u8>,
    registration: CapturedRegistration,
    moved: bool,
}

impl RetainedRootLocation {
    pub(super) fn capture(owner: &NativeWorktreeGit) -> Result<Self, String> {
        validate_root(owner)?;
        let path = &owner.prepared.inspection_path;
        if *path
            != super::super::super::path_buf_from_git_bytes(
                owner.prepared.identity.locator().as_bytes(),
            )
        {
            return Err("native physical retirement does not resume relocated worktrees".into());
        }
        let pointer = super::super::super::path_buf_from_git_bytes(&super::captured_pointer(
            &owner.prepared.identity,
        )?);
        if !pointer.is_absolute() {
            return Err(
                "native physical retirement requires an absolute Git registration pointer".into(),
            );
        }
        let registration = CapturedRegistration::capture(owner)?;
        let location = Self {
            original_path: path.clone(),
            current_path: path.clone(),
            current_identity: directory_identity(path)?,
            checkout_pointer: phoenix_core::git::read_worktree_registration_file(
                &path.join(".git"),
            )
            .map_err(|error| error.to_string())?
            .1,
            registration,
            moved: false,
        };
        location.validate(owner)?;
        Ok(location)
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.current_path
    }

    pub(super) fn original_path(&self) -> &std::path::Path {
        &self.original_path
    }

    pub(super) fn checkout_rename_witness(&self) -> Result<CheckoutRenameWitness, String> {
        if self.moved || self.current_path != self.original_path {
            return Err("checkout final rename requires the original retained source".into());
        }
        Ok(CheckoutRenameWitness {
            path: self.current_path.clone(),
            identity: self.current_identity,
            pointer: self.checkout_pointer.clone(),
            registration: self.registration.clone(),
        })
    }

    pub(super) fn registration(&self) -> &CapturedRegistration {
        &self.registration
    }

    pub(super) fn validate(&self, owner: &NativeWorktreeGit) -> Result<(), String> {
        check_deadline(owner.prepared.deadline)?;
        let prepared = &owner.prepared;
        if self.original_path != prepared.inspection_path
            || directory_identity(&self.current_path)? != self.current_identity
            || captured_git_directory(&self.current_path, &prepared.identity)? != owner.git_dir
            || observe_administrative_dir_incarnation(&owner.git_dir)? != owner.git_dir_incarnation
            || common_directory(&owner.git_dir)? != prepared.common_git_dir
            || observe_administrative_dir_incarnation(&prepared.common_git_dir)?
                != prepared.common_git_dir_incarnation
        {
            return Err("retained Worktree relocation binding changed".into());
        }
        let registrations = prepared.common_git_dir.join("worktrees");
        let registrations_metadata =
            std::fs::symlink_metadata(&registrations).map_err(|error| error.to_string())?;
        if !registrations_metadata.is_dir()
            || registrations_metadata.file_type().is_symlink()
            || registrations
                .canonicalize()
                .map_err(|error| error.to_string())?
                != registrations
            || owner.git_dir.parent() != Some(registrations.as_path())
            || owner
                .git_dir
                .canonicalize()
                .map_err(|error| error.to_string())?
                != owner.git_dir
        {
            return Err("retained Worktree registration escaped captured repository".into());
        }
        self.registration.validate_at(&owner.git_dir)?;
        if self.moved {
            require_absent(&self.original_path)?;
        } else {
            verify_linked_registration(
                &owner.git_dir,
                &prepared.common_git_dir,
                &self.original_path,
            )?;
        }
        check_deadline(prepared.deadline)
    }

    fn check_move_identity(&self, observed_identity: (u64, u64)) -> Result<(), String> {
        if self.moved || observed_identity != self.current_identity {
            return Err("final Worktree object is not the owned rename source".into());
        }
        Ok(())
    }

    pub(super) fn confirm_owned_move(
        &mut self,
        owner: &NativeWorktreeGit,
        destination: &std::path::Path,
        observed_identity: (u64, u64),
    ) -> Result<(), String> {
        self.check_move_identity(observed_identity)?;
        if directory_identity(destination)? != self.current_identity {
            return Err("final Worktree object is not the owned rename source".into());
        }
        require_absent(&self.original_path)?;
        self.current_path = destination.to_path_buf();
        self.current_identity = observed_identity;
        self.moved = true;
        self.validate(owner)
    }
}

pub(super) fn require_absent(path: &std::path::Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot prove path absent at {}: {error}",
            path.display()
        )),
        Ok(_) => Err(format!(
            "post-inspection entry survives at {}; retained object preserved",
            path.display()
        )),
    }
}

pub(super) fn directory_identity(path: &std::path::Path) -> Result<(u64, u64), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("retained Worktree location is not a real directory".into());
        }
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("native physical Worktree retirement requires Unix directory identities".into())
    }
}

/// Shared inspectors prepare commands without starting them. Check their route
/// before adding the original sink's environment hygiene and common-dir binding.
pub(super) fn bind_shared_root_command(
    command: &mut Command,
    owner: &NativeWorktreeGit,
    location: &RetainedRootLocation,
) -> Result<(), String> {
    if command.get_current_dir() != Some(location.path())
        || command
            .get_envs()
            .find(|(key, _)| *key == "GIT_DIR")
            .and_then(|(_, value)| value)
            != Some(owner.git_dir.as_os_str())
    {
        return Err("shared inspection attempted to redirect retained root".into());
    }
    for name in [
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
        "GIT_CONFIG",
    ] {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_CONFIG_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_COMMON_DIR", &owner.prepared.common_git_dir)
        .env("GIT_WORK_TREE", location.path())
        .env("GIT_OPTIONAL_LOCKS", "0");
    Ok(())
}

/// This is routing validation, not a sandbox for untrusted Git arguments. The
/// prepared command still owns noninteractive/fsmonitor policy, scratch-index
/// intent, environment hygiene, and operation-specific safety.
fn validate_root_command(
    command: &Command,
    path: &std::path::Path,
    git_dir: &std::path::Path,
    common_git_dir: &std::path::Path,
) -> Result<(), String> {
    if command.get_current_dir() != Some(path) {
        return Err("Git leaf cwd does not name the captured Worktree root".into());
    }
    for (name, expected) in [
        ("GIT_DIR", git_dir),
        ("GIT_COMMON_DIR", common_git_dir),
        ("GIT_WORK_TREE", path),
    ] {
        let actual = command
            .get_envs()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value);
        if actual != Some(expected.as_os_str()) {
            return Err(format!(
                "Git leaf {name} does not name the captured Worktree binding"
            ));
        }
    }
    // Global routing flags take precedence over the otherwise exact env/cwd.
    // Accept only the global options emitted by our prepared inspection helpers.
    let mut args = command.get_args();
    while let Some(arg) = args.next() {
        if arg == "-c" {
            let Some(config) = args.next().and_then(std::ffi::OsStr::to_str) else {
                return Err("Git leaf has an invalid configuration argument".into());
            };
            let Some((key, _)) = config.split_once('=') else {
                return Err("Git leaf has an invalid configuration argument".into());
            };
            if key.eq_ignore_ascii_case("core.worktree")
                || key.eq_ignore_ascii_case("core.bare")
                || key.to_ascii_lowercase().starts_with("alias.")
            {
                return Err("Git leaf configuration can redirect the captured target".into());
            }
        } else if arg.as_encoded_bytes().starts_with(b"-") {
            if !["--literal-pathspecs", "--no-pager", "--no-optional-locks"]
                .iter()
                .any(|allowed| arg == *allowed)
            {
                return Err("Git leaf global option requires explicit target validation".into());
            }
        } else {
            return Ok(());
        }
    }
    Err("Git leaf has no operation".into())
}

#[cfg(test)]
pub(super) fn require_success(output: Output, operation: &str) -> Result<Output, String> {
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("{operation} exited with {}", output.status)
        } else {
            format!("{operation} exited with {}: {detail}", output.status)
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, time::Duration};

    const ROOT: &str = "/captured/worktree";
    const GIT: &str = "/captured/common/worktrees/wt";
    const COMMON: &str = "/captured/common";

    fn root_command() -> Command {
        super::super::status_command(Path::new(ROOT), Path::new(GIT), Path::new(COMMON))
    }

    fn validate(command: &Command) -> Result<(), String> {
        validate_root_command(command, Path::new(ROOT), Path::new(GIT), Path::new(COMMON))
    }

    fn registration_witness() -> CapturedRegistration {
        CapturedRegistration {
            original_path: GIT.into(),
            incarnation: "git_admin_dir_v1:17:29".into(),
            common_path: COMMON.into(),
            common_incarnation: "git_admin_dir_v1:17:31".into(),
            backlink: b"/captured/root/.git\n".to_vec(),
            commondir: b"../..\n".to_vec(),
        }
    }

    #[test]
    fn registration_witness_requires_exact_records_and_both_original_incarnations() {
        let witness = registration_witness();
        let validate = |admin: &str, common: &str, backlink: &[u8], commondir: &[u8]| {
            witness.validate_records(admin, common, backlink, commondir)
        };
        assert!(validate(
            &witness.incarnation,
            &witness.common_incarnation,
            &witness.backlink,
            &witness.commondir
        )
        .is_ok());
        // Even equivalent spellings are not the captured immutable records.
        for (backlink, commondir) in [
            (b"/other/.git\n".as_slice(), witness.commondir.as_slice()),
            (
                b"/captured/root/.git\r\n".as_slice(),
                witness.commondir.as_slice(),
            ),
            (witness.backlink.as_slice(), b"../../\n".as_slice()),
            (
                witness.backlink.as_slice(),
                b"/captured/common\n".as_slice(),
            ),
        ] {
            assert!(validate(
                &witness.incarnation,
                &witness.common_incarnation,
                backlink,
                commondir
            )
            .is_err());
        }
        assert!(validate(
            "git_admin_dir_v1:17:30",
            &witness.common_incarnation,
            &witness.backlink,
            &witness.commondir
        )
        .is_err());
        assert!(validate(
            &witness.incarnation,
            "git_admin_dir_v1:17:32",
            &witness.backlink,
            &witness.commondir
        )
        .is_err());
        assert!(witness
            .validate_rename_source(Path::new("/moved/object"))
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn registration_checks_original_and_moved_bytes_without_rebasing_relative_commondir() {
        let temp = tempfile::tempdir().unwrap();
        let common = temp.path().join("common");
        let original = common.join("worktrees/wt");
        std::fs::create_dir_all(&original).unwrap();
        let mut witness = registration_witness();
        witness.original_path = original.clone();
        witness.common_path = common.clone();
        witness.incarnation = observe_administrative_dir_incarnation(&original).unwrap();
        witness.common_incarnation = observe_administrative_dir_incarnation(&common).unwrap();
        std::fs::write(original.join("gitdir"), &witness.backlink).unwrap();
        std::fs::write(original.join("commondir"), &witness.commondir).unwrap();
        assert!(witness.validate_rename_source(&original).is_ok());

        // An in-place edit leaves the directory dev/inode unchanged, but must
        // still stop both pre-rename qualification and moved-object traversal.
        for record in ["gitdir", "commondir"] {
            let bytes = std::fs::read(original.join(record)).unwrap();
            std::fs::write(original.join(record), b"changed\n").unwrap();
            assert!(witness.validate_rename_source(&original).is_err());
            std::fs::write(original.join(record), bytes).unwrap();
        }
        let root = common.join("worktrees/.phoenix-admin-delete-test");
        std::fs::create_dir(&root).unwrap();
        let object = root.join("object");
        std::fs::rename(&original, &object).unwrap();
        // Resolving object/../.. would yield worktrees/, NOT captured common/.
        assert_ne!(common_directory(&object).unwrap(), common);
        assert!(witness.validate_at(&object).is_ok());
        for record in ["gitdir", "commondir"] {
            let bytes = std::fs::read(object.join(record)).unwrap();
            std::fs::write(object.join(record), b"changed\n").unwrap();
            assert!(witness.validate_at(&object).is_err());
            assert!(object.is_dir());
            std::fs::write(object.join(record), bytes).unwrap();
        }
        // Equal moved records are insufficient if the captured common-dir
        // pathname now names a different directory incarnation.
        let displaced = temp.path().join("displaced-common");
        std::fs::rename(&common, &displaced).unwrap();
        std::fs::create_dir(&common).unwrap();
        let displaced_object = displaced.join("worktrees/.phoenix-admin-delete-test/object");
        assert!(witness.validate_at(&displaced_object).is_err());
        assert!(displaced_object.is_dir());
    }

    #[cfg(unix)]
    struct RenameBoundaryExecutor(Option<Box<dyn FnOnce()>>);

    #[cfg(unix)]
    impl crate::runtime::close_retirement::WorktreeMutationExecutor for RenameBoundaryExecutor {
        fn execute<T, F>(&mut self, effect: F) -> Result<T, String>
        where
            T: Send + 'static,
            F: FnOnce() -> Result<T, String> + Send + 'static,
        {
            self.0.take().expect("one admitted rename")();
            effect()
        }
    }

    #[cfg(unix)]
    #[test]
    fn checkout_rename_rechecks_immutable_source_and_registration_after_admission() {
        use crate::runtime::close_retirement::{
            deletion_slot_stat, open_deletion_parent, DeletionEntryIdentity, OwnedDeletionSlot,
            OwnedWorktreeRenameLeaf, WorktreeMutationExecutor as _,
        };
        use std::os::unix::fs::PermissionsExt as _;
        // Plain bounded filesystem records, not a native/SQLite/Git fixture.
        for change in [
            "none",
            "checkout",
            "pointer",
            "backlink",
            "commondir",
            "common",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("checkout");
            let root = temp.path().join("root");
            let common = temp.path().join("common");
            let admin = common.join("worktrees/wt");
            std::fs::create_dir_all(&admin).unwrap();
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let pointer = format!("gitdir: {}\n", admin.display()).into_bytes();
            let backlink = format!("{}\n", source.join(".git").display()).into_bytes();
            std::fs::write(source.join(".git"), &pointer).unwrap();
            std::fs::write(admin.join("gitdir"), &backlink).unwrap();
            std::fs::write(admin.join("commondir"), b"../..\n").unwrap();
            let source_identity = directory_identity(&source).unwrap();
            let checkout = CheckoutRenameWitness {
                path: source.clone(),
                identity: source_identity,
                pointer: pointer.clone(),
                registration: CapturedRegistration {
                    original_path: admin.clone(),
                    incarnation: observe_administrative_dir_incarnation(&admin).unwrap(),
                    common_path: common.clone(),
                    common_incarnation: observe_administrative_dir_incarnation(&common).unwrap(),
                    backlink,
                    commondir: b"../..\n".to_vec(),
                },
            };
            assert!(checkout.validate_source(&source).is_ok());
            let parent = open_deletion_parent(temp.path()).unwrap();
            let capture = |name: &str| {
                let name = std::ffi::CString::new(name).unwrap();
                let identity =
                    DeletionEntryIdentity::from_stat(&deletion_slot_stat(&parent, &name).unwrap());
                OwnedDeletionSlot::capture(&parent, &name, identity).unwrap()
            };
            let leaf = OwnedWorktreeRenameLeaf {
                source_path: source.clone(),
                source: capture("checkout"),
                root: capture("root"),
            };
            let effect_source = source.clone();
            let effect_common = common.clone();
            let mut executor = RenameBoundaryExecutor(Some(Box::new(move || match change {
                "checkout" => {
                    std::fs::rename(&effect_source, effect_source.with_file_name("old-checkout"))
                        .unwrap();
                    std::fs::create_dir(&effect_source).unwrap();
                    std::fs::write(effect_source.join(".git"), pointer).unwrap();
                }
                "pointer" => {
                    std::fs::write(effect_source.join(".git"), b"gitdir: /replacement\n").unwrap();
                }
                "backlink" => std::fs::write(admin.join("gitdir"), b"/replacement/.git\n").unwrap(),
                "commondir" => std::fs::write(admin.join("commondir"), b"../../\n").unwrap(),
                "common" => {
                    std::fs::rename(&effect_common, effect_common.with_file_name("old-common"))
                        .unwrap();
                    std::fs::create_dir(&effect_common).unwrap();
                }
                _ => {}
            })));
            // Same proof then owned-leaf sequence as PhysicalDeletionDriver's
            // checkout branch, inside the executor, not at preparation time.
            let result = executor.execute(move || {
                checkout.validate_source(&leaf.source_path)?;
                leaf.rename(None)
            });
            if change == "none" {
                result.unwrap().unwrap();
                assert!(!source.exists());
                assert_eq!(
                    directory_identity(&root.join("object")).unwrap(),
                    source_identity
                );
            } else {
                assert!(result.is_err(), "{change}");
                assert!(source.is_dir(), "mismatched source must not move: {change}");
                assert!(!root.join("object").exists());
            }
        }
    }

    #[test]
    fn retained_relocation_requires_the_single_original_directory_incarnation() {
        let mut location = RetainedRootLocation {
            original_path: ROOT.into(),
            current_path: ROOT.into(),
            current_identity: (17, 29),
            checkout_pointer: b"gitdir: /captured/common/worktrees/wt\n".to_vec(),
            registration: registration_witness(),
            moved: false,
        };
        assert!(location.check_move_identity((17, 29)).is_ok());
        assert!(location.check_move_identity((18, 29)).is_err());
        assert!(location.check_move_identity((17, 30)).is_err());
        location.moved = true;
        assert!(location.check_move_identity((17, 29)).is_err());
        assert_eq!(location.original_path(), Path::new(ROOT));
        assert_eq!(
            location.registration.backlink.as_slice(),
            b"/captured/root/.git\n"
        );
    }

    #[test]
    fn relocated_root_routing_changes_only_worktree_location_not_administration() {
        let relocated = Path::new("/captured/.phoenix-delete-random/object");
        let mut command = root_command();
        command
            .current_dir(relocated)
            .env("GIT_WORK_TREE", relocated);
        assert!(
            validate_root_command(&command, relocated, Path::new(GIT), Path::new(COMMON)).is_ok()
        );
        assert!(validate(&command).is_err());
        for name in ["GIT_DIR", "GIT_COMMON_DIR"] {
            let mut command = root_command();
            command
                .current_dir(relocated)
                .env("GIT_WORK_TREE", relocated)
                .env(name, "/replacement");
            assert!(
                validate_root_command(&command, relocated, Path::new(GIT), Path::new(COMMON))
                    .is_err()
            );
        }
    }

    #[test]
    fn leaf_deadline_is_exact_not_renewed_or_shortened() {
        let original = Instant::now() + Duration::from_secs(60);
        assert!(validate_original_deadline(original, original, original).is_ok());
        for supplied in [
            original
                .checked_sub(Duration::from_secs(1))
                .expect("a monotonic deadline is older than one second"),
            original + Duration::from_secs(1),
        ] {
            assert!(validate_original_deadline(supplied, original, original).is_err());
        }
        assert!(
            validate_original_deadline(original, original, original + Duration::from_secs(1))
                .is_err()
        );
        let expired = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("a monotonic clock is older than one second");
        assert!(validate_original_deadline(expired, expired, expired).is_err());
    }

    #[test]
    fn existing_status_command_explicitly_targets_captured_root() {
        assert!(validate(&root_command()).is_ok());
    }

    #[test]
    fn root_mode_refuses_nested_and_replaced_command_targets() {
        let mut nested = root_command();
        nested.current_dir(Path::new(ROOT).join("submodule"));
        assert!(validate(&nested).is_err());
        for name in ["GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE"] {
            let mut replaced = root_command();
            replaced.env(name, "/replacement");
            assert!(validate(&replaced).is_err(), "accepted replacement {name}");
            let mut unbound = root_command();
            unbound.env_remove(name);
            assert!(validate(&unbound).is_err(), "accepted inherited {name}");
        }
    }

    #[test]
    fn root_mode_refuses_global_target_overrides_and_alias_configuration() {
        for arguments in [
            vec!["-C", "/replacement", "status"],
            vec!["--git-dir=/replacement", "status"],
            vec!["--work-tree=/replacement", "status"],
            vec!["--config-env=core.worktree=TARGET", "status"],
            vec!["-c", "core.worktree=/replacement", "status"],
            vec!["-c", "alias.probe=!git -C /replacement status", "probe"],
            vec![],
        ] {
            let mut command = phoenix_core::git::command();
            command
                .current_dir(ROOT)
                .env("GIT_DIR", GIT)
                .env("GIT_COMMON_DIR", COMMON)
                .env("GIT_WORK_TREE", ROOT)
                .args(arguments);
            assert!(validate(&command).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn status_wrapper_interprets_raw_exit_without_losing_diagnostics() {
        use std::os::unix::process::ExitStatusExt as _;

        let output = Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: b"partial output".to_vec(),
            stderr: b"  refused\n".to_vec(),
        };
        let expected = format!("Git status exited with {}: refused", output.status);
        assert_eq!(require_success(output, "Git status").unwrap_err(), expected);
        let output = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: vec![0, 255],
            stderr: b"diagnostic".to_vec(),
        };
        let actual = require_success(output.clone(), "Git status").unwrap();
        assert_eq!(actual.stdout, output.stdout);
        assert_eq!(actual.stderr, output.stderr);
        assert_eq!(actual.status, output.status);
    }
}
