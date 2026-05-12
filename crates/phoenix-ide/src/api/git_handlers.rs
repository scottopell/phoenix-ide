//! Git-related HTTP handlers: branch listing, search, conflict detection,
//! per-conversation diff snapshots.

use super::handlers::AppError;
use super::types::{
    ConversationDiffResponse, GitBranchEntry, GitBranchesQuery, GitBranchesResponse, PrCheckState,
    PrDisplayState, PrStatusResponse, PrUnavailableReason,
};
use super::AppState;
use crate::db::ConvMode;
use crate::git_ops::{capture_branch_diff, run_git};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) async fn list_git_branches(
    State(state): State<AppState>,
    Query(params): Query<GitBranchesQuery>,
) -> Result<Json<GitBranchesResponse>, AppError> {
    let cwd = PathBuf::from(&params.cwd);
    if !cwd.is_dir() {
        return Err(AppError::BadRequest("Directory does not exist".to_string()));
    }

    // Build branch -> conversation slug conflict map from worktree list + DB.
    let conflict_map = build_branch_conflict_map(&state.db, &cwd).await;

    let search = params.search.clone();
    tokio::task::spawn_blocking(move || {
        let mut resp = if let Some(query) = search {
            search_remote_branches(&cwd, &query)?
        } else {
            list_local_branches(&cwd)?
        };
        // Annotate branches with conflict slugs.
        for branch in &mut resp.branches {
            branch.conflict_slug = conflict_map.get(&branch.name).cloned();
        }
        Ok(resp)
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?
    .map(Json)
}

/// Build a map of `branch_name` -> `conversation_slug` for branches that are
/// checked out in worktrees with active conversations.
async fn build_branch_conflict_map(
    db: &crate::db::Database,
    cwd: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();

    // Get checked-out branches from git worktree list.
    let checked_out: std::collections::HashMap<String, String> =
        run_git(cwd, &["worktree", "list", "--porcelain"])
            .map(|output| {
                let mut result = std::collections::HashMap::new();
                let mut current_path: Option<String> = None;
                for line in output.lines() {
                    if let Some(path) = line.strip_prefix("worktree ") {
                        current_path = Some(path.to_string());
                    } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                        if let Some(ref path) = current_path {
                            result.insert(branch.to_string(), path.clone());
                        }
                    } else if line.is_empty() {
                        current_path = None;
                    }
                }
                result
            })
            .unwrap_or_default();

    if checked_out.is_empty() {
        return map;
    }

    // Cross-reference with active conversations.
    let convs = db.get_work_conversations().await.unwrap_or_default();
    for conv in &convs {
        if conv.state.is_terminal() || conv.parent_conversation_id.is_some() {
            continue;
        }
        if let Some(branch) = conv.conv_mode.branch_name() {
            if checked_out.contains_key(branch) {
                if let Some(slug) = &conv.slug {
                    map.insert(branch.to_string(), slug.clone());
                }
            }
        }
    }

    map
}

/// REQ-PROJ-020: Local branches sorted by recency, no network.
fn list_local_branches(cwd: &std::path::Path) -> Result<GitBranchesResponse, AppError> {
    // Local branches sorted by most recent commit (descending).
    let local_output = run_git(
        cwd,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "refs/heads/",
            "--format=%(refname:short)",
        ],
    )
    .map_err(|e| AppError::Internal(format!("Failed to list branches: {e}")))?;

    let local_names: Vec<String> = local_output
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Build entries with behind-remote counts for tracked branches.
    let branches: Vec<GitBranchEntry> = local_names
        .into_iter()
        .map(|name| {
            let remote_ref = format!("origin/{name}");
            let has_remote = run_git(cwd, &["rev-parse", "--verify", &remote_ref]).is_ok();

            let behind_remote = if has_remote {
                let range = format!("{name}..{remote_ref}");
                run_git(cwd, &["rev-list", "--count", &range])
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .filter(|&n| n > 0)
            } else {
                None
            };

            GitBranchEntry {
                local: true,
                remote: has_remote,
                behind_remote,
                name,
                conflict_slug: None,
            }
        })
        .collect();

    let current_raw = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| AppError::Internal(format!("Failed to get current branch: {e}")))?
        .trim()
        .to_string();
    // Detached HEAD returns literal "HEAD" -- not a real branch name.
    let current = if current_raw == "HEAD" {
        String::new()
    } else {
        current_raw
    };

    // Detect remote default branch from cached symbolic ref (no network).
    let default_branch = run_git(cwd, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|s| {
            s.trim()
                .strip_prefix("refs/remotes/origin/")
                .map(String::from)
        });

    Ok(GitBranchesResponse {
        branches,
        current,
        default_branch,
    })
}

