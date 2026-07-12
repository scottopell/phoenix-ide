//! Host git interrogation helpers.
//!
//! Thin wrappers over the `git` CLI for repository-root and default-branch
//! resolution. These are host-interaction utilities (like [`crate::platform`]),
//! not domain types — they shell out and inspect the working copy. They live in
//! `phoenix-core` because both the persistence layer and the runtime need them.

use std::path::Path;

/// Construct a Git subprocess with Phoenix's process-level safety defaults.
///
/// The environment override takes precedence over system, global, and local
/// Git configuration without modifying any of them. Every statically spawned
/// Git process in Phoenix goes through this constructor so even repository
/// setup in tests cannot invoke an interactive signing agent.
#[must_use]
pub fn command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false");
    command
}

/// Detect the git repository root for a given directory path.
///
/// Returns `None` if the path is not inside a git repository.
#[must_use]
pub fn detect_git_repo_root(path: &Path) -> Option<String> {
    let output = command()
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Resolve a repository's default branch — the project's `main_ref` and the
/// canonical fork base (REQ-PROJ-034a, Allium `GitDirectoryDetected`).
///
/// The remote's default branch (cached `refs/remotes/origin/HEAD`, no network)
/// when detectable, else the repository's checked-out branch. Returns `None`
/// when neither can be determined (e.g. `path` is not a git repository, or HEAD
/// is detached with no remote default) so the caller can decide its own
/// fallback.
#[must_use]
pub fn resolve_default_branch(path: &Path) -> Option<String> {
    if let Some(remote) = resolve_remote_default_branch(path) {
        return Some(remote);
    }

    // Else the checked-out branch. A detached HEAD reports the literal "HEAD",
    // which is not a real branch name.
    match git_capture(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(current) if current != "HEAD" && !current.is_empty() => Some(current),
        _ => None,
    }
}

/// Resolve a repository's *remote* default branch — the authoritative fork base
/// signal for `main_ref` reconciliation (REQ-PROJ-034a).
///
/// Reads the cached `refs/remotes/origin/HEAD` symbolic ref (no network) and
/// strips the `refs/remotes/origin/` prefix. Returns `None` when there is no
/// remote HEAD (e.g. a local/no-origin repository). Unlike
/// `resolve_default_branch`, there is deliberately no current-branch fallback:
/// the current checkout is not an authoritative default and must never be used
/// to overwrite an immutable stored `main_ref`.
#[must_use]
pub fn resolve_remote_default_branch(path: &Path) -> Option<String> {
    let remote = git_capture(path, &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    let branch = remote.strip_prefix("refs/remotes/origin/")?;
    if branch.is_empty() {
        return None;
    }
    Some(branch.to_string())
}

/// Run `git <args>` in `path`, returning trimmed stdout on success.
fn git_capture(path: &Path, args: &[&str]) -> Option<String> {
    let output = command().args(args).current_dir(path).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn command_disables_hostile_commit_signing_configuration() {
        let repo = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let output = command()
                .args(args)
                .current_dir(repo.path())
                .output()
                .expect("git runs");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };

        run(&["init", "--quiet"]);
        run(&["config", "user.email", "test@phoenix"]);
        run(&["config", "user.name", "Phoenix Test"]);
        run(&["config", "commit.gpgsign", "true"]);
        run(&["config", "gpg.format", "ssh"]);
        run(&[
            "config",
            "gpg.ssh.program",
            "signing-program-must-never-run",
        ]);
        run(&["commit", "--allow-empty", "--quiet", "-m", "unsigned"]);
    }
}
