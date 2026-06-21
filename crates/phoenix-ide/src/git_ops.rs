//! Consolidated git operations for worktree management.
//!
//! Previously scattered across `api::handlers` and `runtime::executor`,
//! these functions implement the fetch + materialize + worktree-create
//! pipeline used by Branch mode, Managed mode, and task approval.

use std::path::Path;

/// Walks back from `end` until the byte slice `bytes[..end]` is on a
/// valid UTF-8 character boundary. Used by `run_git_capped` so the
/// truncated buffer is always parseable as UTF-8 (any incomplete
/// multi-byte sequence at the cut point gets dropped, not patched
/// with U+FFFD).
///
/// The walk is bounded at 4 bytes (max UTF-8 sequence length) so the
/// worst case is constant work regardless of buffer size.
fn utf8_floor_boundary(bytes: &[u8], end: usize) -> usize {
    let end = end.min(bytes.len());
    let mut cut = end;
    // At most 4 byte rewinds; UTF-8 sequences are 1-4 bytes long.
    for _ in 0..4 {
        if std::str::from_utf8(&bytes[..cut]).is_ok() {
            return cut;
        }
        if cut == 0 {
            return 0;
        }
        cut -= 1;
    }
    cut
}

/// Outcome of a size-limited git stdout capture (see `run_git_capped`).
pub(crate) struct CappedStdout {
    /// Truncated stdout — at most `max_bytes` long, cut on a UTF-8
    /// character boundary.
    pub stdout: String,
    /// Total stdout bytes seen. Exact when `saturated == false`; a lower
    /// bound when `saturated == true` (we hit the hard read limit and
    /// killed the child).
    pub total_bytes: u64,
    /// True when the streaming reader hit its hard limit and stopped
    /// counting before the child finished. Diffs that can produce more
    /// than `hard_limit` bytes will set this; the truncation indicator
    /// in the UI should treat `total_bytes` as a lower bound.
    pub saturated: bool,
}

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
    run_git_with_env(cwd, args, &[])
}

/// Like [`run_git`], but returns raw stdout bytes — no trimming, no UTF-8
/// lossy conversion. Required when the output is file content (e.g.
/// `git cat-file -p <ref>:<path>`) where trailing whitespace is significant
/// and binary detection needs the exact bytes.
pub(crate) fn run_git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false");
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}

/// Like [`run_git`], but layers `extra_env` on top of the signing-disabled
/// defaults. Used by [`capture_branch_diff`] to redirect index writes via
/// `GIT_INDEX_FILE` so a read-only diff capture doesn't mutate the worktree's
/// real index.
pub(crate) fn run_git_with_env(
    cwd: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false");
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}

