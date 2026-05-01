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

/// Resolve `base_branch` to the freshest available comparator ref.
///
/// Returns `"origin/<base>"` when the remote-tracking ref exists, falling
/// back to bare `"<base>"` for local-only repos with no remote.
///
/// Diff comparisons in this codebase (abandon snapshot, commits-ahead/behind,
/// the conversation diff endpoint) historically used bare `<base>`. The local
/// ref is only fast-forwarded at lifecycle events via `materialize_branch`;
/// the periodic 1-minute fetch loop in `stream_conversation` keeps
/// `origin/<base>` fresh but does NOT re-materialize the local ref. So bare
/// `<base>` drifts stale for any task that lives across upstream advances.
/// Routing all diff-comparison call sites through this helper makes them
/// prefer the already-fresh remote-tracking ref.
pub(crate) fn effective_base_ref(cwd: &Path, base_branch: &str) -> String {
    let remote = format!("origin/{base_branch}");
    if run_git(cwd, &["rev-parse", "--verify", &remote]).is_ok() {
        remote
    } else {
        base_branch.to_string()
    }
}

/// Capture of "what this branch has done relative to its base."
///
/// Used by both the abandon snapshot and the conversation diff endpoint.
/// All fields are best-effort — empty strings when the underlying git
/// command fails (e.g. worktree gone, branch gone). Callers decide how to
/// truncate; this helper returns full output.
pub(crate) struct CapturedDiff {
    /// The ref used as the comparator (`"origin/<base>"` or bare `"<base>"`).
    pub comparator: String,
    /// `git log --oneline <comparator>..HEAD` — commits on the branch that
    /// are not yet in the comparator. Subject lines only.
    pub commit_log: String,
    /// `git diff <comparator>...HEAD` (triple-dot) — file-level diff of
    /// committed work vs the common ancestor. Immune to commit-history
    /// noise from rebases that replay already-merged commits.
    pub committed_diff: String,
    /// `git diff HEAD` after `git add -N .` — working-tree changes
    /// (staged + unstaged + untracked, surfaced as intent-to-add).
    pub uncommitted_diff: String,
}

/// Capture committed and uncommitted state of `worktree` relative to
/// `base_branch`. See `CapturedDiff` for field semantics.
pub(crate) fn capture_branch_diff(worktree: &Path, base_branch: &str) -> CapturedDiff {
    let comparator = effective_base_ref(worktree, base_branch);

    let commit_log = run_git(
        worktree,
        &["log", "--oneline", &format!("{comparator}..HEAD")],
    )
    .unwrap_or_default();

    let committed_diff =
        run_git(worktree, &["diff", &format!("{comparator}...HEAD")]).unwrap_or_default();

    // Stage untracked files as intent-to-add so they surface in the
    // working-tree diff. Agent-created files are typically untracked.
    let _ = run_git(worktree, &["add", "-N", "."]);
    let uncommitted_diff = run_git(worktree, &["diff", "HEAD"]).unwrap_or_default();

    CapturedDiff {
        comparator,
        commit_log,
        committed_diff,
        uncommitted_diff,
    }
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Initialize an empty repo with a single commit on `main`.
    fn init_repo(dir: &Path) {
        run_git(dir, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        run_git(dir, &["config", "user.email", "probe@test"]).unwrap();
        run_git(dir, &["config", "user.name", "probe"]).unwrap();
        run_git(dir, &["commit", "--allow-empty", "-q", "-m", "init"]).unwrap();
    }

    #[test]
    fn effective_base_ref_falls_back_to_local_when_no_remote() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // No `origin` remote exists, so the helper must return the bare ref.
        assert_eq!(effective_base_ref(tmp.path(), "main"), "main");
    }

    #[test]
    fn effective_base_ref_prefers_origin_when_available() {
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());

        let clone = TempDir::new().unwrap();
        // `git clone` fully populates origin/* refs.
        run_git(
            std::env::current_dir().unwrap().as_path(),
            &[
                "clone",
                "--quiet",
                upstream.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ],
        )
        .unwrap();
        run_git(clone.path(), &["config", "user.email", "probe@test"]).unwrap();
        run_git(clone.path(), &["config", "user.name", "probe"]).unwrap();

        assert_eq!(effective_base_ref(clone.path(), "main"), "origin/main");
    }

    #[test]
    fn effective_base_ref_falls_back_when_named_branch_not_at_origin() {
        // Origin exists for some branches but not the requested one.
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());

        let clone = TempDir::new().unwrap();
        run_git(
            std::env::current_dir().unwrap().as_path(),
            &[
                "clone",
                "--quiet",
                upstream.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ],
        )
        .unwrap();
        // origin/main exists; origin/feature does not. A local `feature`
        // branch with no upstream should fall back to bare.
        run_git(clone.path(), &["branch", "feature"]).unwrap();
        assert_eq!(effective_base_ref(clone.path(), "feature"), "feature");
    }
}
