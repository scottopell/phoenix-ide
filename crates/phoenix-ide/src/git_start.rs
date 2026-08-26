#![allow(dead_code)]
use std::path::{Path, PathBuf};

use crate::git_ops::{find_branch_in_worktree_list, materialize_branch, run_git, GitOpError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogicalBaseBranch(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckoutRef(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeRef(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitStartPoint {
    logical_base: LogicalBaseBranch,
    checkout_ref: CheckoutRef,
    tree_ref: TreeRef,
    reserved_oid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitStartError {
    BranchNotFound(String),
    DetachedHead,
    Git(String),
}

impl std::fmt::Display for GitStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BranchNotFound(branch) => {
                write!(f, "Branch '{branch}' not found locally or at origin")
            }
            Self::DetachedHead => write!(
                f,
                "Cannot determine the base branch for this approval (the conversation didn't \
                 record one and the repository is on a detached HEAD). Re-create the \
                 conversation with an explicit base branch."
            ),
            Self::Git(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for GitStartError {}

impl From<GitOpError> for GitStartError {
    fn from(value: GitOpError) -> Self {
        match value {
            GitOpError::BranchNotFound(branch) => Self::BranchNotFound(branch),
            GitOpError::Git(message) | GitOpError::Io(message) => Self::Git(message),
        }
    }
}

impl GitStartPoint {
    pub(crate) fn new(
        logical_base: impl Into<String>,
        checkout_ref: impl Into<String>,
        tree_ref: impl Into<String>,
    ) -> Self {
        Self {
            logical_base: LogicalBaseBranch(logical_base.into()),
            checkout_ref: CheckoutRef(checkout_ref.into()),
            tree_ref: TreeRef(tree_ref.into()),
            reserved_oid: None,
        }
    }

    pub(crate) fn with_reserved_oid(
        logical_base: impl Into<String>,
        checkout_ref: impl Into<String>,
        tree_ref: impl Into<String>,
        reserved_oid: impl Into<String>,
    ) -> Self {
        Self {
            logical_base: LogicalBaseBranch(logical_base.into()),
            checkout_ref: CheckoutRef(checkout_ref.into()),
            tree_ref: TreeRef(tree_ref.into()),
            reserved_oid: Some(reserved_oid.into()),
        }
    }

    pub(crate) fn logical_base(&self) -> &str {
        &self.logical_base.0
    }

    pub(crate) fn checkout_ref(&self) -> &str {
        &self.checkout_ref.0
    }

    pub(crate) fn tree_ref(&self) -> &str {
        &self.tree_ref.0
    }

    pub(crate) fn reserved_oid(&self) -> Option<&str> {
        self.reserved_oid.as_deref()
    }

    pub(crate) fn for_create_request(
        repo_root: &Path,
        base_branch: &str,
        checkout_ref: Option<&str>,
    ) -> Result<Self, GitStartError> {
        let checkout = checkout_ref.unwrap_or(base_branch);
        resolve_checkout_for_materialized_use(repo_root, checkout)?;
        Ok(Self::new(base_branch, checkout, checkout))
    }

    pub(crate) fn for_inline_discovery(
        cwd: &Path,
        mode: &str,
        base_branch: Option<&str>,
    ) -> Option<Self> {
        if !matches!(mode, "branch" | "managed") {
            return None;
        }
        let branch = base_branch.filter(|b| !b.is_empty())?;
        let repo_root = PathBuf::from(phoenix_core::git::detect_git_repo_root(cwd)?);
        let tree_ref = resolve_tree_ref_without_fetch(&repo_root, branch)?;
        Some(Self::new(branch, tree_ref.clone(), tree_ref))
    }

    pub(crate) fn cached_default_task_start(repo_root: &Path) -> Option<Self> {
        let default_branch = if has_remote_named_origin(repo_root) {
            origin_head_branch(repo_root)?
        } else {
            resolve_local_only_default_branch(repo_root)?
        };
        let checkout_ref = preferred_default_checkout_ref(repo_root, &default_branch)?;
        let oid = resolve_commit_oid(repo_root, &checkout_ref)?;
        let mut start = Self::new(default_branch, oid.clone(), oid.clone());
        start.reserved_oid = Some(oid);
        Some(start)
    }

    pub(crate) fn for_default_task_start(repo_root: &Path) -> Option<Self> {
        let resolved = refreshed_default_task_start(repo_root)?;
        let mut start = Self::new(
            resolved.default_branch.clone(),
            resolved.checkout_ref.clone(),
            resolved.checkout_ref,
        );
        start.reserved_oid = Some(resolved.reserved_oid);
        Some(start)
    }

    pub(crate) fn for_approval(
        cwd: &Path,
        repo_root: &Path,
        desired_base_branch: Option<&str>,
    ) -> Result<Self, GitStartError> {
        let base_branch = match desired_base_branch {
            Some(branch) => branch.to_string(),
            None => current_branch(repo_root)?,
        };
        resolve_checkout_for_materialized_use(cwd, &base_branch)?;
        Ok(Self::new(&base_branch, &base_branch, &base_branch))
    }
}

pub(crate) fn is_explicit_ref(reference: &str) -> bool {
    reference.starts_with("origin/") || reference.starts_with("refs/")
}

pub(crate) fn effective_base_ref(cwd: &Path, base_branch: &str) -> String {
    if is_explicit_ref(base_branch) {
        return base_branch.to_string();
    }
    let remote = format!("origin/{base_branch}");
    if verify_commit(cwd, &remote) {
        remote
    } else {
        base_branch.to_string()
    }
}

pub(crate) fn resolve_tree_ref_without_fetch(repo_root: &Path, branch: &str) -> Option<String> {
    if is_explicit_ref(branch) {
        return verify_commit(repo_root, branch).then(|| branch.to_string());
    }

    let remote = format!("origin/{branch}");
    let has_local = verify_commit(repo_root, branch);
    let has_remote = verify_commit(repo_root, &remote);
    match (has_local, has_remote) {
        (true, true) => {
            let local_is_ancestor =
                run_git(repo_root, &["merge-base", "--is-ancestor", branch, &remote]).is_ok();
            if local_is_ancestor && find_branch_in_worktree_list(repo_root, branch).is_none() {
                Some(remote)
            } else {
                Some(branch.to_string())
            }
        }
        (false, true) => Some(remote),
        (true, false) => Some(branch.to_string()),
        (false, false) => None,
    }
}

fn resolve_checkout_for_materialized_use(
    repo_root: &Path,
    checkout: &str,
) -> Result<(), GitStartError> {
    if is_explicit_ref(checkout) {
        verify_commit(repo_root, checkout)
            .then_some(())
            .ok_or_else(|| GitStartError::Git(format!("Git ref '{checkout}' not found")))
    } else {
        materialize_branch(repo_root, checkout).map_err(Into::into)
    }
}

pub(crate) fn refresh_and_resolve_default_branch_for_reservation(
    repo_root: &Path,
) -> Option<String> {
    refresh_and_resolve_default_branch(repo_root)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefreshedDefaultTaskStart {
    default_branch: String,
    checkout_ref: String,
    reserved_oid: String,
}

fn discover_remote_head_branch(repo_root: &Path) -> Option<String> {
    let output = phoenix_core::git::command()
        .current_dir(repo_root)
        .args(["ls-remote", "--symref", "origin", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("ref: refs/heads/"))
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_string)
}

fn refreshed_default_task_start(repo_root: &Path) -> Option<RefreshedDefaultTaskStart> {
    if has_remote_named_origin(repo_root) {
        let default_branch =
            origin_head_branch(repo_root).or_else(|| discover_remote_head_branch(repo_root))?;
        run_git(
            repo_root,
            &[
                "fetch",
                "origin",
                "--no-tags",
                &format!("+refs/heads/{default_branch}:refs/remotes/origin/{default_branch}"),
            ],
        )
        .inspect_err(|e| tracing::debug!(error = %e, branch = %default_branch, "project task targeted default fetch failed"))
        .ok()?;
        let checkout_ref = preferred_default_checkout_ref(repo_root, &default_branch)?;
        let reserved_oid = resolve_commit_oid(repo_root, &checkout_ref)?;
        return Some(RefreshedDefaultTaskStart {
            default_branch,
            checkout_ref,
            reserved_oid,
        });
    }

    let default_branch = resolve_local_only_default_branch(repo_root)?;
    let checkout_ref = preferred_default_checkout_ref(repo_root, &default_branch)?;
    let reserved_oid = resolve_commit_oid(repo_root, &checkout_ref)?;
    Some(RefreshedDefaultTaskStart {
        default_branch,
        checkout_ref,
        reserved_oid,
    })
}

fn refresh_and_resolve_default_branch(repo_root: &Path) -> Option<String> {
    refreshed_default_task_start(repo_root).map(|resolved| resolved.default_branch)
}

fn resolve_local_only_default_branch(repo_root: &Path) -> Option<String> {
    let branches = local_heads(repo_root);
    match branches.as_slice() {
        [only] => return Some(only.clone()),
        [] => return None,
        _ => {}
    }

    let init_default = init_default_branch(repo_root)?;
    branches
        .into_iter()
        .find(|branch| branch == &init_default)
        .filter(|branch| !branch.is_empty())
}

fn local_heads(repo_root: &Path) -> Vec<String> {
    run_git(
        repo_root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .ok()
    .map(|output| {
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

fn init_default_branch(repo_root: &Path) -> Option<String> {
    run_git(repo_root, &["config", "--get", "init.defaultBranch"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn has_remote_named_origin(repo_root: &Path) -> bool {
    run_git(repo_root, &["remote", "get-url", "origin"]).is_ok()
}

pub(crate) fn preferred_default_checkout_ref_for_reservation(
    repo_root: &Path,
    default_branch: &str,
) -> Option<String> {
    preferred_default_checkout_ref(repo_root, default_branch)
}

fn preferred_default_checkout_ref(repo_root: &Path, default_branch: &str) -> Option<String> {
    let remote_ref = format!("origin/{default_branch}");
    if verify_commit(repo_root, &remote_ref) {
        Some(remote_ref)
    } else if verify_commit(repo_root, default_branch) {
        Some(default_branch.to_string())
    } else {
        None
    }
}

pub(crate) fn resolve_commit_oid_for_reservation(repo_root: &Path, rev: &str) -> Option<String> {
    resolve_commit_oid(repo_root, rev)
}

fn resolve_commit_oid(repo_root: &Path, rev: &str) -> Option<String> {
    run_git(repo_root, &["rev-parse", &format!("{rev}^{{commit}}")])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn origin_head_branch(repo_root: &Path) -> Option<String> {
    run_git(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|s| {
            s.trim()
                .strip_prefix("refs/remotes/origin/")
                .map(String::from)
        })
}

fn current_branch(repo_root: &Path) -> Result<String, GitStartError> {
    let branch = run_git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(GitStartError::Git)?
        .trim()
        .to_string();
    if branch.is_empty() || branch == "HEAD" {
        Err(GitStartError::DetachedHead)
    } else {
        Ok(branch)
    }
}

fn verify_commit(repo_root: &Path, rev: &str) -> bool {
    run_git(
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let out = phoenix_core::git::command()
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo_with_origin() -> (TempDir, TempDir) {
        let origin = TempDir::new().unwrap();
        git(origin.path(), &["init", "--bare", "-q"]);

        let clone = TempDir::new().unwrap();
        git(clone.path(), &["init", "-q", "-b", "main"]);
        git(
            clone.path(),
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        std::fs::write(clone.path().join("README.md"), "one").unwrap();
        git(clone.path(), &["add", "."]);
        git(clone.path(), &["commit", "-qm", "initial"]);
        git(clone.path(), &["push", "-u", "origin", "main"]);
        git(origin.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(clone.path(), &["remote", "set-head", "origin", "-a"]);
        (origin, clone)
    }

    #[test]
    fn default_task_start_uses_logical_branch_and_remote_tree() {
        let (_origin, clone) = repo_with_origin();
        let start = GitStartPoint::for_default_task_start(clone.path()).unwrap();
        assert_eq!(start.logical_base(), "main");
        assert_eq!(start.checkout_ref(), "origin/main");
        assert_eq!(start.tree_ref(), "origin/main");
    }

    #[test]
    fn default_task_start_falls_back_to_local_only_repo() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(repo.path().join("README.md"), "one").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "initial"]);

        let start = GitStartPoint::for_default_task_start(repo.path()).unwrap();
        assert_eq!(start.logical_base(), "main");
        assert_eq!(start.checkout_ref(), "main");
        assert_eq!(start.tree_ref(), "main");
        assert_eq!(
            start.reserved_oid(),
            resolve_commit_oid(repo.path(), "main").as_deref()
        );
    }

    #[test]
    fn default_task_start_uses_init_default_branch_for_local_only_repo_with_multiple_heads() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        git(repo.path(), &["config", "init.defaultBranch", "main"]);
        std::fs::write(repo.path().join("README.md"), "one").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "initial"]);
        git(repo.path(), &["branch", "feature"]);
        git(repo.path(), &["checkout", "-q", "feature"]);

        let start = GitStartPoint::for_default_task_start(repo.path()).unwrap();
        assert_eq!(start.logical_base(), "main");
        assert_eq!(start.checkout_ref(), "main");
        assert_eq!(start.tree_ref(), "main");
    }

    #[test]
    fn default_task_start_is_unresolved_for_local_only_repo_with_multiple_heads_and_no_authority() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        git(repo.path(), &["config", "init.defaultBranch", ""]);
        std::fs::write(repo.path().join("README.md"), "one").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "initial"]);
        git(repo.path(), &["branch", "feature"]);
        git(repo.path(), &["checkout", "-q", "feature"]);

        assert!(GitStartPoint::for_default_task_start(repo.path()).is_none());
    }

    #[test]
    fn explicit_create_ref_is_not_materialized_as_branch() {
        let (_origin, clone) = repo_with_origin();
        let start =
            GitStartPoint::for_create_request(clone.path(), "main", Some("origin/main")).unwrap();
        assert_eq!(start.logical_base(), "main");
        assert_eq!(start.checkout_ref(), "origin/main");
        assert_eq!(start.tree_ref(), "origin/main");
    }

    #[test]
    fn normal_create_branch_materializes_and_stays_logical() {
        let (_origin, clone) = repo_with_origin();
        let start = GitStartPoint::for_create_request(clone.path(), "main", None).unwrap();
        assert_eq!(start.logical_base(), "main");
        assert_eq!(start.checkout_ref(), "main");
        assert_eq!(start.tree_ref(), "main");
    }

    #[test]
    fn inline_discovery_prefers_remote_when_local_unpinned_and_behind() {
        let (origin, clone) = repo_with_origin();
        git(clone.path(), &["checkout", "-q", "-b", "feature"]);
        git(clone.path(), &["push", "-u", "origin", "feature"]);
        git(clone.path(), &["checkout", "-q", "main"]);

        let updater = TempDir::new().unwrap();
        git(
            updater.path(),
            &["clone", "-q", origin.path().to_str().unwrap(), "."],
        );
        git(updater.path(), &["checkout", "-q", "feature"]);
        std::fs::write(updater.path().join("README.md"), "two").unwrap();
        git(updater.path(), &["add", "."]);
        git(updater.path(), &["commit", "-qm", "advance"]);
        git(updater.path(), &["push", "origin", "feature"]);
        git(clone.path(), &["fetch", "origin", "feature"]);

        let start =
            GitStartPoint::for_inline_discovery(clone.path(), "managed", Some("feature")).unwrap();
        assert_eq!(start.logical_base(), "feature");
        assert_eq!(start.tree_ref(), "origin/feature");
    }

    #[test]
    fn default_task_start_targeted_fetch_updates_only_default_branch() {
        let (origin, clone) = repo_with_origin();
        git(clone.path(), &["checkout", "-q", "-b", "feature"]);
        git(clone.path(), &["push", "-u", "origin", "feature"]);
        let old_feature = run_git(clone.path(), &["rev-parse", "origin/feature"]).unwrap();
        git(clone.path(), &["checkout", "-q", "main"]);

        let updater = TempDir::new().unwrap();
        git(
            updater.path(),
            &["clone", "-q", origin.path().to_str().unwrap(), "."],
        );
        std::fs::write(updater.path().join("README.md"), "main-two").unwrap();
        git(updater.path(), &["add", "."]);
        git(updater.path(), &["commit", "-qm", "advance main"]);
        git(updater.path(), &["push", "origin", "main"]);
        git(updater.path(), &["checkout", "-q", "feature"]);
        std::fs::write(updater.path().join("feature.txt"), "feature-two").unwrap();
        git(updater.path(), &["add", "."]);
        git(updater.path(), &["commit", "-qm", "advance feature"]);
        git(updater.path(), &["push", "origin", "feature"]);

        let start = GitStartPoint::for_default_task_start(clone.path()).unwrap();
        assert_eq!(start.logical_base(), "main");
        assert_eq!(start.checkout_ref(), "origin/main");
        let new_main = run_git(clone.path(), &["rev-parse", "origin/main"]).unwrap();
        let feature_after = run_git(clone.path(), &["rev-parse", "origin/feature"]).unwrap();
        assert_ne!(
            new_main.trim(),
            run_git(clone.path(), &["rev-parse", "main"])
                .unwrap()
                .trim()
        );
        assert_eq!(feature_after.trim(), old_feature.trim());
    }
}
