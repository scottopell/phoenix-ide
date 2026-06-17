#![allow(clippy::wildcard_enum_match_arm)]
//! Git-related HTTP handlers: branch listing, search, conflict detection,
//! per-conversation diff snapshots.

use super::handlers::AppError;
use super::types::{
    ConversationDiffResponse, GitBranchEntry, GitBranchesQuery, GitBranchesResponse,
    PrAutoFixContextResponse, PrStatusResponse, PrUnavailableReason, WorkChangeNeedsReviewReason,
    WorkChangeSummary,
};
use super::AppState;
use crate::db::ConvMode;
use crate::git_ops::{capture_branch_diff, run_git};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};

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

fn percent_encode_url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

fn github_owner_repo_from_remote(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim();
    let path = if let Some(rest) = remote.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        rest
    } else if let Some(rest) = remote.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = remote.strip_prefix("http://github.com/") {
        rest
    } else {
        return None;
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = path.split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((owner, repo))
}

pub(crate) fn summarize_work_change(
    worktree: &FsPath,
    branch_name: &str,
    base_branch: &str,
) -> WorkChangeSummary {
    let has_uncommitted = match run_git(
        worktree,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    ) {
        Ok(status) => !status.trim().is_empty(),
        Err(error) => return WorkChangeSummary::Unavailable { reason: error },
    };
    if has_uncommitted {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::UncommittedChanges,
        };
    }

    let comparator = crate::git_ops::effective_base_ref(worktree, base_branch);
    let work_commit_count = match run_git(
        worktree,
        &["rev-list", "--count", &format!("{comparator}..HEAD")],
    ) {
        Ok(count) => count.trim().parse::<u32>().unwrap_or(0),
        Err(_) => {
            return WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::Unknown,
            };
        }
    };
    if work_commit_count == 0 {
        return WorkChangeSummary::Clean;
    }

    let remote_ref = format!("origin/{branch_name}");
    if run_git(worktree, &["rev-parse", "--verify", &remote_ref]).is_err() {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::BranchNotPushed,
        };
    }

    let counts = run_git(
        worktree,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{branch_name}...{remote_ref}"),
        ],
    )
    .ok()
    .and_then(|s| {
        let mut parts = s.split_whitespace();
        let ahead = parts.next()?.parse::<u32>().ok()?;
        let behind = parts.next()?.parse::<u32>().ok()?;
        Some((ahead, behind))
    });
    let Some((ahead, behind)) = counts else {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::Unknown,
        };
    };
    if ahead > 0 && behind > 0 {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::RemoteDiverged,
        };
    }
    if ahead > 0 {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::LocalAheadOfRemote,
        };
    }
    if behind > 0 {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::RemoteDiverged,
        };
    }

    let remote_url = match run_git(worktree, &["config", "--get", "remote.origin.url"]) {
        Ok(url) if !url.trim().is_empty() => url,
        Ok(_) | Err(_) => {
            return WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::UnknownRemote,
            };
        }
    };
    let Some((owner, repo)) = github_owner_repo_from_remote(&remote_url) else {
        return WorkChangeSummary::DirtyNeedsReview {
            reason: WorkChangeNeedsReviewReason::NonGithubRemote,
        };
    };

    WorkChangeSummary::DirtyPrReady {
        create_pr_url: format!(
            "https://github.com/{}/{}/compare/{}...{}?expand=1",
            percent_encode_url_component(&owner),
            percent_encode_url_component(&repo),
            percent_encode_url_component(base_branch),
            percent_encode_url_component(branch_name),
        ),
        branch_name: branch_name.to_string(),
        base_branch: base_branch.to_string(),
    }
}
fn stale_primary_response_with_work_change(
    primary: &crate::db::WorkScopePrAssociation,
    refresh: crate::api::pr_monitoring::PrStatusRefresh,
) -> PrStatusResponse {
    let mut response = crate::api::pr_monitoring::stale_primary_response_with_refresh_state(
        primary,
        refresh.response.refresh.state,
        refresh.response.refresh.reason.clone(),
        refresh.response.refresh.last_attempted_at,
    );
    response.work_change = refresh.response.work_change;
    response
}