/// REQ-PROJ-021: Remote branch search via cached `git ls-remote`.
fn search_remote_branches(
    cwd: &std::path::Path,
    query: &str,
) -> Result<GitBranchesResponse, AppError> {
    let refs = ls_remote_cached(cwd)?;
    let query_lower = query.to_lowercase();

    // Local branch set for cross-referencing.
    let local_output =
        run_git(cwd, &["branch", "--list", "--format=%(refname:short)"]).unwrap_or_default();
    let local_set: std::collections::HashSet<String> = local_output
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Start with remote refs that match the query.
    let remote_set: std::collections::HashSet<&str> = refs.iter().map(String::as_str).collect();
    let mut branches: Vec<GitBranchEntry> = refs
        .iter()
        .filter(|name| name.to_lowercase().contains(&query_lower))
        .map(|name| {
            let local = local_set.contains(name.as_str());
            let behind_remote = if local {
                let remote_ref = format!("origin/{name}");
                let range = format!("{name}..{remote_ref}");
                run_git(cwd, &["rev-list", "--count", &range])
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .filter(|&n| n > 0)
            } else {
                None
            };
            GitBranchEntry {
                local,
                remote: true,
                behind_remote,
                name: name.clone(),
                conflict_slug: None,
            }
        })
        .collect();

    // Include local branches that match the query but aren't in ls-remote.
    // This catches branches like "main" that may not appear in --heads output.
    for local_name in &local_set {
        if local_name.to_lowercase().contains(&query_lower)
            && !remote_set.contains(local_name.as_str())
        {
            let remote_ref = format!("origin/{local_name}");
            let has_remote = run_git(cwd, &["rev-parse", "--verify", &remote_ref]).is_ok();
            let behind_remote = if has_remote {
                let range = format!("{local_name}..{remote_ref}");
                run_git(cwd, &["rev-list", "--count", &range])
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .filter(|&n| n > 0)
            } else {
                None
            };
            branches.push(GitBranchEntry {
                local: true,
                remote: has_remote,
                behind_remote,
                name: local_name.clone(),
                conflict_slug: None,
            });
        }
    }

    // Sort: exact match first, then prefix matches, then substring.
    // Within each tier, local branches first (you've used them), then alphabetical.
    branches.sort_by(|a, b| {
        let a_exact = a.name.to_lowercase() == query_lower;
        let b_exact = b.name.to_lowercase() == query_lower;
        let a_prefix = a.name.to_lowercase().starts_with(&query_lower);
        let b_prefix = b.name.to_lowercase().starts_with(&query_lower);
        b_exact
            .cmp(&a_exact)
            .then(b_prefix.cmp(&a_prefix))
            .then(b.local.cmp(&a.local))
            .then(a.name.cmp(&b.name))
    });

    let current_raw = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let current = if current_raw == "HEAD" {
        String::new()
    } else {
        current_raw
    };

    Ok(GitBranchesResponse {
        branches,
        current,
        default_branch: None,
    })
}

/// Cached `git ls-remote` results. Key: canonical repo path. Value: (refs, timestamp).
type LsRemoteCacheMap = std::collections::HashMap<PathBuf, (Vec<String>, std::time::Instant)>;
static LS_REMOTE_CACHE: std::sync::LazyLock<std::sync::Mutex<LsRemoteCacheMap>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(LsRemoteCacheMap::new()));

/// Cache TTL for ls-remote results.
const LS_REMOTE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Returns cached remote ref names, refreshing if expired or missing.
fn ls_remote_cached(cwd: &std::path::Path) -> Result<Vec<String>, AppError> {
    let repo_root = run_git(cwd, &["rev-parse", "--show-toplevel"])
        .map_or_else(|_| cwd.to_path_buf(), |s| PathBuf::from(s.trim()));

    // Check cache.
    {
        let cache = LS_REMOTE_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((refs, ts)) = cache.get(&repo_root) {
            if ts.elapsed() < LS_REMOTE_CACHE_TTL {
                return Ok(refs.clone());
            }
        }
    }

    // Cache miss or expired: run ls-remote.
    let output = run_git(cwd, &["ls-remote", "--heads", "--tags", "origin"])
        .map_err(|e| AppError::Internal(format!("git ls-remote failed: {e}")))?;

    let refs: Vec<String> = output
        .lines()
        .filter_map(|line| {
            let refname = line.split_whitespace().nth(1)?;
            // Skip dereferenced tag refs (e.g. refs/tags/v1.0^{})
            if refname.ends_with("^{}") {
                return None;
            }
            refname
                .strip_prefix("refs/heads/")
                .or_else(|| refname.strip_prefix("refs/tags/"))
                .map(String::from)
        })
        .collect();

    // Update cache.
    {
        let mut cache = LS_REMOTE_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(repo_root, (refs.clone(), std::time::Instant::now()));
    }

    Ok(refs)
}

