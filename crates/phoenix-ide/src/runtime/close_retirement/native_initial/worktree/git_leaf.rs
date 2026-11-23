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
/// read-only, and must not spawn, wait for children, or enter SQLite itself.
/// Root registration/incarnation validation is unconditional in either mode.
/// The revalidator runs both before SQLite admission and inside the admitted
/// spawn callback, so a preflight-only submodule check is not sufficient.
pub(super) enum GitLeafTarget<'a> {
    CapturedRoot,
    // Native nested-target wiring is deliberately separate from this adapter.
    #[allow(dead_code)]
    RevalidateTarget(&'a mut dyn FnMut(&Command, Instant) -> Result<(), String>),
}

/// Admits one prepared command through the original SQLite + source-entry gates.
/// `custody` supplies the exact captured root binding and repository lock. The
/// deadline must equal BOTH retained deadlines; a shorter/fresh deadline is not
/// a replacement operation. Only confirmed DB release permits child/pipe waits.
///
/// Returns raw exit status (including nonzero) for the inspection to interpret.
/// The caller must stop its graph on error and retain this custody independently
/// of observer cancellation; unknown authority/completion never returns at all.
#[allow(clippy::too_many_arguments)]
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
        validate_root(owner)?;
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
        validate_root(owner)?;
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
    let release_confirmed = match &entry {
        Ok((_, NativeCloseInitialEntryRelease::Confirmed))
        | Err(NativeCloseInitialFirstEffectError::NotStarted(_))
        | Err(NativeCloseInitialFirstEffectError::Rejected {
            release: NativeCloseInitialEntryRelease::Confirmed,
            ..
        }) => true,
        _ => false,
    };
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
            retain_unclassified(std::mem::replace(custody, WorkerCustody::empty()));
        }
        Ok((_, NativeCloseInitialEntryRelease::Unconfirmed(_)))
        | Err(NativeCloseInitialFirstEffectError::Rejected {
            release: NativeCloseInitialEntryRelease::Unconfirmed(_),
            ..
        }) => {
            original_effect
                .fatal
                .close("native_close_initial_worktree_unknown_writer_release");
            retain_unclassified(std::mem::replace(custody, WorkerCustody::empty()))
        }
        Err(NativeCloseInitialFirstEffectError::NotStarted(error))
        | Err(NativeCloseInitialFirstEffectError::Rejected {
            error,
            release: NativeCloseInitialEntryRelease::Confirmed,
        }) => {
            authority.armed = false;
            Err(format!(
                "native Worktree Git authorization refused for {operation}: {error}"
            ))
        }
        Ok((
            WorkerEntryResult::Busy | WorkerEntryResult::Refused,
            NativeCloseInitialEntryRelease::Confirmed,
        )) => {
            authority.armed = false;
            Err("original Close source unavailable at Worktree Git spawn".into())
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
            Self::CapturedRoot => validate_root_command(
                command,
                &owner.prepared.inspection_path,
                &owner.git_dir,
                &owner.prepared.common_git_dir,
            ),
            Self::RevalidateTarget(validate) => validate(command, deadline),
        }
    }
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
        } else if ["--literal-pathspecs", "--no-pager", "--no-optional-locks"]
            .iter()
            .any(|allowed| arg == *allowed)
        {
            continue;
        } else if arg.as_encoded_bytes().starts_with(b"-") {
            return Err("Git leaf global option requires explicit target validation".into());
        } else {
            return Ok(());
        }
    }
    Err("Git leaf has no operation".into())
}

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

    #[test]
    fn leaf_deadline_is_exact_not_renewed_or_shortened() {
        let original = Instant::now() + Duration::from_secs(60);
        assert!(validate_original_deadline(original, original, original).is_ok());
        for supplied in [
            original - Duration::from_secs(1),
            original + Duration::from_secs(1),
        ] {
            assert!(validate_original_deadline(supplied, original, original).is_err());
        }
        assert!(
            validate_original_deadline(original, original, original + Duration::from_secs(1)).is_err()
        );
        let expired = Instant::now() - Duration::from_secs(1);
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
            let mut command = Command::new("git");
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