async fn pr_status_response_for_missing_worktree(
    state: &AppState,
    work_scope: &crate::work_scope::WorkScope,
) -> Result<PrStatusResponse, AppError> {
    let attempted_at = chrono::Utc::now().to_rfc3339();
    let mut response = match state
        .runtime
        .db()
        .primary_work_scope_pr_association(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        Some(pr) => crate::api::pr_monitoring::stale_response(
            pr,
            PrUnavailableReason::NotGitRepo,
            attempted_at,
        ),
        None => PrStatusResponse::unavailable(PrUnavailableReason::NotGitRepo),
    };
    response.work_change = WorkChangeSummary::Unavailable {
        reason: "worktree path is not a directory".to_string(),
    };
    Ok(response)
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

    let (branch_name, base_branch, cwd, work_scope) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            base_branch,
            worktree_path,
            ..
        }
        | ConvMode::Branch {
            branch_name,
            base_branch,
            worktree_path,
            ..
        } => (
            branch_name.to_string(),
            base_branch.to_string(),
            worktree_path.to_string(),
            crate::work_scope::WorkScope::resolve(
                &id,
                Some(std::path::Path::new(worktree_path.as_str())),
            ),
        ),
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
        let response = pr_status_response_for_missing_worktree(&state, &work_scope).await?;
        return Ok(Json(response));
    }

    let db = state.runtime.db().clone();
    let cwd_for_status = cwd.clone();
    let cwd_for_change = cwd.clone();
    let branch_name_for_status = branch_name.clone();
    let branch_name_for_change = branch_name.clone();
    let base_branch_for_change = base_branch.clone();
    let refresh = tokio::task::spawn_blocking(move || {
        let mut refresh = crate::api::pr_monitoring::get_pr_status_for_branch(
            &cwd_for_status,
            &branch_name_for_status,
        );
        refresh.response.work_change = summarize_work_change(
            &cwd_for_change,
            &branch_name_for_change,
            &base_branch_for_change,
        );
        refresh
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;

    if !refresh.observations.is_empty() {
        db.upsert_work_scope_pr_observations(&work_scope, &refresh.observations)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    if let Some(primary) = db
        .primary_work_scope_pr_association(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        if refresh.response.refresh.state != crate::api::types::PrRefreshState::Fresh {
            return Ok(Json(stale_primary_response_with_work_change(
                &primary, refresh,
            )));
        }

        if refresh.response.number != Some(primary.pr_number) {
            return Ok(Json(stale_primary_response_with_work_change(
                &primary, refresh,
            )));
        }
    }

    let response = attach_pr_feedback_freshness(refresh.response, &db, &work_scope, &cwd).await?;

    Ok(Json(response))
}

async fn attach_pr_feedback_freshness(
    mut response: PrStatusResponse,
    db: &crate::db::Database,
    work_scope: &crate::work_scope::WorkScope,
    cwd: &FsPath,
) -> Result<PrStatusResponse, AppError> {
    let response_pr_number = response.number;
    let response_updated_at = response.updated_at.clone();

    let (true, Some(pr_number), Some(updated_at)) = (
        response.found,
        response_pr_number,
        response_updated_at.as_deref(),
    ) else {
        return Ok(response);
    };

    let Some(baseline) = db
        .work_scope_pr_feedback_baseline(work_scope, pr_number)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    else {
        return Ok(response);
    };

    if crate::api::pr_monitoring::pr_updated_after_baseline(&baseline, updated_at) {
        let cwd_for_feedback = cwd.to_path_buf();
        let feedback = tokio::task::spawn_blocking(move || {
            crate::api::pr_monitoring::fetch_pr_feedback_for_pr(&cwd_for_feedback, pr_number)
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;
        match feedback {
            Ok(feedback) => {
                // Content change and coverage health are independent signals
                // derived from the same fetch: the count reflects only the
                // surfaces that were read, and coverage flags any that weren't.
                response.feedback_freshness =
                    crate::api::pr_monitoring::feedback_freshness_from_baseline(
                        &baseline,
                        Some(&feedback),
                    );
                response.feedback_coverage = crate::api::pr_monitoring::coverage_health(&feedback);
            }
            Err(err) => {
                // Couldn't fetch feedback at all (e.g. not a git repo) — leave
                // both signals absent rather than guessing.
                tracing::debug!(pr = pr_number, error = %err, "could not fetch PR feedback to classify freshness");
            }
        }
    }

    Ok(response)
}

pub(crate) async fn create_pr_auto_fix_context(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PrAutoFixContextResponse>, AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let (branch_name, worktree_path, work_scope) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            ..
        }
        | ConvMode::Branch {
            branch_name,
            worktree_path,
            ..
        } => (
            branch_name.to_string(),
            worktree_path.to_string(),
            crate::work_scope::WorkScope::resolve(
                &id,
                Some(std::path::Path::new(worktree_path.as_str())),
            ),
        ),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode (no associated PR)".to_string(),
            ));
        }
    };

    let db = state.runtime.db().clone();
    let associated_pr = db
        .primary_work_scope_pr_association(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let result = tokio::task::spawn_blocking(move || {
        let worktree = PathBuf::from(worktree_path);
        if let Some(pr) = associated_pr {
            crate::api::pr_monitoring::capture_pr_auto_fix_context_for_pr(
                &worktree,
                pr.pr_number,
                conv.llm_language,
            )
        } else {
            crate::api::pr_monitoring::capture_pr_auto_fix_context_for_branch(
                &worktree,
                &branch_name,
                conv.llm_language,
            )
        }
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;

    let (response, observations, _result_baseline) = match result {
        Ok(capture) => (
            Ok(capture.response),
            capture.observations,
            Some(capture.baseline),
        ),
        Err(crate::api::pr_monitoring::PrMonitorError::BadRequestWithObservations {
            message,
            observations,
        }) => (Err(AppError::BadRequest(message)), observations, None),
        Err(crate::api::pr_monitoring::PrMonitorError::BadRequest(message)) => {
            return Err(AppError::BadRequest(message));
        }
        Err(crate::api::pr_monitoring::PrMonitorError::Internal(message)) => {
            return Err(AppError::Internal(message));
        }
    };

    if !observations.is_empty() {
        db.upsert_work_scope_pr_observations(&work_scope, &observations)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(Json(response?))
}

fn validate_pr_auto_fix_artifact_path(artifact_path: &str) -> Result<(), AppError> {
    let path = std::path::Path::new(artifact_path);
    if path.is_absolute() {
        return Err(AppError::BadRequest(
            "Invalid PR context artifact path".to_string(),
        ));
    }

    let components = path.components().collect::<Vec<_>>();
    let valid_prefix = matches!(components.first(), Some(std::path::Component::Normal(part)) if *part == ".phoenix")
        && matches!(components.get(1), Some(std::path::Component::Normal(part)) if *part == "pr-context")
        && components.len() > 2;
    let only_normal_components = components
        .iter()
        .all(|component| matches!(component, std::path::Component::Normal(_)));
    if valid_prefix && only_normal_components {
        return Ok(());
    }

    Err(AppError::BadRequest(
        "Invalid PR context artifact path".to_string(),
    ))
}

fn pr_auto_fix_artifact_path(
    conv: &crate::db::Conversation,
    artifact_path: &str,
) -> Result<PathBuf, AppError> {
    validate_pr_auto_fix_artifact_path(artifact_path)?;
    Ok(match &conv.conv_mode {
        ConvMode::Work { worktree_path, .. } | ConvMode::Branch { worktree_path, .. } => {
            let worktree_artifact =
                std::path::Path::new(worktree_path.as_str()).join(artifact_path);
            if worktree_artifact.exists() {
                worktree_artifact
            } else {
                let cwd_artifact = std::path::Path::new(&conv.cwd).join(artifact_path);
                if cwd_artifact.exists() {
                    cwd_artifact
                } else {
                    worktree_artifact
                }
            }
        }
        _ => std::path::Path::new(&conv.cwd).join(artifact_path),
    })
}

pub(crate) async fn record_pr_auto_fix_context_baseline(
    db: &crate::db::Database,
    conversation_id: &str,
    text: &str,
) -> Result<(), AppError> {
    let Some(raw) = text.strip_prefix(phoenix_core::llm_language::pr_auto_fix_instruction_prefix())
    else {
        return Ok(());
    };
    let Some((artifact_path, _)) = raw.split_once('`') else {
        return Ok(());
    };

    let conv = db
        .get_conversation(conversation_id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let work_scope = match &conv.conv_mode {
        ConvMode::Work { worktree_path, .. } | ConvMode::Branch { worktree_path, .. } => {
            crate::work_scope::WorkScope::resolve(
                conversation_id,
                Some(std::path::Path::new(worktree_path.as_str())),
            )
        }
        _ => return Ok(()),
    };
    let artifact = crate::api::pr_monitoring::read_pr_auto_fix_context_artifact(
        &pr_auto_fix_artifact_path(&conv, artifact_path)?,
    )
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let baseline = artifact.baseline();
    db.upsert_work_scope_pr_feedback_baseline(&work_scope, &baseline)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(())
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

    fn conversation_with_mode(
        cwd: &std::path::Path,
        conv_mode: ConvMode,
    ) -> crate::db::Conversation {
        let now = chrono::Utc::now();
        crate::db::Conversation {
            id: "conv-test".to_string(),
            slug: None,
            title: None,
            cwd: cwd.to_string_lossy().to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: crate::db::ConvState::Idle,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: None,
            project_id: None,
            conv_mode,
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            steering_queue: Vec::new(),
            llm_language: crate::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
        }
    }

    fn init_repo(dir: &std::path::Path) {
        run_git(dir, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        run_git(dir, &["config", "user.email", "probe@test"]).unwrap();
        run_git(dir, &["config", "user.name", "probe"]).unwrap();
        run_git(dir, &["commit", "--allow-empty", "-q", "-m", "init"]).unwrap();
    }

    fn commit_file(dir: &std::path::Path, name: &str, content: &str, message: &str) {
        std::fs::write(dir.join(name), content).unwrap();
        run_git(dir, &["add", name]).unwrap();
        run_git(dir, &["commit", "-q", "-m", message]).unwrap();
    }

    fn bare_remote() -> tempfile::TempDir {
        let remote = tempfile::tempdir().unwrap();
        run_git(
            remote.path(),
            &["init", "--bare", "--quiet", "--initial-branch=main"],
        )
        .unwrap();
        remote
    }

    fn push_branch(repo: &std::path::Path, branch: &str) {
        run_git(repo, &["push", "--quiet", "-u", "origin", branch]).unwrap();
        run_git(repo, &["fetch", "--quiet", "origin"]).unwrap();
    }

    #[test]
    fn work_change_clean_branch() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        assert_eq!(
            summarize_work_change(repo.path(), "main", "main"),
            WorkChangeSummary::Clean
        );
    }

    #[test]
    fn work_change_uncommitted_changes_need_review() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(repo.path().join("dirty.txt"), "dirty").unwrap();
        assert_eq!(
            summarize_work_change(repo.path(), "main", "main"),
            WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::UncommittedChanges
            }
        );
    }

    #[test]
    fn work_change_branch_not_pushed() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["checkout", "-q", "-b", "task"]).unwrap();
        commit_file(repo.path(), "a.txt", "a", "a");
        assert_eq!(
            summarize_work_change(repo.path(), "task", "main"),
            WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::BranchNotPushed
            }
        );
    }

    #[test]
    fn work_change_missing_base_needs_review() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["checkout", "-q", "-b", "task"]).unwrap();
        commit_file(repo.path(), "a.txt", "a", "a");
        assert_eq!(
            summarize_work_change(repo.path(), "task", "missing-base"),
            WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::Unknown
            }
        );
    }

    #[test]
    fn work_change_pushed_github_branch_is_pr_ready() {
        let remote = bare_remote();
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        )
        .unwrap();
        push_branch(repo.path(), "main");
        run_git(
            repo.path(),
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:acme/repo.git",
            ],
        )
        .unwrap();
        run_git(repo.path(), &["checkout", "-q", "-b", "task"]).unwrap();
        commit_file(repo.path(), "a.txt", "a", "a");
        run_git(
            repo.path(),
            &[
                "remote",
                "set-url",
                "origin",
                remote.path().to_str().unwrap(),
            ],
        )
        .unwrap();
        push_branch(repo.path(), "task");
        run_git(
            repo.path(),
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:acme/repo.git",
            ],
        )
        .unwrap();

        assert_eq!(
            summarize_work_change(repo.path(), "task", "main"),
            WorkChangeSummary::DirtyPrReady {
                create_pr_url: "https://github.com/acme/repo/compare/main...task?expand=1"
                    .to_string(),
                branch_name: "task".to_string(),
                base_branch: "main".to_string(),
            }
        );
    }

    #[test]
    fn work_change_local_ahead_of_remote_needs_review() {
        let remote = bare_remote();
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        )
        .unwrap();
        push_branch(repo.path(), "main");
        run_git(repo.path(), &["checkout", "-q", "-b", "task"]).unwrap();
        commit_file(repo.path(), "a.txt", "a", "a");
        push_branch(repo.path(), "task");
        commit_file(repo.path(), "b.txt", "b", "b");
        assert_eq!(
            summarize_work_change(repo.path(), "task", "main"),
            WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::LocalAheadOfRemote
            }
        );
    }

    #[test]
    fn work_change_remote_diverged_needs_review() {
        let remote = bare_remote();
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        )
        .unwrap();
        push_branch(repo.path(), "main");
        run_git(repo.path(), &["checkout", "-q", "-b", "task"]).unwrap();
        commit_file(repo.path(), "a.txt", "a", "a");
        push_branch(repo.path(), "task");
        run_git(
            repo.path(),
            &["checkout", "-q", "-b", "remote-task", "origin/task"],
        )
        .unwrap();
        commit_file(repo.path(), "remote.txt", "remote", "remote");
        run_git(
            repo.path(),
            &["push", "--quiet", "origin", "remote-task:task"],
        )
        .unwrap();
        run_git(repo.path(), &["checkout", "-q", "task"]).unwrap();
        commit_file(repo.path(), "local.txt", "local", "local");
        run_git(repo.path(), &["fetch", "--quiet", "origin"]).unwrap();
        assert_eq!(
            summarize_work_change(repo.path(), "task", "main"),
            WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::RemoteDiverged
            }
        );
    }

    #[test]
    fn work_change_non_github_remote_needs_review() {
        let remote = bare_remote();
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        )
        .unwrap();
        push_branch(repo.path(), "main");
        run_git(repo.path(), &["checkout", "-q", "-b", "task"]).unwrap();
        commit_file(repo.path(), "a.txt", "a", "a");
        push_branch(repo.path(), "task");
        assert_eq!(
            summarize_work_change(repo.path(), "task", "main"),
            WorkChangeSummary::DirtyNeedsReview {
                reason: WorkChangeNeedsReviewReason::NonGithubRemote
            }
        );
    }

    #[test]
    fn pr_auto_fix_artifact_path_prefers_worktree_root_over_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let cwd = worktree.join("nested");
        std::fs::create_dir_all(worktree.join(".phoenix/pr-context")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let artifact_path = ".phoenix/pr-context/pr-12.json";
        let artifact = worktree.join(artifact_path);
        std::fs::write(&artifact, "{}").unwrap();
        let conv = conversation_with_mode(
            &cwd,
            ConvMode::Branch {
                branch_name: crate::db::NonEmptyString::new("feature").unwrap(),
                worktree_path: crate::db::NonEmptyString::new(
                    worktree.to_string_lossy().to_string(),
                )
                .unwrap(),
                base_branch: crate::db::NonEmptyString::new("main").unwrap(),
            },
        );

        assert_eq!(
            pr_auto_fix_artifact_path(&conv, artifact_path).unwrap(),
            artifact
        );
    }

    #[test]
    fn pr_auto_fix_artifact_path_preserves_cwd_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(cwd.join(".phoenix/pr-context")).unwrap();
        let artifact_path = ".phoenix/pr-context/pr-12.json";
        let artifact = cwd.join(artifact_path);
        std::fs::write(&artifact, "{}").unwrap();
        let conv = conversation_with_mode(
            &cwd,
            ConvMode::Work {
                branch_name: crate::db::NonEmptyString::new("task-test").unwrap(),
                worktree_path: crate::db::NonEmptyString::new(
                    worktree.to_string_lossy().to_string(),
                )
                .unwrap(),
                base_branch: crate::db::NonEmptyString::new("main").unwrap(),
                task_id: crate::db::NonEmptyString::new("11002").unwrap(),
                task_title: crate::db::NonEmptyString::new("Fix freshness").unwrap(),
            },
        );

        assert_eq!(
            pr_auto_fix_artifact_path(&conv, artifact_path).unwrap(),
            artifact
        );
    }

    #[test]
    fn pr_auto_fix_artifact_path_rejects_paths_outside_pr_context_dir() {
        let temp = tempfile::tempdir().unwrap();
        let conv = conversation_with_mode(temp.path(), ConvMode::Direct);

        for artifact_path in [
            "/tmp/pr-12.json",
            "../.phoenix/pr-context/pr-12.json",
            ".phoenix/../pr-context/pr-12.json",
            ".phoenix/not-pr-context/pr-12.json",
            ".phoenix/pr-context/../secrets.json",
        ] {
            assert!(
                pr_auto_fix_artifact_path(&conv, artifact_path).is_err(),
                "expected {artifact_path} to be rejected",
            );
        }
    }

    #[test]
    fn truncated_kib_passthrough_when_under_cap() {
        assert_eq!(truncated_kib("short", 5, false), None);
    }

    #[test]
    fn truncated_kib_at_exact_cap_is_passthrough() {
        let body = "x".repeat(100);
        assert_eq!(truncated_kib(&body, 100, false), None);
    }

    #[test]
    fn truncated_kib_over_cap_reports_kib() {
        let body = "x".repeat(1024);
        assert_eq!(truncated_kib(&body, 3072, false), Some(3));
    }

    #[test]
    fn truncated_kib_saturated_returns_lower_bound() {
        assert_eq!(truncated_kib("x", 8 * 1024, true), Some(8));
    }
}