pub(crate) async fn get_conversation_pr_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PrStatusResponse>, AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let (branch_name, cwd) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            ..
        }
        | ConvMode::Branch {
            branch_name,
            worktree_path,
            ..
        } => (branch_name.to_string(), worktree_path.to_string()),
        _ => {
            // Not applicable: no branch/worktree to query. Distinct from the
            // `gh`-can't-tell-us cases below, which return 200 + unavailable_reason.
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode (no associated branch)".to_string(),
            ));
        }
    };

    let cwd = PathBuf::from(cwd);
    if !cwd.is_dir() {
        return Ok(Json(PrStatusResponse::unavailable(
            PrUnavailableReason::NotGitRepo,
        )));
    }

    tokio::task::spawn_blocking(move || get_pr_status_for_branch(&cwd, &branch_name))
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))
        .map(Json)
}

fn get_pr_status_for_branch(cwd: &std::path::Path, branch_name: &str) -> PrStatusResponse {
    if run_git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return PrStatusResponse::unavailable(PrUnavailableReason::NotGitRepo);
    }

    let list_output = match run_gh(
        cwd,
        &[
            "pr",
            "list",
            "--head",
            branch_name,
            "--state",
            "all",
            "--limit",
            "1",
            "--json",
            "number,title,url,state,isDraft,baseRefName,headRefName",
        ],
    ) {
        Ok(output) => output,
        Err(e) => {
            tracing::debug!(branch = %branch_name, error = %e.message, "gh pr list failed");
            return PrStatusResponse::unavailable(e.reason);
        }
    };

    let mut prs: Vec<GhPrListItem> = match serde_json::from_str(&list_output) {
        Ok(prs) => prs,
        Err(e) => {
            tracing::debug!(branch = %branch_name, output = %list_output, error = %e, "failed to parse gh pr list JSON");
            return PrStatusResponse::unavailable(PrUnavailableReason::CommandFailed);
        }
    };

    let Some(pr) = prs.pop() else {
        return PrStatusResponse::not_found();
    };

    let checks_state = fetch_pr_checks_state(cwd, pr.number);
    let display_state = normalize_pr_display_state(&pr.state, pr.is_draft);

    PrStatusResponse {
        found: true,
        unavailable_reason: None,
        number: Some(pr.number),
        title: Some(pr.title),
        url: Some(pr.url),
        state: Some(pr.state),
        draft: Some(pr.is_draft),
        base: Some(pr.base_ref_name),
        head: Some(pr.head_ref_name),
        check_state: Some(checks_state),
        display_state: Some(display_state),
    }
}

fn fetch_pr_checks_state(cwd: &std::path::Path, number: u64) -> PrCheckState {
    let number = number.to_string();
    let out = match run_gh_raw(
        cwd,
        &[
            "pr",
            "checks",
            &number,
            "--json",
            "name,state,bucket",
            "--watch=false",
        ],
    ) {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(pr = %number, error = %e.message, "gh pr checks could not run");
            return PrCheckState::Unknown;
        }
    };

    // `gh pr checks` exits non-zero when checks are failing (1) or pending (8) but still
    // emits the JSON we need to classify them — so we key off the parsed output, not the
    // exit code. Only a usage/auth error (empty or non-JSON stdout) yields `Unknown`.
    let stdout = out.stdout.trim();
    if stdout.is_empty() {
        tracing::debug!(pr = %number, stderr = %out.stderr.trim(), "gh pr checks produced no output");
        return PrCheckState::Unknown;
    }
    match serde_json::from_str::<Vec<GhPrCheck>>(stdout) {
        Ok(checks) => normalize_checks(&checks),
        Err(e) => {
            tracing::debug!(pr = %number, output = %stdout, error = %e, "failed to parse gh pr checks JSON");
            PrCheckState::Unknown
        }
    }
}

#[derive(Debug)]
struct GhError {
    reason: PrUnavailableReason,
    message: String,
}

