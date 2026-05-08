//! Git-related HTTP handlers: branch listing, search, conflict detection,
//! per-conversation diff snapshots.

use super::handlers::AppError;
use super::types::{
    ConversationDiffResponse, GitBranchEntry, GitBranchesQuery, GitBranchesResponse,
};
use super::AppState;
use crate::db::ConvMode;
use crate::git_ops::{capture_branch_diff, run_git};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use std::path::PathBuf;

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
}