/// Run `git` and stream stdout, keeping at most `max_bytes` in memory.
/// Continues counting up to `hard_limit_bytes` so callers can report
/// "X KiB total" for the truncation indicator; if the child still has
/// more output past `hard_limit_bytes`, the stream is killed and the
/// returned `total_bytes` is a lower bound (`saturated = true`).
///
/// The point of this helper is to keep peak memory bounded for the
/// per-conversation diff endpoint, which can be invoked repeatedly from
/// the UI on worktrees with arbitrarily large diffs. Without it,
/// `Command::output()` materialises the entire stdout buffer before any
/// truncation runs.
///
/// `max_bytes == 0` returns an empty string but still drains/counts the
/// stream up to the hard limit.
pub(crate) fn run_git_capped(
    cwd: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    max_bytes: usize,
    hard_limit_bytes: u64,
) -> Result<CappedStdout, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn git {}: {e}", args.join(" ")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "git child had no stdout".to_string())?;

    let initial_capacity = max_bytes.min(64 * 1024);
    let mut buf: Vec<u8> = Vec::with_capacity(initial_capacity);
    let mut total_bytes: u64 = 0;
    let mut saturated = false;
    let mut chunk = [0u8; 8192];

    loop {
        let n = match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} read failed: {e}", args.join(" ")));
            }
        };
        // Append to the in-memory buffer up to `max_bytes`. Anything past
        // the cap is discarded immediately; we only count it.
        if buf.len() < max_bytes {
            let take = (max_bytes - buf.len()).min(n);
            buf.extend_from_slice(&chunk[..take]);
        }
        total_bytes = total_bytes.saturating_add(n as u64);
        if total_bytes > hard_limit_bytes {
            saturated = true;
            let _ = child.kill();
            // Drain any remaining buffered bytes from the OS pipe so the
            // child can exit cleanly; cap the drain at a small bound.
            let mut drained = 0u64;
            while drained < 64 * 1024 {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(m) => drained += m as u64,
                }
            }
            break;
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("git {} wait failed: {e}", args.join(" ")))?;

    if !status.success() && !saturated {
        // Git failed (and we didn't kill it). Capture stderr for the
        // error message — small + bounded so a full read is fine.
        let mut stderr_bytes = Vec::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_end(&mut stderr_bytes);
        }
        let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")));
    }

    // Truncate the raw byte buffer at a valid UTF-8 boundary BEFORE
    // converting. Doing it the other way around (lossy → string →
    // floor_char_boundary) would let `from_utf8_lossy` insert U+FFFD
    // for an incomplete trailing multi-byte sequence and silently
    // change the byte length, producing more truncation than asked.
    let cut = utf8_floor_boundary(&buf, buf.len().min(max_bytes));
    let truncated = String::from_utf8_lossy(&buf[..cut]).into_owned();

    Ok(CappedStdout {
        stdout: truncated,
        total_bytes,
        saturated,
    })
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
            if let Some(worktree_path) = find_branch_in_worktree_list(cwd, branch_name) {
                tracing::debug!(
                    branch = %branch_name,
                    worktree = %worktree_path,
                    "Cannot fast-forward: branch is checked out in a worktree"
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
/// Diff comparisons in this codebase (abandon snapshot, the conversation
/// diff endpoint) historically used bare `<base>`. The local ref is only
/// fast-forwarded at lifecycle events via `materialize_branch`, so bare
/// `<base>` drifts stale for any task that lives across upstream advances.
/// Routing all diff-comparison call sites through this helper makes them
/// prefer the remote-tracking ref when one exists.
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
/// command fails (e.g. worktree gone, branch gone).
///
/// Diff fields are captured via `run_git_capped`, which streams stdout
/// and stops appending to memory after `max_section_bytes` while
/// continuing to count up to the hard limit. The `*_total_bytes` and
/// `*_saturated` fields let callers render an accurate truncation
/// indicator without re-running git.
pub(crate) struct CapturedDiff {
    /// The ref used as the comparator (`"origin/<base>"` or bare `"<base>"`).
    pub comparator: String,
    /// `git log --oneline <comparator>..HEAD` — commits on the branch that
    /// are not yet in the comparator. Subject lines only; not capped
    /// (commit titles are tiny).
    pub commit_log: String,
    /// `git diff <comparator>...HEAD` (triple-dot), capped at
    /// `max_section_bytes`.
    pub committed_diff: String,
    /// Total stdout size of the committed-diff stream. Exact unless
    /// `committed_saturated == true` (see `CappedStdout`).
    pub committed_total_bytes: u64,
    pub committed_saturated: bool,
    /// `git diff HEAD` capturing working-tree changes (staged + unstaged +
    /// untracked, surfaced via a `GIT_INDEX_FILE` temp index so the real
    /// index is never mutated). Capped at `max_section_bytes`.
    pub uncommitted_diff: String,
    pub uncommitted_total_bytes: u64,
    pub uncommitted_saturated: bool,
}

/// RAII guard that removes a temp file on drop. Used by `capture_branch_diff`
/// to ensure the `GIT_INDEX_FILE` temp index is cleaned up even on panic.
struct TempPath(std::path::PathBuf);

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Capture committed and uncommitted state of `worktree` relative to
/// `base_branch`. See `CapturedDiff` for field semantics.
///
/// `max_section_bytes` caps each diff section in memory; the streaming
/// reader continues to count past the cap up to a hard limit
/// (`max_section_bytes * 8`) so the response can include an accurate
/// "X KiB total" truncation indicator. Beyond the hard limit the count
/// becomes a lower bound (see `CappedStdout::saturated`).
///
/// The uncommitted-diff capture surfaces untracked files via `git add -N`
/// against a `GIT_INDEX_FILE` temp index copied from the worktree's real
/// one. Real index is never touched. If the temp index cannot be set up
/// (e.g. `git rev-parse --git-dir` fails or the copy errors), the
/// uncommitted-diff capture falls back to a plain `git diff HEAD` that
/// skips untracked files rather than mutating.
///
/// `commit_log` is bounded at `MAX_COMMITS_IN_LOG` so long-lived branches
/// don't blow up the response with thousands of subject lines.
const MAX_COMMITS_IN_LOG: u32 = 200;

pub(crate) fn capture_branch_diff(
    worktree: &Path,
    base_branch: &str,
    max_section_bytes: usize,
) -> CapturedDiff {
    let comparator = effective_base_ref(worktree, base_branch);
    // Hard limit on bytes streamed before we kill the child process. 8x
    // the visible cap is a comfortable margin for "we know the diff is
    // bigger than this" without burning unbounded CPU on monster diffs.
    let hard_limit = (max_section_bytes as u64).saturating_mul(8);

    // Bound the commit list (see MAX_COMMITS_IN_LOG above) so long-
    // lived branches don't blow up the wire response.
    let commit_log = run_git(
        worktree,
        &[
            "log",
            "--oneline",
            "--max-count",
            &MAX_COMMITS_IN_LOG.to_string(),
            &format!("{comparator}..HEAD"),
        ],
    )
    .unwrap_or_default();

    let committed = run_git_capped(
        worktree,
        &["diff", &format!("{comparator}...HEAD")],
        &[],
        max_section_bytes,
        hard_limit,
    )
    .unwrap_or(CappedStdout {
        stdout: String::new(),
        total_bytes: 0,
        saturated: false,
    });

    let uncommitted = capture_uncommitted_diff(worktree, max_section_bytes, hard_limit);

    CapturedDiff {
        comparator,
        commit_log,
        committed_diff: committed.stdout,
        committed_total_bytes: committed.total_bytes,
        committed_saturated: committed.saturated,
        uncommitted_diff: uncommitted.stdout,
        uncommitted_total_bytes: uncommitted.total_bytes,
        uncommitted_saturated: uncommitted.saturated,
    }
}

fn capture_uncommitted_diff(worktree: &Path, max_bytes: usize, hard_limit: u64) -> CappedStdout {
    let empty = || CappedStdout {
        stdout: String::new(),
        total_bytes: 0,
        saturated: false,
    };

    // Try to set up an isolated index. If anything fails, fall back to a
    // tracked-only diff — never mutate the real index.
    let Some(temp) = prepare_temp_index(worktree) else {
        tracing::debug!(
            worktree = %worktree.display(),
            "could not isolate git index — falling back to tracked-only uncommitted diff"
        );
        return run_git_capped(worktree, &["diff", "HEAD"], &[], max_bytes, hard_limit)
            .unwrap_or_else(|_| empty());
    };

    let temp_path_str = temp.0.to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", temp_path_str.as_str())];

    // Stage untracked files in the temp index so they surface in the diff.
    // Errors here are non-fatal — diff just won't include the untracked.
    let _ = run_git_with_env(worktree, &["add", "-N", "."], &env);
    run_git_capped(worktree, &["diff", "HEAD"], &env, max_bytes, hard_limit)
        .unwrap_or_else(|_| empty())
}

/// Find the worktree's git index, copy it to a unique temp path, and
/// return a guard that cleans up the copy on drop. Returns `None` if any
/// step fails.
fn prepare_temp_index(worktree: &Path) -> Option<TempPath> {
    let git_dir = run_git(worktree, &["rev-parse", "--git-dir"]).ok()?;
    // `git rev-parse --git-dir` returns a path that may be relative to
    // `worktree`. Resolve it.
    let git_dir = {
        let p = std::path::Path::new(git_dir.trim());
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            worktree.join(p)
        }
    };
    let real_index = git_dir.join("index");
    let temp = phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()
        .tmp_subdir("git-index")
        .ok()?
        .join(format!("idx-{}", uuid::Uuid::new_v4()));
    if real_index.exists() {
        std::fs::copy(&real_index, &temp).ok()?;
    } else {
        // No existing index (rare — fresh worktree). Touch an empty file
        // so GIT_INDEX_FILE has something to point at; git will populate.
        std::fs::write(&temp, []).ok()?;
    }
    Some(TempPath(temp))
}

/// How a worktree-create call makes `.phoenix/` ignored in the repo it cuts the
/// worktree from.
///
/// The two strategies are not interchangeable: one mutates and stages a tracked
/// file, the other writes a local untracked file. Which one is correct depends
/// on whether the caller is allowed to dirty the origin checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhoenixIgnoreStrategy {
    /// Append `.phoenix/` to the tracked `.gitignore` and `git add` it. Used by
    /// Branch / Managed / Explore-gateway worktree creation, where staging the
    /// ignore rule is the intended, durable behavior.
    StageGitignore,
    /// Append `.phoenix/` to the repo's local untracked exclude file
    /// (`.git/info/exclude`), staging nothing. Used by fork resolution, which
    /// MUST leave the originating checkout's index and tracked `.gitignore`
    /// unaffected (REQ-PROJ-035).
    LocalExclude,
}