struct GhOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run `gh` with an 8s timeout, returning the exit status and captured output.
/// `Err` only on spawn failure or timeout — a non-zero exit is still `Ok` so the
/// caller can decide (some `gh` subcommands exit non-zero but still emit useful JSON).
fn run_gh_raw(cwd: &std::path::Path, args: &[&str]) -> Result<GhOutput, GhError> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut child = Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GhError {
            reason: if e.kind() == std::io::ErrorKind::NotFound {
                PrUnavailableReason::GhMissing
            } else {
                PrUnavailableReason::CommandFailed
            },
            message: format!("Failed to run gh {}: {e}", args.join(" ")),
        })?;

    // Drain stdout/stderr in background threads so a large `gh` response (e.g.
    // a PR with many checks) can't fill the OS pipe buffer and wedge the child
    // while we poll for exit. The threads finish on EOF — which arrives when
    // the child exits or is killed — so they always join cleanly.
    let mut stdout_pipe = child.stdout.take().expect("gh stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("gh stderr is piped");
    let stdout_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| GhError {
            reason: PrUnavailableReason::CommandFailed,
            message: format!("gh {} wait failed: {e}", args.join(" ")),
        })? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_h.join();
            let _ = stderr_h.join();
            return Err(GhError {
                reason: PrUnavailableReason::CommandFailed,
                message: format!("gh {} timed out", args.join(" ")),
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    Ok(GhOutput {
        status,
        stdout: String::from_utf8_lossy(&stdout_h.join().unwrap_or_default())
            .trim()
            .to_string(),
        stderr: String::from_utf8_lossy(&stderr_h.join().unwrap_or_default())
            .trim()
            .to_string(),
    })
}

/// Run `gh` expecting success; maps a non-zero exit to a typed `GhError`
/// (auth vs generic failure) and returns trimmed stdout on success.
fn run_gh(cwd: &std::path::Path, args: &[&str]) -> Result<String, GhError> {
    let out = run_gh_raw(cwd, args)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let lower = out.stderr.to_lowercase();
        let reason = if lower.contains("not logged")
            || lower.contains("not authenticated")
            || lower.contains("authentication")
            || lower.contains("gh auth login")
        {
            PrUnavailableReason::NotAuthenticated
        } else {
            PrUnavailableReason::CommandFailed
        };
        Err(GhError {
            reason,
            message: format!("gh {} failed: {}", args.join(" "), out.stderr),
        })
    }
}

#[derive(Debug, Deserialize)]
struct GhPrListItem {
    number: u64,
    title: String,
    url: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
}

#[derive(Debug, Deserialize)]
struct GhPrCheck {
    state: Option<String>,
    bucket: Option<String>,
}

fn normalize_pr_display_state(state: &str, draft: bool) -> PrDisplayState {
    if draft {
        return PrDisplayState::Draft;
    }
    match state.to_ascii_uppercase().as_str() {
        "MERGED" => PrDisplayState::Merged,
        "CLOSED" => PrDisplayState::Closed,
        _ => PrDisplayState::Open,
    }
}

fn normalize_checks(checks: &[GhPrCheck]) -> PrCheckState {
    if checks.is_empty() {
        return PrCheckState::Unknown;
    }

    let mut has_pending = false;
    for check in checks {
        let state = check
            .state
            .as_deref()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let bucket = check
            .bucket
            .as_deref()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            state.as_str(),
            "FAILURE" | "ERROR" | "CANCELLED" | "ACTION_REQUIRED"
        ) || matches!(bucket.as_str(), "FAIL" | "CANCEL" | "ACTION_REQUIRED")
        {
            return PrCheckState::Failing;
        }
        if !matches!(state.as_str(), "SUCCESS" | "PASS" | "SKIPPED")
            && !matches!(bucket.as_str(), "PASS" | "SKIP")
        {
            has_pending = true;
        }
    }

    if has_pending {
        PrCheckState::Pending
    } else {
        PrCheckState::Passing
    }
}

