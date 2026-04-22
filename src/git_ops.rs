//! Consolidated git operations for worktree management.
//!
//! Previously scattered across `api::handlers` and `runtime::executor`,
//! these functions implement the fetch + materialize + worktree-create
//! pipeline used by Branch mode, Managed mode, and task approval.

use std::path::Path;

/// Unified error type for git operations.
#[derive(Debug)]
pub(crate) enum GitOpError {
    /// Git command failed.
    Git(String),
    /// Branch not found locally or at origin.
    BranchNotFound(String),
    /// Filesystem error.
    Io(String),
}

impl std::fmt::Display for GitOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Git(msg) => write!(f, "git error: {msg}"),
            Self::BranchNotFound(branch) => {
                write!(f, "branch '{branch}' not found locally or at origin")
            }
            Self::Io(msg) => write!(f, "IO error: {msg}"),
        }
    }
}

impl From<GitOpError> for String {
    fn from(e: GitOpError) -> Self {
        e.to_string()
    }
}

/// Run a git command in the given directory, returning stdout on success or an error message.
///
/// All git operations use a dedicated bot identity and disable commit signing
/// to avoid depending on the user's SSH agent (which breaks in workspaces/tmux).
pub(crate) fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .map_err(|e| format!("Failed to run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}

/// Fetch a single branch from origin and ensure a local ref exists.
///
/// Best-effort: network failure is non-fatal (uses local ref).
/// Creates a local tracking branch if only a remote ref exists.
/// Fast-forwards the local ref if behind the remote.
pub(crate) fn materialize_branch(cwd: &Path, branch_name: &str) -> Result<(), GitOpError> {
    // 1. Single-branch fetch (best-effort, REQ-PROJ-022)
    let refspec = format!("refs/heads/{branch_name}:refs/remotes/origin/{branch_name}");
    if let Err(e) = run_git(cwd, &["fetch", "origin", &refspec]) {
        tracing::debug!(
            branch = %branch_name,
            error = %e,
            "Single-branch fetch failed (non-fatal, using local ref)"
        );
    }

    // 2. Check local / remote ref existence
    let has_local = run_git(cwd, &["rev-parse", "--verify", branch_name]).is_ok();
    let remote_ref = format!("origin/{branch_name}");
    let has_remote = run_git(cwd, &["rev-parse", "--verify", &remote_ref]).is_ok();

    if has_local && has_remote {
        // 3. Fast-forward local to remote tip if possible.
        let local_sha = run_git(cwd, &["rev-parse", branch_name]).unwrap_or_default();
        let remote_sha = run_git(cwd, &["rev-parse", &remote_ref]).unwrap_or_default();
        if local_sha.trim() != remote_sha.trim()
            && run_git(
                cwd,
                &["merge-base", "--is-ancestor", branch_name, &remote_ref],
            )
            .is_ok()
        {
            let current_head =
                run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
            if current_head.trim() == branch_name {
                tracing::debug!(
                    branch = %branch_name,
                    "Cannot fast-forward: branch is currently checked out"
                );
            } else {
                let _ = run_git(
                    cwd,
                    &[
                        "update-ref",
                        &format!("refs/heads/{branch_name}"),
                        remote_sha.trim(),
                    ],
                );
                tracing::info!(branch = %branch_name, "Fast-forwarded local branch to remote tip");
            }
        } else if local_sha.trim() != remote_sha.trim() {
            tracing::debug!(
                branch = %branch_name,
                "Local and remote have diverged; using local ref as-is"
            );
        }
    } else if !has_local && has_remote {
        // 4. Remote-only: create local tracking branch.
        run_git(cwd, &["branch", "--track", branch_name, &remote_ref]).map_err(|e| {
            GitOpError::Git(format!(
                "Failed to create local branch '{branch_name}' from {remote_ref}: {e}"
            ))
        })?;
        tracing::info!(
            branch = %branch_name,
            "Created local tracking branch from remote"
        );
    } else if !has_local && !has_remote {
        // 5. Neither local nor remote: error
        return Err(GitOpError::BranchNotFound(branch_name.to_string()));
    }
    // has_local && !has_remote: local-only branch, use as-is.

    Ok(())
}

/// Create a git worktree at `.phoenix/worktrees/{conv_id}`.
///
/// If `create_branch` is `Some((new_branch, start_point))`, creates a new branch
/// (`git worktree add -b <new_branch> <path> <start_point>`).
/// If `None`, checks out the existing `branch_name` (`git worktree add <path> <branch>`).
pub(crate) fn create_worktree(
    cwd: &Path,
    conv_id: &str,
    branch_name: &str,
    create_branch: Option<&str>, // start_point for -b mode
) -> Result<String, GitOpError> {
    // 1. Create .phoenix/worktrees/ directory
    let phoenix_dir = cwd.join(".phoenix").join("worktrees");
    std::fs::create_dir_all(&phoenix_dir)
        .map_err(|e| GitOpError::Io(format!("Failed to create .phoenix/worktrees/: {e}")))?;

    let worktree_path = phoenix_dir.join(conv_id);
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    // 2. git worktree add (with or without -b)
    if let Some(start_point) = create_branch {
        run_git(
            cwd,
            &[
                "worktree",
                "add",
                "-b",
                branch_name,
                &worktree_path_str,
                start_point,
            ],
        )
        .map_err(|e| {
            GitOpError::Git(format!(
                "Failed to create worktree with new branch '{branch_name}' from '{start_point}': {e}"
            ))
        })?;
    } else {
        run_git(cwd, &["worktree", "add", &worktree_path_str, branch_name]).map_err(|e| {
            GitOpError::Git(format!(
                "Failed to create worktree for branch '{branch_name}': {e}"
            ))
        })?;
    }

    // 3. Ensure .phoenix/ is in .gitignore at the repo root
    if let Err(e) = ensure_gitignore_has_phoenix(cwd) {
        tracing::warn!(error = %e, "Failed to update .gitignore (non-fatal)");
    }

    Ok(worktree_path_str)
}

/// Check if a branch is already checked out in any worktree.
/// If so, look up the owning conversation for conflict resolution.
pub(crate) fn check_branch_conflict(
    cwd: &Path,
    db: &crate::db::Database,
    branch_name: &str,
) -> Result<(), BranchConflict> {
    if let Some(existing_path) = find_branch_in_worktree_list(cwd, branch_name) {
        // Branch is checked out somewhere. Check if it's a Phoenix conversation.
        if let Some(slug) = find_active_branch_conversation_slug(db, branch_name) {
            return Err(BranchConflict::PhoenixConversation { slug });
        }
        // Not a Phoenix conversation -- checked out in main worktree or external.
        let location = if existing_path == cwd.to_string_lossy() {
            "your main working tree".to_string()
        } else {
            format!("a worktree at {existing_path}")
        };
        return Err(BranchConflict::ExternalCheckout {
            branch: branch_name.to_string(),
            location,
        });
    }
    Ok(())
}

/// Result of a branch conflict check.
#[derive(Debug)]
pub(crate) enum BranchConflict {
    /// Branch is owned by an active Phoenix conversation.
    PhoenixConversation { slug: String },
    /// Branch is checked out externally (main worktree or non-Phoenix worktree).
    ExternalCheckout { branch: String, location: String },
}

/// Check if a branch is already checked out in any worktree.
/// Returns the worktree path if found, None if the branch is free.
/// Uses `git worktree list --porcelain` for structured, reliable detection.
pub(crate) fn find_branch_in_worktree_list(cwd: &Path, branch_name: &str) -> Option<String> {
    let output = run_git(cwd, &["worktree", "list", "--porcelain"]).ok()?;
    let target_ref = format!("refs/heads/{branch_name}");

    let mut current_path: Option<String> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if branch == target_ref {
                return current_path;
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
    None
}

/// Look up the slug of a non-terminal conversation that owns this branch.
/// Runs synchronously (called from a blocking thread).
pub(crate) fn find_active_branch_conversation_slug(
    db: &crate::db::Database,
    branch_name: &str,
) -> Option<String> {
    let rt = tokio::runtime::Handle::try_current().ok()?;
    let convs = rt.block_on(db.get_work_conversations()).ok()?;
    convs
        .iter()
        .find(|c| {
            c.conv_mode.branch_name().is_some_and(|b| b == branch_name) && !c.state.is_terminal()
        })
        .and_then(|c| c.slug.clone())
}

/// Remove a worktree from disk without touching its branch. Mirrors the
/// first step of the abandon / mark-merged cleanup but stops before any
/// `git branch -D`. Used by the `ContextExhausted` cleanup path
/// (specs/projects/projects.allium §5b `WorkBranchWorktreeCleanupOnContextExhausted`),
/// which preserves the branch so a continuation conversation can resume on it.
///
/// Best-effort: tries `git worktree remove --force`, then falls back to
/// `rm -rf` + `git worktree prune`. Returns `Ok(())` whenever the directory
/// is no longer on disk at the end, even if individual git commands failed
/// along the way (the caller only cares about the end state).
pub(crate) fn remove_worktree_preserve_branch(
    repo_root: &Path,
    worktree_path: &str,
) -> Result<(), String> {
    let worktree_dir = std::path::PathBuf::from(worktree_path);

    if let Err(e) = run_git(repo_root, &["worktree", "remove", worktree_path, "--force"]) {
        tracing::warn!(
            error = %e,
            worktree = %worktree_path,
            "git worktree remove failed (non-fatal), trying filesystem fallback"
        );
        let _ = std::fs::remove_dir_all(&worktree_dir);
        let _ = run_git(repo_root, &["worktree", "prune"]);
    }

    if worktree_dir.exists() {
        return Err(format!(
            "worktree directory still present after cleanup: {worktree_path}"
        ));
    }
    Ok(())
}

/// Ensure .gitignore in the given directory contains `.phoenix/`.
/// Creates .gitignore if it doesn't exist. Stages the change if modified.
pub(crate) fn ensure_gitignore_has_phoenix(dir: &Path) -> Result<(), String> {
    use std::io::Write as _;

    let gitignore_path = dir.join(".gitignore");
    let needs_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)
            .map_err(|e| format!("Failed to read .gitignore: {e}"))?;
        !content.lines().any(|line| line.trim() == ".phoenix/")
    } else {
        true
    };

    if needs_update {
        let needs_leading_newline = gitignore_path.exists()
            && std::fs::read(&gitignore_path)
                .ok()
                .is_some_and(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to open .gitignore: {e}"))?;
        if needs_leading_newline {
            writeln!(f).map_err(|e| format!("Failed to write .gitignore: {e}"))?;
        }
        writeln!(f, ".phoenix/").map_err(|e| format!("Failed to write .gitignore: {e}"))?;
        run_git(dir, &["add", ".gitignore"])?;
        tracing::info!(dir = %dir.display(), "Added .phoenix/ to .gitignore");
    }

    Ok(())
}

#[cfg(test)]
mod remove_worktree_preserve_branch_tests {
    use super::*;
    use tempfile::TempDir;

    /// Initialise a repo with one committed file and return (tempdir, repo_root).
    /// Uses `-c` overrides so the test works without a user-level git config.
    fn init_repo_with_commit() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();

        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-m",
                "init",
                "-q",
            ][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        }

        (tmp, root)
    }

    /// Create a worktree at `.phoenix/worktrees/{id}` on a new branch named `branch`.
    fn add_worktree(repo_root: &std::path::Path, id: &str, branch: &str) -> String {
        let wt_dir = repo_root.join(".phoenix").join("worktrees").join(id);
        std::fs::create_dir_all(wt_dir.parent().unwrap()).unwrap();
        let wt_str = wt_dir.to_string_lossy().to_string();

        let status = std::process::Command::new("git")
            .args(["worktree", "add", "-b", branch, &wt_str])
            .current_dir(repo_root)
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add failed");

        wt_str
    }

    fn branch_exists(repo_root: &std::path::Path, branch: &str) -> bool {
        let output = std::process::Command::new("git")
            .args(["branch", "--list", branch])
            .current_dir(repo_root)
            .output()
            .expect("git branch --list");
        !String::from_utf8_lossy(&output.stdout).trim().is_empty()
    }

    /// REQ: worktree directory removed, branch still listed by `git branch`.
    /// Models the Work-mode ContextExhausted cleanup path where the branch
    /// carries committed work that a continuation conversation must resume on.
    #[test]
    fn work_mode_scenario_removes_worktree_preserves_branch() {
        let (_tmp, root) = init_repo_with_commit();
        let wt = add_worktree(&root, "conv-work-1", "task-42-fix-bug");

        assert!(std::path::Path::new(&wt).exists());
        assert!(branch_exists(&root, "task-42-fix-bug"));

        remove_worktree_preserve_branch(&root, &wt).expect("cleanup ok");

        assert!(
            !std::path::Path::new(&wt).exists(),
            "worktree dir must be gone"
        );
        assert!(
            branch_exists(&root, "task-42-fix-bug"),
            "branch must survive — continuation needs it"
        );
    }

    /// REQ: identical behaviour for Branch-mode. `remove_worktree_preserve_branch`
    /// is mode-agnostic, so the branch preservation is the same property.
    #[test]
    fn branch_mode_scenario_removes_worktree_preserves_branch() {
        let (_tmp, root) = init_repo_with_commit();
        let wt = add_worktree(&root, "conv-branch-1", "feature/pr-99");

        remove_worktree_preserve_branch(&root, &wt).expect("cleanup ok");

        assert!(!std::path::Path::new(&wt).exists());
        assert!(
            branch_exists(&root, "feature/pr-99"),
            "user's PR branch must survive"
        );
    }

    /// Fallback path: worktree directory manually deleted behind git's back.
    /// `git worktree remove` fails, fallback `rm -rf` + `prune` still succeeds.
    /// End state is what the caller cares about.
    #[test]
    fn recovers_when_git_worktree_remove_fails() {
        let (_tmp, root) = init_repo_with_commit();
        let wt = add_worktree(&root, "conv-dirty", "task-99-dirty");

        // Nuke the directory out from under git — `git worktree remove` will
        // refuse. The fallback should still converge to a clean state.
        std::fs::remove_dir_all(&wt).expect("rm -rf");

        remove_worktree_preserve_branch(&root, &wt).expect("fallback ok");

        assert!(!std::path::Path::new(&wt).exists());
        assert!(branch_exists(&root, "task-99-dirty"));
    }
}