/// Create a git worktree at `.phoenix/worktrees/{conv_id}`.
///
/// If `create_branch` is `Some((new_branch, start_point))`, creates a new branch
/// (`git worktree add -b <new_branch> <path> <start_point>`).
/// If `None`, checks out the existing `branch_name` (`git worktree add <path> <branch>`).
///
/// `ignore_strategy` selects how `.phoenix/` is made ignored in `cwd`'s repo —
/// see [`PhoenixIgnoreStrategy`].
pub(crate) fn create_worktree(
    cwd: &Path,
    conv_id: &str,
    branch_name: &str,
    create_branch: Option<&str>, // start_point for -b mode
    ignore_strategy: PhoenixIgnoreStrategy,
) -> Result<String, GitOpError> {
    // For LocalExclude (fork resolution), make `.phoenix/` ignored via the repo's
    // `.git/info/exclude` BEFORE creating the `.phoenix/worktrees` dir or running
    // `git worktree add`. If the add later fails (occupied deterministic path, FS
    // error), the exclude is already in place, so the partial `.phoenix/` dir never
    // surfaces as `?? .phoenix/` in the originating checkout — the decoupling this
    // strategy preserves (REQ-PROJ-035). StageGitignore keeps its post-add ordering
    // below (staging a tracked file before the worktree exists has no benefit and
    // would dirty the index on a failed add).
    if ignore_strategy == PhoenixIgnoreStrategy::LocalExclude {
        if let Err(e) = ensure_local_exclude_has_phoenix(cwd) {
            tracing::warn!(error = %e, "Failed to ignore .phoenix/ via local exclude before worktree add (non-fatal)");
        }
    }

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
                // `--` ends option parsing so the positional path and start-point
                // (the latter caller-influenced) can never be read as flags.
                "--",
                &worktree_path_str,
                start_point,
            ],
        )
        .map_err(|e| {
            // `git worktree add -b` creates the branch ref before checking out
            // files. A mid-checkout failure (commonly ENOSPC) rolls back the
            // worktree dir + admin entry but leaves the branch we created behind.
            // Roll it back so a retry isn't permanently poisoned by a stale ref.
            cleanup_failed_worktree(cwd, &worktree_path, Some(branch_name));
            GitOpError::Git(format!(
                "Failed to create worktree with new branch '{branch_name}' from '{start_point}': {e}"
            ))
        })?;
    } else {
        run_git(
            cwd,
            &["worktree", "add", "--", &worktree_path_str, branch_name],
        )
        .map_err(|e| {
            // Branch pre-existed; clean only the partial worktree, never the branch.
            cleanup_failed_worktree(cwd, &worktree_path, None);
            GitOpError::Git(format!(
                "Failed to create worktree for branch '{branch_name}': {e}"
            ))
        })?;
    }

    prewarm_new_worktree(cwd, &worktree_path);

    // 3. Stage `.phoenix/` into the tracked `.gitignore` for StageGitignore.
    //    LocalExclude already wrote its exclude entry BEFORE the add (above), so
    //    `.phoenix/` is ignored even if the add failed; nothing to do here.
    if ignore_strategy == PhoenixIgnoreStrategy::StageGitignore {
        if let Err(e) = ensure_gitignore_has_phoenix(cwd) {
            tracing::warn!(error = %e, "Failed to ignore .phoenix/ (non-fatal)");
        }
    }

    Ok(worktree_path_str)
}

fn prewarm_new_worktree(source_root: &Path, worktree_path: &Path) {
    crate::project_opportunistic_build_warm::prewarm_project_build_caches(
        source_root,
        worktree_path,
    );

    #[cfg(test)]
    TEST_PREWARM_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook(source_root, worktree_path);
        }
    });
}

#[cfg(test)]
type PrewarmHook = dyn Fn(&Path, &Path);