/// `GET /api/conversations/:id/diff` — committed and uncommitted changes
/// in the conversation's worktree, vs the base branch. Read-only; used by
/// the Work/Branch-mode "View diff" action so users can review before
/// deciding to merge or abandon.
///
/// Requires the conversation to be in Work or Branch mode (anything else
/// has no worktree to diff). Any conversation state is acceptable —
/// inspection during streaming is fine and useful.
///
/// Each diff section is capped at 256KiB; truncation metadata is returned
/// alongside so the UI can show a "(truncated, X KiB total)" hint.
pub(crate) async fn get_conversation_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConversationDiffResponse>, AppError> {
    const MAX_DIFF_BYTES: usize = 256 * 1024;

    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let (worktree_path, base_branch) = match &conv.conv_mode {
        ConvMode::Work {
            worktree_path,
            base_branch,
            ..
        }
        | ConvMode::Branch {
            worktree_path,
            base_branch,
            ..
        } => (worktree_path.to_string(), base_branch.to_string()),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode (no worktree to diff)".to_string(),
            ));
        }
    };

    tokio::task::spawn_blocking(move || {
        let wt = PathBuf::from(&worktree_path);
        if !wt.exists() {
            return Err(AppError::NotFound(format!(
                "Worktree no longer exists: {worktree_path}"
            )));
        }

        let captured = capture_branch_diff(&wt, &base_branch, MAX_DIFF_BYTES);

        Ok(ConversationDiffResponse {
            comparator: captured.comparator,
            commit_log: captured.commit_log,
            committed_truncated_kib: truncated_kib(
                &captured.committed_diff,
                captured.committed_total_bytes,
                captured.committed_saturated,
            ),
            committed_saturated: captured.committed_saturated,
            committed_diff: captured.committed_diff,
            uncommitted_truncated_kib: truncated_kib(
                &captured.uncommitted_diff,
                captured.uncommitted_total_bytes,
                captured.uncommitted_saturated,
            ),
            uncommitted_saturated: captured.uncommitted_saturated,
            uncommitted_diff: captured.uncommitted_diff,
        })
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?
    .map(Json)
}

/// Convert the streamed-capture metadata into the wire `Option<u32>`:
/// `None` when the diff fit under the cap, otherwise the total stdout
/// size in KiB. When `saturated` is true the returned value is a lower
/// bound (we hit the hard read limit and stopped counting).
fn truncated_kib(stdout: &str, total_bytes: u64, saturated: bool) -> Option<u32> {
    if !saturated && total_bytes <= stdout.len() as u64 {
        return None;
    }
    Some(u32::try_from(total_bytes / 1024).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_kib_passthrough_when_under_cap() {
        // 5-byte stdout, 5 total bytes, not saturated → None.
        assert_eq!(truncated_kib("short", 5, false), None);
    }

    #[test]
    fn truncated_kib_at_exact_cap_is_passthrough() {
        let body = "x".repeat(100);
        assert_eq!(truncated_kib(&body, 100, false), None);
    }

    #[test]
    fn truncated_kib_over_cap_reports_kib() {
        // 1 KiB visible, 3 KiB total, not saturated → Some(3).
        let body = "x".repeat(1024);
        assert_eq!(truncated_kib(&body, 3072, false), Some(3));
    }

    #[test]
    fn truncated_kib_saturated_returns_lower_bound() {
        // Saturated always reports the (lower-bound) total even if it
        // happens to equal stdout.len() — caller must show "≥X KiB" UI.
        assert_eq!(truncated_kib("x", 8 * 1024, true), Some(8));
    }

    #[test]
    fn normalize_pr_display_state_prefers_draft() {
        assert_eq!(
            normalize_pr_display_state("OPEN", true),
            PrDisplayState::Draft
        );
        assert_eq!(
            normalize_pr_display_state("MERGED", false),
            PrDisplayState::Merged
        );
        assert_eq!(
            normalize_pr_display_state("CLOSED", false),
            PrDisplayState::Closed
        );
        assert_eq!(
            normalize_pr_display_state("OPEN", false),
            PrDisplayState::Open
        );
    }

    #[test]
    fn normalize_checks_classifies_empty_as_unknown() {
        assert_eq!(normalize_checks(&[]), PrCheckState::Unknown);
    }

    #[test]
    fn normalize_checks_classifies_pass_pending_and_fail() {
        assert_eq!(
            normalize_checks(&[GhPrCheck {
                state: Some("SUCCESS".to_string()),
                bucket: Some("pass".to_string()),
            }]),
            PrCheckState::Passing
        );
        assert_eq!(
            normalize_checks(&[GhPrCheck {
                state: Some("PENDING".to_string()),
                bucket: Some("pending".to_string()),
            }]),
            PrCheckState::Pending
        );
        assert_eq!(
            normalize_checks(&[GhPrCheck {
                state: Some("FAILURE".to_string()),
                bucket: Some("fail".to_string()),
            }]),
            PrCheckState::Failing
        );
    }
}