#[cfg(test)]
thread_local! {
    static TEST_PREWARM_HOOK: std::cell::RefCell<Option<Box<PrewarmHook>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn with_test_prewarm_hook<R>(hook: impl Fn(&Path, &Path) + 'static, f: impl FnOnce() -> R) -> R {
    TEST_PREWARM_HOOK.with(|slot| {
        let previous = slot.replace(Some(Box::new(hook)));
        let result = f();
        slot.replace(previous);
        result
    })
}

/// Best-effort rollback of a partially-created worktree after `git worktree add`
/// fails. Every step is non-fatal: the original add error is what gets surfaced;
/// these cleanups only stop the failure from leaving artifacts that poison a retry.
///
/// `created_branch` is `Some(name)` only when the caller passed `-b` and thus owns
/// the branch ref. In the checkout-existing-branch path it is `None` — deleting a
/// pre-existing branch would be data loss.
fn cleanup_failed_worktree(cwd: &Path, worktree_path: &Path, created_branch: Option<&str>) {
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    // 1. Drop any admin entry + partial dir git registered before it bailed.
    if let Err(e) = run_git(cwd, &["worktree", "remove", &worktree_path_str, "--force"]) {
        tracing::debug!(
            error = %e,
            worktree = %worktree_path_str,
            "worktree remove during failed-add cleanup failed; trying fs + prune"
        );
        if worktree_path.exists() {
            if let Err(rm_err) = std::fs::remove_dir_all(worktree_path) {
                tracing::debug!(error = %rm_err, "fs cleanup of partial worktree failed");
            }
        }
        let _ = run_git(cwd, &["worktree", "prune"]);
    }

    // 2. Delete the branch we created, but only if it isn't checked out elsewhere
    //    (defensive: a failed add means it shouldn't be, but never move a ref a
    //    worktree owns — see AGENTS.md worktree-safety rule).
    if let Some(branch) = created_branch {
        if find_branch_in_worktree_list(cwd, branch).is_some() {
            tracing::debug!(branch, "leaked-branch cleanup skipped: still checked out");
            return;
        }
        if let Err(e) = run_git(cwd, &["branch", "-D", branch]) {
            tracing::debug!(
                error = %e,
                branch,
                "leaked-branch cleanup failed (non-fatal)"
            );
        }
    }
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

/// If `path` is inside a Phoenix worktree at `{repo_root}/.phoenix/worktrees/{id}`,
/// returns `Some(repo_root)`.  Returns `None` otherwise — the input is not a
/// Phoenix worktree path.
///
/// Strict match: requires the `.phoenix/worktrees/{id}` segment, not just a
/// `.phoenix` ancestor.  `{repo_root}/src/...` returns `None`, even though it
/// is inside the repo, because there is no Phoenix worktree above it.
/// `{repo_root}/.phoenix/foo` (a non-worktree subdir of `.phoenix`) also returns
/// `None`.
///
/// Callers that want the input path as a fallback when the predicate fails
/// (e.g., Direct-mode conversations whose `working_dir` IS the repo root)
/// should use `.unwrap_or_else(|| path.to_path_buf())` at the call site so the
/// fallback is explicit rather than hidden in this helper.
pub(crate) fn repo_root_from_phoenix_worktree(path: &Path) -> Option<std::path::PathBuf> {
    // Walk up looking for `{repo_root}/.phoenix/worktrees/{id}` shape: an
    // ancestor whose parent.file_name == "worktrees" and grandparent.file_name
    // == ".phoenix".  Returning the great-grandparent gives `{repo_root}`.
    path.ancestors()
        .find(|p| {
            p.parent().is_some_and(|parent| {
                parent.file_name().is_some_and(|n| n == "worktrees")
                    && parent
                        .parent()
                        .is_some_and(|gp| gp.file_name().is_some_and(|n| n == ".phoenix"))
            })
        })
        .and_then(|conv_dir| conv_dir.parent()?.parent()?.parent())
        .map(std::path::Path::to_path_buf)
}

/// True when `git check-ignore` reports `.phoenix/` already ignored in `dir` —
/// by `.git/info/exclude`, a parent `.gitignore`, or any other mechanism. The
/// `-q` form exits 0 when ignored, 1 when not, and >1 on error; we treat only a
/// clean exit-0 as "already ignored" (an error/missing-git resolves to false so
/// the caller falls back to its append+stage path).
///
/// The query path carries a trailing slash (`.phoenix/`): the canonical exclude
/// pattern `.phoenix/` is directory-only, and git matches a directory-only
/// pattern against a pathname only when the path is known to be a directory —
/// either it exists on disk OR the queried path itself ends in `/`. Querying the
/// bare `.phoenix` would spuriously report "not ignored" whenever the `.phoenix`
/// directory does not yet exist.
fn phoenix_is_already_ignored(dir: &Path) -> bool {
    std::process::Command::new("git")
        .args(["check-ignore", "-q", ".phoenix/"])
        .current_dir(dir)
        .status()
        .is_ok_and(|status| status.success())
}

/// Ensure `.phoenix/` is ignored in the given directory, appending + staging the
/// tracked `.gitignore` only when it is NOT already ignored by some other
/// mechanism. Creates `.gitignore` if it doesn't exist.
///
/// No-op when `git check-ignore` already reports `.phoenix/` ignored — e.g. a
/// fork/refinement worktree created with `PhoenixIgnoreStrategy::LocalExclude`
/// shares the main repo's `.git/info/exclude`, so `.phoenix/` is already covered.
/// Re-staging the tracked `.gitignore` there would sweep an unrelated `.gitignore`
/// edit onto the task branch when the refinement is later approved (Explore→Work).
pub(crate) fn ensure_gitignore_has_phoenix(dir: &Path) -> Result<(), String> {
    use std::io::Write as _;

    // Already covered by `.git/info/exclude` (the fork strategy's shared
    // exclude), a parent `.gitignore`, or anything else → touch nothing: no
    // append, no staging.
    if phoenix_is_already_ignored(dir) {
        return Ok(());
    }

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

/// Ensure `.phoenix/` is ignored via the repo's LOCAL untracked exclude file
/// (`<git-common-dir>/info/exclude`), staging nothing and never touching the
/// tracked `.gitignore`. Used by fork resolution so cutting a fork's worktree
/// leaves the originating checkout's index and tracked files unaffected
/// (REQ-PROJ-035).
///
/// `dir` may be the main checkout or a linked worktree; the exclude file lives
/// in the MAIN repository's git dir, so the real common dir is resolved via
/// `git rev-parse --git-common-dir` rather than assuming `dir/.git` is a
/// directory (in a linked worktree `.git` is a file pointing elsewhere).
pub(crate) fn ensure_local_exclude_has_phoenix(dir: &Path) -> Result<(), String> {
    use std::io::Write as _;

    let common_dir = run_git(dir, &["rev-parse", "--git-common-dir"])?;
    let common_dir = {
        let p = Path::new(common_dir.trim());
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            dir.join(p)
        }
    };

    let info_dir = common_dir.join("info");
    let exclude_path = info_dir.join("exclude");

    let already_present = exclude_path.exists()
        && std::fs::read_to_string(&exclude_path)
            .map_err(|e| format!("Failed to read git exclude file: {e}"))?
            .lines()
            .any(|line| line.trim() == ".phoenix/");
    if already_present {
        return Ok(());
    }

    std::fs::create_dir_all(&info_dir)
        .map_err(|e| format!("Failed to create git info dir: {e}"))?;
    let needs_leading_newline = exclude_path.exists()
        && std::fs::read(&exclude_path)
            .ok()
            .is_some_and(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
        .map_err(|e| format!("Failed to open git exclude file: {e}"))?;
    if needs_leading_newline {
        writeln!(f).map_err(|e| format!("Failed to write git exclude file: {e}"))?;
    }
    writeln!(f, ".phoenix/").map_err(|e| format!("Failed to write git exclude file: {e}"))?;
    tracing::info!(dir = %dir.display(), "Added .phoenix/ to .git/info/exclude");

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

    fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
        std::fs::write(dir.join(name), content).unwrap();
        run_git(dir, &["add", name]).unwrap();
        run_git(dir, &["commit", "-q", "-m", message]).unwrap();
    }

    fn clone_repo(source: &Path, dest: &Path) {
        run_git(
            std::env::current_dir().unwrap().as_path(),
            &[
                "clone",
                "--quiet",
                source.to_str().unwrap(),
                dest.to_str().unwrap(),
            ],
        )
        .unwrap();
        run_git(dest, &["config", "user.email", "probe@test"]).unwrap();
        run_git(dest, &["config", "user.name", "probe"]).unwrap();
    }

    #[test]
    fn repo_root_from_phoenix_worktree_matches_canonical_shape() {
        let root = Path::new("/repo");
        let wt = Path::new("/repo/.phoenix/worktrees/abc123");
        assert_eq!(
            repo_root_from_phoenix_worktree(wt),
            Some(root.to_path_buf())
        );
    }

    #[test]
    fn repo_root_from_phoenix_worktree_matches_descendant_of_worktree() {
        let root = Path::new("/repo");
        let inside = Path::new("/repo/.phoenix/worktrees/abc123/src/main.rs");
        assert_eq!(
            repo_root_from_phoenix_worktree(inside),
            Some(root.to_path_buf())
        );
    }

    #[test]
    fn repo_root_from_phoenix_worktree_rejects_repo_root_itself() {
        // A plain repo root has no `.phoenix/worktrees/{id}` ancestor.
        assert_eq!(repo_root_from_phoenix_worktree(Path::new("/repo")), None);
    }

    #[test]
    fn repo_root_from_phoenix_worktree_rejects_arbitrary_repo_subdir() {
        // {repo_root}/src is inside the repo but not a Phoenix worktree —
        // the original looser predicate would have returned `/repo/src`
        // unchanged, which was the bug Copilot flagged.
        assert_eq!(
            repo_root_from_phoenix_worktree(Path::new("/repo/src")),
            None
        );
    }

    #[test]
    fn repo_root_from_phoenix_worktree_rejects_non_worktree_phoenix_subdir() {
        // {repo_root}/.phoenix/foo is under .phoenix but not a worktree —
        // the original predicate (any `.phoenix` ancestor) would have
        // claimed this is a worktree, which it isn't.
        assert_eq!(
            repo_root_from_phoenix_worktree(Path::new("/repo/.phoenix/foo")),
            None
        );
        assert_eq!(
            repo_root_from_phoenix_worktree(Path::new("/repo/.phoenix")),
            None
        );
    }

    #[test]
    fn repo_root_from_phoenix_worktree_rejects_paths_outside_any_repo() {
        assert_eq!(repo_root_from_phoenix_worktree(Path::new("/tmp/xyz")), None);
        assert_eq!(repo_root_from_phoenix_worktree(Path::new("relative")), None);
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
    fn materialize_branch_does_not_move_branch_checked_out_in_main_worktree() {
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());

        let clone = TempDir::new().unwrap();
        clone_repo(upstream.path(), clone.path());
        let original_main = run_git(clone.path(), &["rev-parse", "main"]).unwrap();

        run_git(clone.path(), &["branch", "task-pending-test", "main"]).unwrap();
        let pending_worktree = TempDir::new().unwrap();
        run_git(
            clone.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                pending_worktree.path().to_str().unwrap(),
                "task-pending-test",
            ],
        )
        .unwrap();

        commit_file(
            upstream.path(),
            "upstream.txt",
            "new upstream content\n",
            "advance main",
        );
        let advanced_main = run_git(upstream.path(), &["rev-parse", "main"]).unwrap();

        materialize_branch(pending_worktree.path(), "main").unwrap();

        assert_eq!(
            run_git(clone.path(), &["rev-parse", "origin/main"]).unwrap(),
            advanced_main
        );
        assert_eq!(
            run_git(clone.path(), &["rev-parse", "main"]).unwrap(),
            original_main
        );
        assert_eq!(
            run_git(clone.path(), &["status", "--porcelain"]).unwrap(),
            ""
        );
    }

    #[test]
    fn materialize_branch_does_not_move_branch_checked_out_in_other_worktree() {
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());
        run_git(upstream.path(), &["checkout", "-q", "-b", "feature"]).unwrap();
        commit_file(
            upstream.path(),
            "feature.txt",
            "old feature content\n",
            "create feature",
        );
        let original_feature = run_git(upstream.path(), &["rev-parse", "feature"]).unwrap();
        run_git(upstream.path(), &["checkout", "-q", "main"]).unwrap();

        let clone = TempDir::new().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["branch", "feature", &original_feature]).unwrap();
        let feature_worktree = TempDir::new().unwrap();
        run_git(
            clone.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                feature_worktree.path().to_str().unwrap(),
                "feature",
            ],
        )
        .unwrap();

        run_git(upstream.path(), &["checkout", "-q", "feature"]).unwrap();
        commit_file(
            upstream.path(),
            "feature.txt",
            "new feature content\n",
            "advance feature",
        );
        let advanced_feature = run_git(upstream.path(), &["rev-parse", "feature"]).unwrap();

        materialize_branch(clone.path(), "feature").unwrap();

        assert_eq!(
            run_git(clone.path(), &["rev-parse", "origin/feature"]).unwrap(),
            advanced_feature
        );
        assert_eq!(
            run_git(clone.path(), &["rev-parse", "feature"]).unwrap(),
            original_feature
        );
        assert_eq!(
            run_git(feature_worktree.path(), &["status", "--porcelain"]).unwrap(),
            ""
        );
    }

    fn branch_exists(dir: &Path, branch: &str) -> bool {
        run_git(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }

    #[test]
    fn create_worktree_invokes_prewarm_after_successful_add() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "tracked.txt", "content\n", "add tracked file");

        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let calls_for_hook = std::rc::Rc::clone(&calls);
        let conv_id = "feedface-2222-2222-2222-222222222222";

        let result = with_test_prewarm_hook(
            move |source_root, worktree_path| {
                calls_for_hook
                    .borrow_mut()
                    .push((source_root.to_path_buf(), worktree_path.to_path_buf()));
            },
            || {
                create_worktree(
                    tmp.path(),
                    conv_id,
                    "task-pending-feedface",
                    Some("main"),
                    PhoenixIgnoreStrategy::StageGitignore,
                )
            },
        )
        .unwrap();

        let expected_worktree = tmp.path().join(".phoenix/worktrees").join(conv_id);
        assert_eq!(Path::new(&result), expected_worktree.as_path());
        assert!(expected_worktree.join("tracked.txt").exists());
        assert_eq!(
            calls.borrow().as_slice(),
            &[(tmp.path().to_path_buf(), expected_worktree)]
        );
    }

    #[test]
    fn create_worktree_still_succeeds_when_prewarm_source_cache_exists() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "tracked.txt", "content\n", "add tracked file");
        std::fs::create_dir_all(tmp.path().join("target/debug")).unwrap();
        std::fs::write(tmp.path().join("target/debug/cache.txt"), "warm\n").unwrap();

        let conv_id = "feedface-3333-3333-3333-333333333333";
        let result = create_worktree(
            tmp.path(),
            conv_id,
            "task-pending-warm",
            Some("main"),
            PhoenixIgnoreStrategy::StageGitignore,
        )
        .unwrap();

        assert!(Path::new(&result).join("tracked.txt").exists());
    }

    #[test]
    fn cleanup_failed_worktree_deletes_branch_we_created() {
        // Models the leak: `git worktree add -b` made the branch, then checkout
        // failed and git rolled back the dir/admin entry. The branch must go.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "task-pending-deadbeef", "main"]).unwrap();
        assert!(branch_exists(tmp.path(), "task-pending-deadbeef"));

        let phantom = tmp.path().join(".phoenix/worktrees/deadbeef");
        cleanup_failed_worktree(tmp.path(), &phantom, Some("task-pending-deadbeef"));

        assert!(!branch_exists(tmp.path(), "task-pending-deadbeef"));
    }

    #[test]
    fn cleanup_failed_worktree_preserves_preexisting_branch() {
        // Checkout-existing-branch path passes None: the branch predates us and
        // must survive a failed add — deleting it would be data loss.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "existing-feature", "main"]).unwrap();

        let phantom = tmp.path().join(".phoenix/worktrees/cafe");
        cleanup_failed_worktree(tmp.path(), &phantom, None);

        assert!(branch_exists(tmp.path(), "existing-feature"));
    }

    #[test]
    fn cleanup_failed_worktree_spares_branch_checked_out_elsewhere() {
        // Safety guard: never delete a ref a live worktree owns, even if asked.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "task-pending-live", "main"]).unwrap();
        let live_wt = TempDir::new().unwrap();
        run_git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                live_wt.path().to_str().unwrap(),
                "task-pending-live",
            ],
        )
        .unwrap();

        let phantom = tmp.path().join(".phoenix/worktrees/other");
        cleanup_failed_worktree(tmp.path(), &phantom, Some("task-pending-live"));

        assert!(branch_exists(tmp.path(), "task-pending-live"));
    }

    #[test]
    fn create_worktree_failure_leaves_no_leaked_branch() {
        // End-to-end: force `git worktree add -b` to fail by pre-creating a
        // colliding child path, then assert no `task-pending-*` ref survives.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "tracked.txt", "content\n", "add tracked file");

        // Occupy the worktree target with a non-empty dir so `add` aborts.
        let conv_id = "feedface-0000-0000-0000-000000000000";
        let target = tmp.path().join(".phoenix/worktrees").join(conv_id);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("squatter"), "blocks the add\n").unwrap();

        let res = create_worktree(
            tmp.path(),
            conv_id,
            "task-pending-feedface",
            Some("main"),
            PhoenixIgnoreStrategy::StageGitignore,
        );
        assert!(res.is_err(), "add into a non-empty dir should fail");

        let branches = run_git(tmp.path(), &["branch", "--list", "task-pending-*"]).unwrap();
        assert_eq!(
            branches.trim(),
            "",
            "no task-pending branch may leak: {branches:?}"
        );
    }

    #[test]
    fn local_exclude_fork_failure_leaves_no_untracked_phoenix_in_origin() {
        // F2: for the LocalExclude (fork) strategy, the `.git/info/exclude` entry
        // is written BEFORE `git worktree add`. So even when the add FAILS (here,
        // the deterministic target path is pre-occupied by a non-empty dir), the
        // partial `.phoenix/` dir is already ignored and never surfaces as
        // `?? .phoenix/` in the origin checkout — the decoupling forks must keep.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "tracked.txt", "content\n", "add tracked file");

        let conv_id = "feedface-1111-1111-1111-111111111111";
        let target = tmp.path().join(".phoenix/worktrees").join(conv_id);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("squatter"), "blocks the add\n").unwrap();

        let res = create_worktree(
            tmp.path(),
            conv_id,
            "task-pending-feedface",
            Some("main"),
            PhoenixIgnoreStrategy::LocalExclude,
        );
        assert!(res.is_err(), "add into a non-empty dir should fail");

        // The exclude entry was written first, so `.phoenix/` is ignored.
        let exclude =
            std::fs::read_to_string(tmp.path().join(".git/info/exclude")).unwrap_or_default();
        assert!(
            exclude.lines().any(|l| l.trim() == ".phoenix/"),
            "`.phoenix/` must be in .git/info/exclude even on a failed add: {exclude:?}"
        );

        // The origin status shows NO untracked `.phoenix/` (and nothing staged).
        let status = run_git(tmp.path(), &["status", "--porcelain"]).unwrap();
        assert!(
            !status.contains(".phoenix"),
            "a failed fork worktree add must not leave `?? .phoenix/` in the origin: {status:?}"
        );
    }

    #[test]
    fn run_git_capped_passthrough_when_under_cap() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // `git status --porcelain` on a fresh repo is empty stdout.
        let out =
            run_git_capped(tmp.path(), &["status", "--porcelain"], &[], 1024, 8 * 1024).unwrap();
        assert_eq!(out.stdout, "");
        assert_eq!(out.total_bytes, 0);
        assert!(!out.saturated);
    }

    #[test]
    fn run_git_capped_truncates_at_max_bytes_and_counts_total() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // Stage a 4 KiB blob so `git diff --cached` produces a sizable
        // stdout. Cap reads at 256 bytes; stream should keep counting.
        let payload = "abcdefghijklmnop\n".repeat(256); // 4352 bytes
        std::fs::write(tmp.path().join("big.txt"), &payload).unwrap();
        run_git(tmp.path(), &["add", "big.txt"]).unwrap();

        let out = run_git_capped(tmp.path(), &["diff", "--cached"], &[], 256, 64 * 1024).unwrap();
        assert!(out.stdout.len() <= 256, "stdout should be capped");
        assert!(
            out.total_bytes > 256,
            "total_bytes should reflect the full stream",
        );
        assert!(!out.saturated, "should not saturate under hard limit");
    }

    #[test]
    fn run_git_capped_saturates_past_hard_limit() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // Make the staged blob bigger than the hard limit.
        let payload = "x".repeat(8 * 1024); // 8 KiB raw → diff is bigger
        std::fs::write(tmp.path().join("huge.txt"), &payload).unwrap();
        run_git(tmp.path(), &["add", "huge.txt"]).unwrap();

        // Cap memory at 128 bytes, hard limit at 1 KiB. Expect saturation.
        let out = run_git_capped(tmp.path(), &["diff", "--cached"], &[], 128, 1024).unwrap();
        assert!(out.stdout.len() <= 128);
        assert!(
            out.saturated,
            "should saturate when stream exceeds hard limit"
        );
        assert!(out.total_bytes >= 1024);
    }

    #[test]
    fn utf8_floor_boundary_passthrough_for_ascii() {
        let buf = b"abcdefgh";
        assert_eq!(utf8_floor_boundary(buf, 5), 5);
        assert_eq!(utf8_floor_boundary(buf, 0), 0);
        assert_eq!(utf8_floor_boundary(buf, buf.len()), buf.len());
    }

    #[test]
    fn utf8_floor_boundary_clamps_past_end() {
        let buf = b"abc";
        assert_eq!(utf8_floor_boundary(buf, 100), 3);
    }

    #[test]
    fn utf8_floor_boundary_retreats_before_partial_multibyte() {
        // 'é' is 0xC3 0xA9 (2 bytes). "aé" = [0x61, 0xC3, 0xA9].
        // Cutting at byte 2 splits the é mid-sequence; helper retreats
        // to byte 1 so the prefix is valid UTF-8.
        let buf = "aé".as_bytes();
        assert_eq!(buf, &[0x61, 0xC3, 0xA9]);
        assert_eq!(utf8_floor_boundary(buf, 2), 1);
        // Cutting at byte 3 (full sequence) is fine.
        assert_eq!(utf8_floor_boundary(buf, 3), 3);
    }

    #[test]
    fn utf8_floor_boundary_handles_4_byte_sequences() {
        // 😀 (U+1F600) is 4 bytes: F0 9F 98 80
        let buf = "x😀y".as_bytes();
        // x = 0x78. After 'x' is byte 1.
        // 😀 occupies bytes 1..5. y starts at byte 5.
        assert_eq!(utf8_floor_boundary(buf, 1), 1); // before emoji
        assert_eq!(utf8_floor_boundary(buf, 2), 1); // mid-emoji byte 1 → retreat
        assert_eq!(utf8_floor_boundary(buf, 3), 1); // mid-emoji byte 2 → retreat
        assert_eq!(utf8_floor_boundary(buf, 4), 1); // mid-emoji byte 3 → retreat
        assert_eq!(utf8_floor_boundary(buf, 5), 5); // after emoji
                                                    // The retreat is bounded — only walks back at most 4 bytes for
                                                    // the longest possible UTF-8 sequence.
    }

    #[test]
    fn utf8_floor_boundary_result_is_always_valid_utf8() {
        // Property check: for any cut point on a UTF-8 string's byte
        // representation, the helper returns an offset where bytes[..offset]
        // is valid UTF-8.
        let texts = ["hello", "café", "😀😃😄", "mixed: x→y", "", "a"];
        for text in texts {
            let buf = text.as_bytes();
            for end in 0..=buf.len() + 2 {
                let cut = utf8_floor_boundary(buf, end);
                assert!(
                    std::str::from_utf8(&buf[..cut]).is_ok(),
                    "bytes[..{cut}] of {text:?} should be valid UTF-8 (end={end})"
                );
            }
        }
    }

    #[test]
    fn capture_branch_diff_does_not_mutate_real_index() {
        // Regression for review: capture_branch_diff used to run `git add -N .`
        // directly, mutating the worktree index. This test creates an
        // untracked file, runs the capture, and asserts the real index is
        // byte-for-byte unchanged.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // Create an untracked file in the worktree.
        std::fs::write(tmp.path().join("untracked.txt"), "hello\n").unwrap();

        let git_dir = run_git(tmp.path(), &["rev-parse", "--git-dir"]).unwrap();
        let git_dir = {
            let p = std::path::Path::new(git_dir.trim());
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                tmp.path().join(p)
            }
        };
        let index_path = git_dir.join("index");
        let before = if index_path.exists() {
            std::fs::read(&index_path).ok()
        } else {
            None
        };

        let captured = capture_branch_diff(tmp.path(), "main", 100 * 1024);
        // Sanity: the untracked file showed up in the diff (so add -N
        // actually ran against the temp index).
        assert!(
            captured.uncommitted_diff.contains("untracked.txt"),
            "expected untracked file to surface in diff: {}",
            captured.uncommitted_diff
        );

        let after = if index_path.exists() {
            std::fs::read(&index_path).ok()
        } else {
            None
        };
        assert_eq!(before, after, "real .git/index must not be mutated");
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

    /// Bug 2: when `.phoenix/` is ALREADY ignored (here via `.git/info/exclude`,
    /// the shared exclude a fork/refinement worktree inherits), a later
    /// Explore→Work approval that calls `ensure_gitignore_has_phoenix` must NOT
    /// touch the tracked `.gitignore` — no append, no `git add`. Otherwise an
    /// unrelated `.gitignore` edit is swept onto the task branch.
    #[test]
    fn ensure_gitignore_is_noop_when_phoenix_already_ignored_via_exclude() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // Ignore `.phoenix/` via the local exclude (the fork strategy's path),
        // not via the tracked `.gitignore`.
        let info_dir = tmp.path().join(".git/info");
        std::fs::create_dir_all(&info_dir).unwrap();
        std::fs::write(info_dir.join("exclude"), ".phoenix/\n").unwrap();

        // Stage an UNRELATED edit to a tracked `.gitignore` (not yet committed),
        // to detect any accidental re-staging by the call below.
        std::fs::write(tmp.path().join(".gitignore"), "*.log\n").unwrap();
        run_git(tmp.path(), &["add", ".gitignore"]).unwrap();
        run_git(tmp.path(), &["commit", "-q", "-m", "add gitignore"]).unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "*.log\nbuild/\n").unwrap();
        let before = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();

        ensure_gitignore_has_phoenix(tmp.path()).unwrap();

        // Content unchanged: no `.phoenix/` appended.
        let after = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert_eq!(
            before, after,
            "`.gitignore` must not be modified when `.phoenix/` is already ignored"
        );
        assert!(
            !after.contains(".phoenix"),
            "no `.phoenix/` line should be appended: {after:?}"
        );
        // The unrelated working-tree edit must NOT have been staged.
        let staged = run_git(tmp.path(), &["diff", "--cached", "--name-only"]).unwrap();
        assert!(
            staged.trim().is_empty(),
            "nothing should be staged (the unrelated .gitignore edit must not be swept): {staged:?}"
        );
    }

    /// Bug 2 complement: when `.phoenix/` is NOT yet ignored (a plain
    /// Explore/Branch worktree), `ensure_gitignore_has_phoenix` keeps its
    /// append + stage behaviour.
    #[test]
    fn ensure_gitignore_appends_and_stages_when_not_yet_ignored() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());

        ensure_gitignore_has_phoenix(tmp.path()).unwrap();

        let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(
            content.lines().any(|l| l.trim() == ".phoenix/"),
            "`.phoenix/` must be appended: {content:?}"
        );
        let staged = run_git(tmp.path(), &["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(
            staged.trim(),
            ".gitignore",
            "the new `.gitignore` must be staged: {staged:?}"
        );
    }

    // ---- find_branch_in_worktree_list: porcelain parsing ----

    #[test]
    fn find_branch_in_worktree_list_returns_none_for_free_branch() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "feature", "main"]).unwrap();
        // `feature` exists but is checked out in no worktree.
        assert_eq!(find_branch_in_worktree_list(tmp.path(), "feature"), None);
    }

    #[test]
    fn find_branch_in_worktree_list_finds_primary_worktree_branch() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let found = find_branch_in_worktree_list(tmp.path(), "main")
            .expect("main is checked out in the primary worktree");
        assert_eq!(
            std::fs::canonicalize(found).unwrap(),
            std::fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[test]
    fn find_branch_in_worktree_list_finds_linked_worktree_branch() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "feature", "main"]).unwrap();
        let wt = TempDir::new().unwrap();
        run_git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                wt.path().to_str().unwrap(),
                "feature",
            ],
        )
        .unwrap();
        let found = find_branch_in_worktree_list(tmp.path(), "feature")
            .expect("feature is checked out in the linked worktree");
        assert_eq!(
            std::fs::canonicalize(found).unwrap(),
            std::fs::canonicalize(wt.path()).unwrap()
        );
    }

    #[test]
    fn find_branch_in_worktree_list_requires_exact_ref_match() {
        // A shared prefix must not be mistaken for a checkout: `feature` is
        // checked out, `feature-2` is free. The match is on the full
        // `refs/heads/<name>` line, so prefixes do not collide.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "feature", "main"]).unwrap();
        let wt = TempDir::new().unwrap();
        run_git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                wt.path().to_str().unwrap(),
                "feature",
            ],
        )
        .unwrap();
        assert_eq!(find_branch_in_worktree_list(tmp.path(), "feature-2"), None);
    }

    // ---- check_branch_conflict: the branch-collision gate ----
    //
    // `check_branch_conflict` resolves a Phoenix-owner lookup via
    // `Handle::block_on`, so it must run on a blocking thread, not an async
    // worker — hence `spawn_blocking` under a multi-thread runtime.

    #[tokio::test(flavor = "multi_thread")]
    async fn check_branch_conflict_ok_when_branch_is_free() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "feature", "main"]).unwrap();
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let cwd = tmp.path().to_path_buf();

        let result =
            tokio::task::spawn_blocking(move || check_branch_conflict(&cwd, &db, "feature"))
                .await
                .unwrap();
        assert!(
            result.is_ok(),
            "a free branch must not conflict: {result:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn check_branch_conflict_flags_external_checkout() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        run_git(tmp.path(), &["branch", "feature", "main"]).unwrap();
        let wt = TempDir::new().unwrap();
        run_git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                wt.path().to_str().unwrap(),
                "feature",
            ],
        )
        .unwrap();
        // Empty DB: no Phoenix conversation owns the branch, so a checkout that
        // exists on disk must classify as an external (non-Phoenix) checkout.
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let cwd = tmp.path().to_path_buf();

        let result =
            tokio::task::spawn_blocking(move || check_branch_conflict(&cwd, &db, "feature"))
                .await
                .unwrap();
        match result {
            Err(BranchConflict::ExternalCheckout { branch, .. }) => assert_eq!(branch, "feature"),
            other => panic!("expected ExternalCheckout, got {other:?}"),
        }
    }

    // ---- materialize_branch: local/remote reconciliation ----

    #[test]
    fn materialize_branch_keeps_local_when_diverged_from_remote() {
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());
        let clone = TempDir::new().unwrap();
        clone_repo(upstream.path(), clone.path());

        // Local `feature` gains its own commit.
        run_git(
            clone.path(),
            &["checkout", "--quiet", "-b", "feature", "main"],
        )
        .unwrap();
        commit_file(clone.path(), "local.txt", "L", "local-only");
        let local_sha = run_git(clone.path(), &["rev-parse", "feature"]).unwrap();
        run_git(clone.path(), &["checkout", "--quiet", "main"]).unwrap();

        // Upstream `feature` gains a *different* commit → the two diverge.
        run_git(
            upstream.path(),
            &["checkout", "--quiet", "-b", "feature", "main"],
        )
        .unwrap();
        commit_file(upstream.path(), "remote.txt", "R", "remote-only");
        run_git(upstream.path(), &["checkout", "--quiet", "main"]).unwrap();

        materialize_branch(clone.path(), "feature").unwrap();

        // Diverged history must never be force-moved; local ref stays put.
        let after = run_git(clone.path(), &["rev-parse", "feature"]).unwrap();
        assert_eq!(
            after.trim(),
            local_sha.trim(),
            "a diverged local branch must not be moved"
        );
    }

    #[test]
    fn materialize_branch_fast_forwards_local_to_remote_tip() {
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());
        let clone = TempDir::new().unwrap();
        clone_repo(upstream.path(), clone.path());

        // Local `feature` sits at `main`; upstream `feature` is one commit ahead.
        run_git(clone.path(), &["branch", "feature", "main"]).unwrap();
        run_git(
            upstream.path(),
            &["checkout", "--quiet", "-b", "feature", "main"],
        )
        .unwrap();
        commit_file(upstream.path(), "ahead.txt", "R", "remote ahead");
        let remote_sha = run_git(upstream.path(), &["rev-parse", "feature"]).unwrap();
        run_git(upstream.path(), &["checkout", "--quiet", "main"]).unwrap();

        materialize_branch(clone.path(), "feature").unwrap();

        // `feature` is checked out nowhere, so it fast-forwards to the remote tip.
        let after = run_git(clone.path(), &["rev-parse", "feature"]).unwrap();
        assert_eq!(
            after.trim(),
            remote_sha.trim(),
            "an ancestor local branch must fast-forward to the remote tip"
        );
    }

    #[test]
    fn materialize_branch_creates_local_tracking_branch_when_remote_only() {
        let upstream = TempDir::new().unwrap();
        init_repo(upstream.path());
        run_git(
            upstream.path(),
            &["checkout", "--quiet", "-b", "feature", "main"],
        )
        .unwrap();
        commit_file(upstream.path(), "r.txt", "R", "remote feature");
        run_git(upstream.path(), &["checkout", "--quiet", "main"]).unwrap();

        let clone = TempDir::new().unwrap();
        clone_repo(upstream.path(), clone.path());
        assert!(
            run_git(clone.path(), &["rev-parse", "--verify", "feature"]).is_err(),
            "precondition: clone has no local `feature`"
        );

        materialize_branch(clone.path(), "feature").unwrap();

        assert!(
            run_git(clone.path(), &["rev-parse", "--verify", "feature"]).is_ok(),
            "a remote-only branch must be materialized as a local tracking branch"
        );
    }

    #[test]
    fn materialize_branch_errors_when_branch_absent_everywhere() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let err = materialize_branch(tmp.path(), "ghost").unwrap_err();
        assert!(
            matches!(err, GitOpError::BranchNotFound(ref b) if b == "ghost"),
            "got {err:?}"
        );
    }

    // ---- ensure_local_exclude_has_phoenix: .git/info/exclude mutation ----

    #[test]
    fn ensure_local_exclude_appends_phoenix_when_absent() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        ensure_local_exclude_has_phoenix(tmp.path()).unwrap();
        let exclude = std::fs::read_to_string(tmp.path().join(".git/info/exclude")).unwrap();
        assert!(
            exclude.lines().any(|l| l.trim() == ".phoenix/"),
            "exclude was {exclude:?}"
        );
    }

    #[test]
    fn ensure_local_exclude_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        ensure_local_exclude_has_phoenix(tmp.path()).unwrap();
        ensure_local_exclude_has_phoenix(tmp.path()).unwrap();
        let exclude = std::fs::read_to_string(tmp.path().join(".git/info/exclude")).unwrap();
        let count = exclude.lines().filter(|l| l.trim() == ".phoenix/").count();
        assert_eq!(count, 1, "the entry must not be duplicated: {exclude:?}");
    }

    #[test]
    fn ensure_local_exclude_separates_from_unterminated_existing_content() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let exclude_path = tmp.path().join(".git/info/exclude");
        std::fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        // Pre-existing entry with NO trailing newline.
        std::fs::write(&exclude_path, "*.tmp").unwrap();

        ensure_local_exclude_has_phoenix(tmp.path()).unwrap();

        let exclude = std::fs::read_to_string(&exclude_path).unwrap();
        assert!(
            exclude.contains("*.tmp\n.phoenix/"),
            "entries must be newline-separated: {exclude:?}"
        );
        assert_eq!(
            exclude.lines().filter(|l| l.trim() == ".phoenix/").count(),
            1
        );
    }
}
