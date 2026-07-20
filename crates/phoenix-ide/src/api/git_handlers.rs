#![allow(clippy::wildcard_enum_match_arm)]
//! Git-related HTTP handlers: branch listing, search, conflict detection,
//! per-conversation diff snapshots.

use super::handlers::AppError;
use super::types::{
    ActivePrIdentityResponse, ActivePrSelectionMutationResponse,
    ActivePrSelectionProvenanceResponse, ActivePrSelectionResponse, AssociatedPrStatusEnvelope,
    AssociatedPrSummaryResponse, BranchRemoteStatus, CheckoutStatus, ConversationDiffResponse,
    ConversationGitStatusResponse, GitBranchEntry, GitBranchesQuery, GitBranchesResponse,
    GitChangedPath, GitFileStatus, GitStatusCounts, ObservedBranchSummaryResponse,
    PinAssociatedPrRequest, PrAutoFixContextResponse, PrFeedbackStatus, PrStatusResponse,
    PrUnavailableReason, WorkChangeNeedsReviewReason, WorkChangeSummary,
};
use super::AppState;
use crate::db::ConvMode;
use crate::git_ops::{capture_branch_diff, run_git};

use axum::http::StatusCode;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use tracing::Instrument as _;

fn build_diff_response(
    captured: crate::git_ops::CapturedDiff,
    label: String,
    kind: &str,
    pr_number: Option<u64>,
    checkout_status: CheckoutStatus,
) -> ConversationDiffResponse {
    ConversationDiffResponse {
        comparator: captured.comparator,
        label,
        kind: kind.to_string(),
        pr_number,
        checkout_status,
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
    }
}

fn active_pr_selection_response(
    selection: phoenix_core::domain::active_pr_selection::ActivePrSelection,
) -> ActivePrSelectionResponse {
    ActivePrSelectionResponse {
        pr: ActivePrIdentityResponse {
            repo_owner: selection.pr.repo_owner,
            repo_name: selection.pr.repo_name,
            pr_number: selection.pr.pr_number,
        },
        provenance: match selection.provenance {
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Inferred => {
                ActivePrSelectionProvenanceResponse::Inferred
            }
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned => {
                ActivePrSelectionProvenanceResponse::Pinned
            }
        },
    }
}

fn github_repo_identifier_from_worktree(path: &FsPath) -> Option<String> {
    crate::runtime::pr_status_poll::github_repo_identifier(path)
}

fn github_repo_identity_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveCheckoutObservation {
    branch_name: String,
    remote_status: BranchRemoteStatus,
}

const CHECKOUT_CAPTURE_ATTEMPTS: usize = 2;
const GIT_STATUS_MAX_OUTPUT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitStatusSnapshot {
    counts: GitStatusCounts,
    changed_paths: Vec<GitChangedPath>,
}

fn configured_upstream_ref(worktree_path: &FsPath, branch_name: &str) -> Option<String> {
    run_git(
        worktree_path,
        &[
            "for-each-ref",
            "--format=%(upstream)",
            &format!("refs/heads/{branch_name}"),
        ],
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| value.starts_with("refs/remotes/"))
}

fn matching_remote_ref(worktree_path: &FsPath, branch_name: &str) -> Option<String> {
    let output = run_git(worktree_path, &["remote"]).ok()?;
    let mut remotes = output
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .collect::<Vec<_>>();
    remotes.sort_unstable();
    remotes.dedup();
    remotes.into_iter().find_map(|remote| {
        let candidate = format!("refs/remotes/{remote}/{branch_name}");
        run_git(
            worktree_path,
            &["for-each-ref", "--format=%(refname)", &candidate],
        )
        .ok()
        .filter(|value| value.trim() == candidate)
        .map(|_| candidate)
    })
}

fn ahead_behind_counts(
    worktree_path: &FsPath,
    head_oid: &str,
    remote_ref: &str,
) -> Result<(u32, u32), String> {
    const DISPLAY_SAFE_ERROR: &str = "Last-fetched remote comparison is unavailable.";
    let counts = match run_git(
        worktree_path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{head_oid}...{remote_ref}"),
        ],
    ) {
        Ok(counts) => counts,
        Err(error) => {
            tracing::warn!(%error, remote_ref, "failed to compare checkout with remote ref");
            return Err(DISPLAY_SAFE_ERROR.to_string());
        }
    };
    let parsed = (|| {
        let mut parts = counts.split_whitespace();
        let ahead = parts.next()?.parse::<u32>().ok()?;
        let behind = parts.next()?.parse::<u32>().ok()?;
        Some((ahead, behind))
    })();
    parsed.ok_or_else(|| {
        tracing::warn!(remote_ref, output = %counts, "git returned invalid ahead/behind counts");
        DISPLAY_SAFE_ERROR.to_string()
    })
}

fn branch_remote_status(
    worktree_path: &FsPath,
    branch_name: &str,
    head_oid: &str,
) -> BranchRemoteStatus {
    if let Some(remote_ref) = configured_upstream_ref(worktree_path, branch_name) {
        return match ahead_behind_counts(worktree_path, head_oid, &remote_ref) {
            Ok((ahead, behind)) => BranchRemoteStatus::Tracked {
                remote_ref,
                ahead,
                behind,
            },
            Err(reason) => BranchRemoteStatus::Unavailable { reason },
        };
    }

    let Some(remote_ref) = matching_remote_ref(worktree_path, branch_name) else {
        return BranchRemoteStatus::NoKnown;
    };
    match ahead_behind_counts(worktree_path, head_oid, &remote_ref) {
        Ok((ahead, behind)) => BranchRemoteStatus::Matching {
            remote_ref,
            ahead,
            behind,
        },
        Err(reason) => BranchRemoteStatus::Unavailable { reason },
    }
}

fn detached_pointing_refs(worktree_path: &FsPath, head_oid: &str) -> Vec<String> {
    const MAX_POINTING_REFS: usize = 8;
    let Ok(output) = run_git(
        worktree_path,
        &[
            "for-each-ref",
            &format!("--points-at={head_oid}"),
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    ) else {
        return Vec::new();
    };
    let mut refs = output
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    refs.truncate(MAX_POINTING_REFS);
    refs
}

fn live_checkout_observation_details(
    worktree_path: &FsPath,
    branch_name: String,
    head_oid: &str,
) -> LiveCheckoutObservation {
    LiveCheckoutObservation {
        remote_status: branch_remote_status(worktree_path, &branch_name, head_oid),
        branch_name,
    }
}

fn git_status_file_state(code: u8) -> Option<GitFileStatus> {
    match code {
        b'.' => Some(GitFileStatus::Unmodified),
        b'M' => Some(GitFileStatus::Modified),
        b'A' => Some(GitFileStatus::Added),
        b'D' => Some(GitFileStatus::Deleted),
        b'R' => Some(GitFileStatus::Renamed),
        b'C' => Some(GitFileStatus::Copied),
        b'T' => Some(GitFileStatus::TypeChanged),
        b'U' => Some(GitFileStatus::Unmerged),
        _ => None,
    }
}

fn parse_porcelain_v2_status_record(
    record: &[u8],
    rename_source: Option<&[u8]>,
) -> Result<Option<GitChangedPath>, String> {
    if record.is_empty() {
        return Ok(None);
    }
    match record[0] {
        b'1' | b'2' | b'u' => {
            if record.len() < 4 || record[1] != b' ' {
                return Err("invalid porcelain v2 record".to_string());
            }
            let xy = &record[2..4];
            let index_status = git_status_file_state(xy[0])
                .ok_or_else(|| "invalid porcelain v2 index status".to_string())?;
            let worktree_status = git_status_file_state(xy[1])
                .ok_or_else(|| "invalid porcelain v2 worktree status".to_string())?;
            let field_count = match record[0] {
                b'1' => 9,
                b'2' => 10,
                b'u' => 11,
                _ => unreachable!(),
            };
            let path = record
                .splitn(field_count, |byte| *byte == b' ')
                .nth(field_count - 1)
                .ok_or_else(|| "invalid porcelain v2 path record".to_string())?;
            let path = String::from_utf8(path.to_vec())
                .map_err(|_| "invalid utf-8 in git status path".to_string())?;
            Ok(Some(match record[0] {
                b'1' => GitChangedPath::Ordinary {
                    path,
                    index_status,
                    worktree_status,
                },
                b'2' => {
                    let previous_path = String::from_utf8(
                        rename_source
                            .ok_or_else(|| "rename record missing source path".to_string())?
                            .to_vec(),
                    )
                    .map_err(|_| "invalid utf-8 in git rename source".to_string())?;
                    if index_status == GitFileStatus::Copied {
                        GitChangedPath::Copied {
                            path,
                            source_path: previous_path,
                            worktree_status,
                        }
                    } else {
                        GitChangedPath::Renamed {
                            path,
                            previous_path,
                            worktree_status,
                        }
                    }
                }
                b'u' => GitChangedPath::Unmerged {
                    path,
                    index_status,
                    worktree_status,
                },
                _ => unreachable!(),
            }))
        }
        b'?' => {
            if record.len() < 3 || record[1] != b' ' {
                return Err("invalid porcelain v2 untracked record".to_string());
            }
            let path = String::from_utf8(record[2..].to_vec())
                .map_err(|_| "invalid utf-8 in git status path".to_string())?;
            Ok(Some(GitChangedPath::Untracked { path }))
        }
        b'!' | b'#' => Ok(None),
        _ => Err("unsupported porcelain v2 record".to_string()),
    }
}

fn parse_git_status_porcelain_v2(output: &[u8]) -> Result<GitStatusSnapshot, String> {
    let mut changed_paths = Vec::new();
    let mut parts = output.split(|byte| *byte == 0).peekable();
    let mut counts = GitStatusCounts::default();
    while let Some(record) = parts.next() {
        if record.is_empty() {
            continue;
        }
        let rename_source = if record.first() == Some(&b'2') {
            Some(
                parts
                    .next()
                    .ok_or_else(|| "rename record missing source path".to_string())?,
            )
        } else {
            None
        };
        let Some(changed_path) = parse_porcelain_v2_status_record(record, rename_source)? else {
            continue;
        };
        match &changed_path {
            GitChangedPath::Ordinary {
                index_status,
                worktree_status,
                ..
            } => {
                counts.changed_paths += 1;
                if *index_status != GitFileStatus::Unmodified {
                    counts.staged_paths += 1;
                }
                if *worktree_status != GitFileStatus::Unmodified {
                    counts.unstaged_paths += 1;
                }
            }
            GitChangedPath::Renamed {
                worktree_status, ..
            }
            | GitChangedPath::Copied {
                worktree_status, ..
            } => {
                counts.changed_paths += 1;
                counts.staged_paths += 1;
                if *worktree_status != GitFileStatus::Unmodified {
                    counts.unstaged_paths += 1;
                }
            }
            GitChangedPath::Unmerged { .. } => {
                counts.changed_paths += 1;
                counts.conflicted_paths += 1;
            }
            GitChangedPath::Untracked { .. } => {
                counts.changed_paths += 1;
                counts.untracked_paths += 1;
            }
        }
        changed_paths.push(changed_path);
    }
    Ok(GitStatusSnapshot {
        counts,
        changed_paths,
    })
}

fn capture_git_status_snapshot(worktree_path: &FsPath) -> Result<GitStatusSnapshot, String> {
    let repo_root = run_git(worktree_path, &["rev-parse", "--show-toplevel"])
        .map_err(|error| format!("git status failed: {error}"))?;
    let repo_root = std::fs::canonicalize(repo_root.trim())
        .map_err(|error| format!("git status repository root is unavailable: {error}"))?;
    let worktree_path = std::fs::canonicalize(worktree_path)
        .map_err(|error| format!("git status root is unavailable: {error}"))?;
    let pathspec = worktree_path
        .strip_prefix(&repo_root)
        .map_err(|_| "git status root is outside repository".to_string())?;
    let relative_path = pathspec
        .to_str()
        .ok_or_else(|| "git status root is not valid UTF-8".to_string())?;
    let pathspec = if relative_path.is_empty() {
        ":(literal).".to_string()
    } else {
        format!(":(literal){relative_path}")
    };

    let mut cmd = crate::git_ops::git_command();
    cmd.current_dir(&repo_root)
        .args([
            "-c",
            "status.renames=copies",
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--",
            &pathspec,
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("git status failed to start: {error}"))?;
    let stdout = crate::git_ops::read_child_stdout_bounded(
        &mut child,
        GIT_STATUS_MAX_OUTPUT_BYTES,
        "git status",
    )?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("git status failed: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git status failed: {stderr}"));
    }
    let mut snapshot = parse_git_status_porcelain_v2(&stdout)?;
    if !relative_path.is_empty() {
        let prefix = format!("{relative_path}/");
        for changed_path in &mut snapshot.changed_paths {
            let path = match changed_path {
                GitChangedPath::Ordinary { path, .. }
                | GitChangedPath::Unmerged { path, .. }
                | GitChangedPath::Untracked { path } => path,
                GitChangedPath::Renamed {
                    path,
                    previous_path,
                    ..
                } => {
                    if let Some(stripped) = previous_path.strip_prefix(&prefix) {
                        *previous_path = stripped.to_string();
                    }
                    path
                }
                GitChangedPath::Copied {
                    path, source_path, ..
                } => {
                    if let Some(stripped) = source_path.strip_prefix(&prefix) {
                        *source_path = stripped.to_string();
                    }
                    path
                }
            };
            *path = path
                .strip_prefix(&prefix)
                .ok_or_else(|| "git status returned a path outside the requested root".to_string())?
                .to_string();
        }
    }
    Ok(snapshot)
}

fn checkout_status_from_live_observation(worktree_path: &FsPath) -> CheckoutStatus {
    match phoenix_core::git::observe_local_git_head(worktree_path) {
        phoenix_core::domain::observed_branch::LocalGitHeadObservation::NamedBranch {
            branch_name,
            head_oid,
            ..
        } => {
            let live = live_checkout_observation_details(worktree_path, branch_name, &head_oid);
            CheckoutStatus::NamedBranch {
                branch_name: live.branch_name,
                head_oid,
                remote_status: live.remote_status,
            }
        }
        phoenix_core::domain::observed_branch::LocalGitHeadObservation::Detached {
            head_oid,
            ..
        } => CheckoutStatus::Detached {
            pointing_refs: detached_pointing_refs(worktree_path, &head_oid),
            head_oid,
        },
        phoenix_core::domain::observed_branch::LocalGitHeadObservation::Unborn {
            branch_name,
            ..
        } => CheckoutStatus::Unborn { branch_name },
        phoenix_core::domain::observed_branch::LocalGitHeadObservation::Unavailable {
            error,
            ..
        } => {
            tracing::warn!(%error, "failed to observe worktree checkout");
            CheckoutStatus::Unavailable {
                reason: "Checkout status is unavailable.".to_string(),
            }
        }
    }
}

fn same_checkout(left: &CheckoutStatus, right: &CheckoutStatus) -> bool {
    match (left, right) {
        (
            CheckoutStatus::NamedBranch {
                branch_name: left_branch,
                head_oid: left_oid,
                ..
            },
            CheckoutStatus::NamedBranch {
                branch_name: right_branch,
                head_oid: right_oid,
                ..
            },
        ) => left_branch == right_branch && left_oid == right_oid,
        (
            CheckoutStatus::Detached {
                head_oid: left_oid, ..
            },
            CheckoutStatus::Detached {
                head_oid: right_oid,
                ..
            },
        ) => left_oid == right_oid,
        (
            CheckoutStatus::Unborn {
                branch_name: left_branch,
                ..
            },
            CheckoutStatus::Unborn {
                branch_name: right_branch,
                ..
            },
        ) => left_branch == right_branch,
        (CheckoutStatus::Unavailable { .. }, CheckoutStatus::Unavailable { .. }) => true,
        _ => false,
    }
}

fn capture_with_stable_checkout<T>(
    worktree_path: &FsPath,
    mut capture: impl FnMut() -> Result<T, AppError>,
) -> Result<(T, CheckoutStatus), AppError> {
    for attempt in 0..CHECKOUT_CAPTURE_ATTEMPTS {
        let before = checkout_status_from_live_observation(worktree_path);
        let captured = capture()?;
        let after = checkout_status_from_live_observation(worktree_path);
        if same_checkout(&before, &after) {
            return Ok((captured, after));
        }
        if attempt + 1 == CHECKOUT_CAPTURE_ATTEMPTS {
            tracing::warn!("worktree checkout changed repeatedly while capturing diff");
            return Err(AppError::Internal(
                "The worktree checkout changed while the diff was captured. Reload to retry."
                    .to_string(),
            ));
        }
    }
    unreachable!("checkout capture loop always returns")
}

fn active_pr_diff_repo_mismatch_reason(
    worktree_repo_identity: Option<&str>,
    selected_repo_identity: &str,
) -> Option<String> {
    match worktree_repo_identity {
        Some(worktree_repo_identity)
            if github_repo_identity_eq(worktree_repo_identity, selected_repo_identity) => None,
        Some(repo) => Some(format!(
            "PR-specific diff unavailable for selected repository {selected_repo_identity}; local worktree is attached to {repo}"
        )),
        None => Some(format!(
            "PR-specific diff unavailable for selected repository {selected_repo_identity}; local worktree origin is not a GitHub repository"
        )),
    }
}

fn capture_active_pr_diff(
    worktree_path: &FsPath,
    active_pr: &crate::db::WorkScopePrAssociation,
    max_diff_bytes: usize,
) -> Result<crate::git_ops::CapturedDiff, AppError> {
    capture_active_pr_diff_for_repo_identity(
        worktree_path,
        github_repo_identifier_from_worktree(worktree_path).as_deref(),
        active_pr,
        max_diff_bytes,
    )
}

fn capture_active_pr_diff_for_repo_identity(
    worktree_path: &FsPath,
    worktree_repo_identity: Option<&str>,
    active_pr: &crate::db::WorkScopePrAssociation,
    max_diff_bytes: usize,
) -> Result<crate::git_ops::CapturedDiff, AppError> {
    let selected_repo_identity = format!("{}/{}", active_pr.repo_owner, active_pr.repo_name);
    if let Some(reason) =
        active_pr_diff_repo_mismatch_reason(worktree_repo_identity, &selected_repo_identity)
    {
        return Err(AppError::BadRequest(reason));
    }
    let checked_out_branch = run_git(
        worktree_path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .ok()
    .map(|branch| branch.trim().to_string());
    if checked_out_branch.as_deref() != Some(active_pr.head.as_str()) {
        return Err(AppError::BadRequest(format!(
            "PR-specific diff unavailable until selected head {} is checked out; current branch is {}",
            active_pr.head,
            checked_out_branch.as_deref().unwrap_or("detached or unborn")
        )));
    }
    Ok(capture_branch_diff(
        worktree_path,
        &active_pr.base,
        max_diff_bytes,
    ))
}

fn associated_pr_summary_response(
    pr: crate::db::WorkScopePrAssociation,
) -> AssociatedPrSummaryResponse {
    AssociatedPrSummaryResponse {
        repo_owner: pr.repo_owner,
        repo_name: pr.repo_name,
        pr_number: pr.pr_number,
        title: pr.title,
        url: pr.url,
        state: pr.state,
        draft: pr.draft,
        display_state: pr.display_state,
        base: pr.base,
        head: pr.head,
        github_updated_at: pr.github_updated_at,
        feedback_status: pr.feedback_status,
    }
}

async fn selection_envelope_for_scope(
    db: &crate::db::Database,
    work_scope: &phoenix_core::work_scope::WorkScopeId,
) -> Result<AssociatedPrStatusEnvelope, AppError> {
    Ok(selection_envelope_for_scope_from_snapshot(
        db.list_work_scope_pr_associations(work_scope)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        db.active_work_scope_pr_selection(work_scope)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        db.list_work_scope_observed_branches(work_scope)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
    ))
}

fn selection_envelope_for_scope_from_snapshot(
    associated: Vec<crate::db::WorkScopePrAssociation>,
    active: Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>,
    observed: Vec<crate::db::WorkScopeObservedBranch>,
) -> AssociatedPrStatusEnvelope {
    let associated_prs = associated
        .into_iter()
        .map(associated_pr_summary_response)
        .collect();
    let active_pr = active
        .and_then(|state| state.selection)
        .map(active_pr_selection_response);
    let latest_observed_branch =
        observed
            .into_iter()
            .next()
            .map(|branch| ObservedBranchSummaryResponse {
                repository_identity: branch.repository_identity,
                branch_name: branch.branch_name,
            });
    AssociatedPrStatusEnvelope {
        associated_prs,
        active_pr,
        latest_observed_branch,
    }
}

fn response_identity_matches_association(
    response: &PrStatusResponse,
    observations: &[crate::db::WorkScopePrObservation],
    association: &crate::db::WorkScopePrAssociation,
) -> bool {
    response.number == Some(association.pr_number)
        && observations.iter().any(|pr| {
            pr.repo_owner == association.repo_owner
                && pr.repo_name == association.repo_name
                && pr.pr_number == association.pr_number
        })
}

fn refreshed_pr_association(
    response: &PrStatusResponse,
    observations: &[crate::db::WorkScopePrObservation],
    associations: &[crate::db::WorkScopePrAssociation],
) -> Option<crate::db::WorkScopePrAssociation> {
    observations
        .iter()
        .find(|observation| response.number == Some(observation.pr_number))
        .and_then(|observation| {
            associations.iter().find(|association| {
                association
                    .repo_owner
                    .eq_ignore_ascii_case(&observation.repo_owner)
                    && association
                        .repo_name
                        .eq_ignore_ascii_case(&observation.repo_name)
                    && association.pr_number == observation.pr_number
            })
        })
        .cloned()
}

fn association_for_artifact_baseline<'a>(
    associations: &'a [crate::db::WorkScopePrAssociation],
    baseline: &crate::db::WorkScopePrFeedbackBaselineInput,
) -> Result<&'a crate::db::WorkScopePrAssociation, AppError> {
    let legacy_identity = baseline.repo_owner.is_empty() && baseline.repo_name.is_empty();
    if baseline.repo_owner.is_empty() != baseline.repo_name.is_empty() {
        return Err(AppError::BadRequest(
            "PR context artifact has an incomplete repository identity".to_string(),
        ));
    }

    let matches = associations
        .iter()
        .filter(|association| {
            association.pr_number == baseline.pr_number
                && (legacy_identity
                    || (association
                        .repo_owner
                        .eq_ignore_ascii_case(&baseline.repo_owner)
                        && association
                            .repo_name
                            .eq_ignore_ascii_case(&baseline.repo_name)))
        })
        .collect::<Vec<_>>();

    if legacy_identity {
        return match matches.as_slice() {
            [association] => Ok(*association),
            [] => Err(AppError::BadRequest(
                "PR context artifact no longer matches an associated PR".to_string(),
            )),
            _ => Err(AppError::BadRequest(
                "Legacy PR context artifact is ambiguous across associated repositories"
                    .to_string(),
            )),
        };
    }

    matches.into_iter().next().ok_or_else(|| {
        AppError::BadRequest("PR context artifact no longer matches an associated PR".to_string())
    })
}

async fn active_selection_target_for_scope(
    db: &crate::db::Database,
    work_scope: &phoenix_core::work_scope::WorkScopeId,
) -> Result<Option<crate::db::WorkScopePrAssociation>, AppError> {
    let Some(selection) = db
        .active_work_scope_pr_selection(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .and_then(|state| state.selection)
    else {
        return Ok(None);
    };
    Ok(db
        .list_work_scope_pr_associations(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .into_iter()
        .find(|pr| {
            pr.repo_owner == selection.pr.repo_owner
                && pr.repo_name == selection.pr.repo_name
                && pr.pr_number == selection.pr.pr_number
        }))
}

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
async fn pr_status_response_for_missing_worktree(
    state: &AppState,
    work_scope: &phoenix_core::work_scope::WorkScopeId,
) -> Result<PrStatusResponse, AppError> {
    let attempted_at = chrono::Utc::now().to_rfc3339();
    let associated = state
        .runtime
        .db()
        .list_work_scope_pr_associations(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let active = state
        .runtime
        .db()
        .active_work_scope_pr_selection(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let observed = state
        .runtime
        .db()
        .list_work_scope_observed_branches(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let envelope =
        selection_envelope_for_scope_from_snapshot(associated.clone(), active.clone(), observed);
    let compatibility_primary = state
        .runtime
        .db()
        .primary_work_scope_pr_association(work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut response = match active.and_then(|state| state.selection) {
        Some(selection) => {
            let Some(pr) = associated.into_iter().find(|pr| {
                pr.repo_owner == selection.pr.repo_owner
                    && pr.repo_name == selection.pr.repo_name
                    && pr.pr_number == selection.pr.pr_number
            }) else {
                let mut response = PrStatusResponse::unavailable(PrUnavailableReason::NotGitRepo);
                response.selection = envelope.clone();
                response.work_change = WorkChangeSummary::Unavailable {
                    reason: "worktree path is not a directory".to_string(),
                };
                return Ok(response);
            };
            crate::api::pr_monitoring::stale_response(
                pr,
                PrUnavailableReason::NotGitRepo,
                attempted_at,
            )
        }
        None => compatibility_primary.map_or_else(
            || PrStatusResponse::unavailable(PrUnavailableReason::NotGitRepo),
            |pr| {
                crate::api::pr_monitoring::stale_response(
                    pr,
                    PrUnavailableReason::NotGitRepo,
                    attempted_at,
                )
            },
        ),
    };
    response.work_change = WorkChangeSummary::Unavailable {
        reason: "worktree path is not a directory".to_string(),
    };
    response.selection = envelope;
    Ok(response)
}

fn effective_feedback_status_for_cache(
    previous: Option<PrFeedbackStatus>,
    fetched: PrFeedbackStatus,
) -> (Option<PrFeedbackStatus>, bool) {
    (Some(fetched), previous != Some(fetched))
}

#[allow(clippy::too_many_lines)]
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
            conv.work_scope_id
                .clone()
                .expect("persisted conversation has work scope"),
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
    let refresh_generation = db
        .active_work_scope_pr_selection(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_or(0, |state| state.inference_generation);
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
    .instrument(tracing::info_span!(
        target: "phoenix_ide::otel",
        "pr_status.refresh",
        operation = "branch_and_work_change",
    ))
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;

    if !refresh.observations.is_empty() {
        db.upsert_work_scope_pr_observations(&work_scope, &refresh.observations)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let latest_branch = db
        .list_work_scope_observed_branches(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .into_iter()
        .next()
        .map(
            |branch| phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                repository_identity: branch.repository_identity,
                branch_name: branch.branch_name,
            },
        );
    db.derive_active_work_scope_pr_selection(
        &work_scope,
        &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
            latest_observed_branch: latest_branch,
        },
        Some(refresh_generation),
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut associated_snapshot = db
        .list_work_scope_pr_associations(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut active_snapshot = db
        .active_work_scope_pr_selection(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut observed_snapshot = db
        .list_work_scope_observed_branches(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let selection = active_snapshot
        .clone()
        .and_then(|state| state.selection)
        .and_then(|selection| {
            associated_snapshot
                .iter()
                .find(|pr| {
                    pr.repo_owner == selection.pr.repo_owner
                        && pr.repo_name == selection.pr.repo_name
                        && pr.pr_number == selection.pr.pr_number
                })
                .cloned()
        });

    let mut response = if let Some(active_pr) = selection {
        let needs_direct_active_refresh = refresh.response.refresh.state
            != crate::api::types::PrRefreshState::Fresh
            || refresh.response.refresh.stale
            || !response_identity_matches_association(
                &refresh.response,
                &refresh.observations,
                &active_pr,
            );
        if needs_direct_active_refresh {
            let active_refresh = tokio::task::spawn_blocking({
                let cwd = cwd.clone();
                let repo_owner = active_pr.repo_owner.clone();
                let repo_name = active_pr.repo_name.clone();
                let pr_number = active_pr.pr_number;
                move || {
                    crate::api::pr_monitoring::get_pr_status_for_pr(
                        &cwd,
                        &repo_owner,
                        &repo_name,
                        pr_number,
                    )
                }
            })
            .instrument(tracing::info_span!(
                target: "phoenix_ide::otel",
                "pr_status.refresh",
                operation = "active_pr",
            ))
            .await
            .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;
            if !active_refresh.observations.is_empty() {
                db.upsert_work_scope_pr_observations(&work_scope, &active_refresh.observations)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                observed_snapshot = db
                    .list_work_scope_observed_branches(&work_scope)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                let latest_branch = observed_snapshot.first().map(|branch| {
                    phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                        repository_identity: branch.repository_identity.clone(),
                        branch_name: branch.branch_name.clone(),
                    }
                });
                db.derive_active_work_scope_pr_selection(
                    &work_scope,
                    &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                        latest_observed_branch: latest_branch,
                    },
                    None,
                )
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
                associated_snapshot = db
                    .list_work_scope_pr_associations(&work_scope)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                active_snapshot = db
                    .active_work_scope_pr_selection(&work_scope)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                observed_snapshot = db
                    .list_work_scope_observed_branches(&work_scope)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
            }
            let refreshed_active_pr = active_snapshot
                .clone()
                .and_then(|state| state.selection)
                .and_then(|selection| {
                    associated_snapshot
                        .iter()
                        .find(|pr| {
                            pr.repo_owner == selection.pr.repo_owner
                                && pr.repo_name == selection.pr.repo_name
                                && pr.pr_number == selection.pr.pr_number
                        })
                        .cloned()
                })
                .unwrap_or_else(|| active_pr.clone());
            let refreshed_identity_changed = !refreshed_active_pr
                .repo_owner
                .eq_ignore_ascii_case(&active_pr.repo_owner)
                || !refreshed_active_pr
                    .repo_name
                    .eq_ignore_ascii_case(&active_pr.repo_name)
                || refreshed_active_pr.pr_number != active_pr.pr_number;
            if refreshed_identity_changed {
                let retargeted_refresh = tokio::task::spawn_blocking({
                    let cwd = cwd.clone();
                    let repo_owner = refreshed_active_pr.repo_owner.clone();
                    let repo_name = refreshed_active_pr.repo_name.clone();
                    let pr_number = refreshed_active_pr.pr_number;
                    move || {
                        crate::api::pr_monitoring::get_pr_status_for_pr(
                            &cwd,
                            &repo_owner,
                            &repo_name,
                            pr_number,
                        )
                    }
                })
                .instrument(tracing::info_span!(
                    target: "phoenix_ide::otel",
                    "pr_status.refresh",
                    operation = "retargeted_pr",
                ))
                .await
                .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;
                if !retargeted_refresh.observations.is_empty() {
                    db.upsert_work_scope_pr_observations(
                        &work_scope,
                        &retargeted_refresh.observations,
                    )
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                    associated_snapshot = db
                        .list_work_scope_pr_associations(&work_scope)
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    active_snapshot = db
                        .active_work_scope_pr_selection(&work_scope)
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    observed_snapshot = db
                        .list_work_scope_observed_branches(&work_scope)
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                }
                attach_pr_feedback_freshness(
                    retargeted_refresh.response,
                    &db,
                    &work_scope,
                    &cwd,
                    &refreshed_active_pr,
                )
                .await?
            } else if active_refresh.response.refresh.state
                == crate::api::types::PrRefreshState::Fresh
            {
                attach_pr_feedback_freshness(
                    active_refresh.response,
                    &db,
                    &work_scope,
                    &cwd,
                    &refreshed_active_pr,
                )
                .await?
            } else {
                crate::api::pr_monitoring::persisted_primary_response(
                    &refreshed_active_pr,
                    active_refresh.response.refresh,
                    true,
                )
            }
        } else {
            attach_pr_feedback_freshness(refresh.response, &db, &work_scope, &cwd, &active_pr)
                .await?
        }
    } else {
        refresh.response
    };
    response.selection = selection_envelope_for_scope_from_snapshot(
        associated_snapshot,
        active_snapshot,
        observed_snapshot,
    );

    Ok(Json(response))
}

#[allow(clippy::too_many_lines)]
async fn attach_pr_feedback_freshness(
    mut response: PrStatusResponse,
    db: &crate::db::Database,
    work_scope: &phoenix_core::work_scope::WorkScopeId,
    cwd: &FsPath,
    active_pr: &crate::db::WorkScopePrAssociation,
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

    let previous_feedback_status =
        (active_pr.pr_number == pr_number).then_some(active_pr.feedback_status);

    let baseline = db
        .work_scope_pr_feedback_baseline(
            work_scope,
            &active_pr.repo_owner,
            &active_pr.repo_name,
            pr_number,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if baseline.as_ref().is_some_and(|baseline| {
        crate::api::pr_monitoring::pr_updated_after_baseline(baseline, updated_at)
    }) {
        let cwd_for_feedback = cwd.to_path_buf();
        let repo_owner = active_pr.repo_owner.clone();
        let repo_name = active_pr.repo_name.clone();
        let feedback = tokio::task::spawn_blocking(move || {
            crate::api::pr_monitoring::fetch_pr_feedback_for_pr(
                &cwd_for_feedback,
                &repo_owner,
                &repo_name,
                pr_number,
            )
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;
        match feedback {
            Ok(feedback) => {
                let coverage_health = crate::api::pr_monitoring::coverage_health(&feedback);
                response.feedback_status = feedback.feedback_status.or(previous_feedback_status);
                if let Some(fetched_status) = feedback.feedback_status {
                    let (_, should_update_cache) = effective_feedback_status_for_cache(
                        previous_feedback_status,
                        fetched_status,
                    );
                    if should_update_cache {
                        db.update_work_scope_pr_feedback_status(
                            work_scope,
                            &active_pr.repo_owner,
                            &active_pr.repo_name,
                            pr_number,
                            fetched_status,
                        )
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    }
                }
                response.feedback_freshness =
                    crate::api::pr_monitoring::actionable_feedback_freshness_from_baseline(
                        baseline
                            .as_ref()
                            .expect("baseline exists in full feedback branch"),
                        Some(&feedback),
                    );
                response.feedback_coverage = coverage_health;
            }
            Err(err) => {
                tracing::debug!(pr = pr_number, error = %err, "could not fetch PR feedback to classify freshness");
            }
        }
    } else {
        let cwd_for_status = cwd.to_path_buf();
        let repo_owner = active_pr.repo_owner.clone();
        let repo_name = active_pr.repo_name.clone();
        let status = tokio::task::spawn_blocking(move || {
            crate::api::pr_monitoring::fetch_pr_feedback_status_for_pr(
                &cwd_for_status,
                &repo_owner,
                &repo_name,
                pr_number,
            )
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;
        match status {
            Ok(status) => {
                response.feedback_status = Some(status);
                if Some(status) != previous_feedback_status {
                    db.update_work_scope_pr_feedback_status(
                        work_scope,
                        &active_pr.repo_owner,
                        &active_pr.repo_name,
                        pr_number,
                        status,
                    )
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                }
            }
            Err(err) => {
                tracing::debug!(pr = pr_number, error = %err, "could not fetch bounded PR feedback reaction status");
            }
        }
    }

    Ok(response)
}

#[allow(clippy::too_many_lines)]
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
            conv.work_scope_id
                .clone()
                .expect("persisted conversation has work scope"),
        ),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode (no associated PR)".to_string(),
            ));
        }
    };

    let db = state.runtime.db().clone();
    let active_pr =
        if let Some(active_pr) = active_selection_target_for_scope(&db, &work_scope).await? {
            active_pr
        } else {
            let worktree = PathBuf::from(&worktree_path);
            let refresh = tokio::task::spawn_blocking({
                let branch_name = branch_name.clone();
                move || crate::api::pr_monitoring::get_pr_status_for_branch(&worktree, &branch_name)
            })
            .await
            .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;
            if !refresh.observations.is_empty() {
                db.upsert_work_scope_pr_observations(&work_scope, &refresh.observations)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
            }
            let associations = db
                .list_work_scope_pr_associations(&work_scope)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let discovered =
                refreshed_pr_association(&refresh.response, &refresh.observations, &associations);
            match discovered {
                Some(pr) => pr,
                None => db
                    .primary_work_scope_pr_association(&work_scope)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?
                    .ok_or_else(|| {
                        AppError::BadRequest(
                            "PR-specific action unavailable until a PR is associated with this work"
                                .to_string(),
                        )
                    })?,
            }
        };

    let target_repo_owner = active_pr.repo_owner.clone();
    let target_repo_name = active_pr.repo_name.clone();
    let target_pr_number = active_pr.pr_number;
    let result = tokio::task::spawn_blocking(move || {
        let worktree = PathBuf::from(worktree_path);
        crate::api::pr_monitoring::capture_pr_auto_fix_context_for_pr(
            &worktree,
            &target_repo_owner,
            &target_repo_name,
            target_pr_number,
            conv.llm_language,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?;

    let (response, observations, _result_baseline, feedback_status) = match result {
        Ok(capture) => (
            Ok(capture.response),
            capture.observations,
            Some(capture.baseline),
            capture.feedback_status,
        ),
        Err(crate::api::pr_monitoring::PrMonitorError::BadRequestWithObservations {
            message,
            observations,
        }) => (Err(AppError::BadRequest(message)), observations, None, None),
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

    if let (Ok(response), Some(feedback_status)) = (&response, feedback_status) {
        db.update_work_scope_pr_feedback_status(
            &work_scope,
            &response.repo_owner,
            &response.repo_name,
            response.pr_number,
            feedback_status,
        )
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
    if !matches!(
        conv.conv_mode,
        ConvMode::Work { .. } | ConvMode::Branch { .. }
    ) {
        return Ok(());
    }
    let work_scope = conv
        .work_scope_id
        .clone()
        .expect("persisted conversation has work scope");
    let artifact = crate::api::pr_monitoring::read_pr_auto_fix_context_artifact(
        &pr_auto_fix_artifact_path(&conv, artifact_path)?,
    )
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let artifact_baseline = artifact.baseline();
    let associations = db
        .list_work_scope_pr_associations(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let association = association_for_artifact_baseline(&associations, &artifact_baseline)?;
    let baseline =
        artifact.baseline_for_repository(&association.repo_owner, &association.repo_name);
    db.upsert_work_scope_pr_feedback_baseline(&work_scope, &baseline)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(())
}

pub(crate) async fn pin_associated_pr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PinAssociatedPrRequest>,
) -> Result<(StatusCode, Json<ActivePrSelectionMutationResponse>), AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let work_scope = match &conv.conv_mode {
        ConvMode::Work { .. } | ConvMode::Branch { .. } => conv
            .work_scope_id
            .clone()
            .expect("persisted conversation has work scope"),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode".to_string(),
            ));
        }
    };
    let active = state
        .runtime
        .db()
        .pin_active_work_scope_pr_selection(
            &work_scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: request.repo_owner,
                repo_name: request.repo_name,
                pr_number: request.pr_number,
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let latest = selection_envelope_for_scope(state.runtime.db(), &work_scope).await?;
    Ok((
        StatusCode::OK,
        Json(ActivePrSelectionMutationResponse {
            active_pr: active.selection.map(active_pr_selection_response),
            latest_observed_branch: latest.latest_observed_branch,
        }),
    ))
}

pub(crate) async fn resume_associated_pr_inference(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ActivePrSelectionMutationResponse>), AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let work_scope = match &conv.conv_mode {
        ConvMode::Work { .. } | ConvMode::Branch { .. } => conv
            .work_scope_id
            .clone()
            .expect("persisted conversation has work scope"),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode".to_string(),
            ));
        }
    };
    let latest_durable_branch = state
        .runtime
        .db()
        .list_work_scope_observed_branches(&work_scope)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .into_iter()
        .next()
        .map(
            |branch| phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                repository_identity: branch.repository_identity,
                branch_name: branch.branch_name,
            },
        );
    let active = state
        .runtime
        .db()
        .clear_active_work_scope_pr_pin(
            &work_scope,
            &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                latest_observed_branch: latest_durable_branch,
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let latest = selection_envelope_for_scope(state.runtime.db(), &work_scope).await?;
    Ok((
        StatusCode::OK,
        Json(ActivePrSelectionMutationResponse {
            active_pr: active
                .and_then(|state| state.selection)
                .map(active_pr_selection_response),
            latest_observed_branch: latest.latest_observed_branch,
        }),
    ))
}

/// `GET /api/conversations/:id/active-pr/diff` — committed and uncommitted changes
/// in the conversation's worktree, compared against the explicit active PR's
/// actual base branch. Read-only; used by the PR-specific diff action.
pub(crate) async fn get_active_pr_diff(
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

    let worktree_path = match &conv.conv_mode {
        ConvMode::Work { worktree_path, .. } | ConvMode::Branch { worktree_path, .. } => {
            worktree_path.to_string()
        }
        _ => {
            return Err(AppError::BadRequest(
                "Conversation is not in Work or Branch mode (no worktree to diff)".to_string(),
            ));
        }
    };
    let work_scope = conv
        .work_scope_id
        .clone()
        .expect("persisted conversation has work scope");
    let active_pr = active_selection_target_for_scope(state.runtime.db(), &work_scope)
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(
                "PR-specific diff unavailable until an active PR is selected".to_string(),
            )
        })?;
    let pr_number = active_pr.pr_number;

    tokio::task::spawn_blocking(move || {
        let wt = PathBuf::from(&worktree_path);
        if !wt.exists() {
            return Err(AppError::NotFound(format!(
                "Worktree no longer exists: {worktree_path}"
            )));
        }

        let (captured, checkout_status) = capture_with_stable_checkout(&wt, || {
            capture_active_pr_diff(&wt, &active_pr, MAX_DIFF_BYTES)
        })?;
        Ok(build_diff_response(
            captured,
            format!("PR #{pr_number} Diff"),
            "active_pr",
            Some(pr_number),
            checkout_status,
        ))
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?
    .map(Json)
}

/// `GET /api/conversations/:id/diff` — committed and uncommitted changes
/// in the conversation's worktree, vs the base branch. Read-only; used by
/// the Work/Branch-mode workspace diff action so users can review before
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

        let (captured, checkout_status) = capture_with_stable_checkout(&wt, || {
            Ok(capture_branch_diff(&wt, &base_branch, MAX_DIFF_BYTES))
        })?;

        Ok(build_diff_response(
            captured,
            "Workspace Diff".to_string(),
            "workspace",
            None,
            checkout_status,
        ))
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))?
    .map(Json)
}

pub(crate) async fn get_conversation_git_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConversationGitStatusResponse>, AppError> {
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    let file_root = std::path::PathBuf::from(conv.file_root());
    tokio::task::spawn_blocking(move || {
        if !file_root.exists() {
            return Err(AppError::NotFound(format!(
                "Conversation file root no longer exists: {}",
                file_root.display()
            )));
        }
        match capture_with_stable_checkout(&file_root, || {
            capture_git_status_snapshot(&file_root).map_err(AppError::Internal)
        }) {
            Ok((snapshot, checkout_status)) => match checkout_status {
                CheckoutStatus::NamedBranch { .. }
                | CheckoutStatus::Detached { .. }
                | CheckoutStatus::Unborn { .. } => Ok(ConversationGitStatusResponse::Snapshot {
                    checkout_status,
                    counts: snapshot.counts,
                    changed_paths: snapshot.changed_paths,
                }),
                CheckoutStatus::Unavailable { ref reason } => {
                    Ok(ConversationGitStatusResponse::Unavailable {
                        reason: reason.clone(),
                        checkout_status: Some(checkout_status),
                    })
                }
            },
            Err(error) => {
                let error = match error {
                    AppError::Internal(message) => message,
                    other => format!("{other:?}"),
                };
                let lower = error.to_ascii_lowercase();
                if lower.contains("not a git repository") || lower.contains("outside repository") {
                    Ok(ConversationGitStatusResponse::NonGit)
                } else {
                    tracing::warn!(%error, path = %file_root.display(), "failed to capture git status snapshot");
                    Ok(ConversationGitStatusResponse::Unavailable {
                        reason: "Git status is unavailable.".to_string(),
                        checkout_status: None,
                    })
                }
            }
        }
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
    fn successful_root_status_clears_stale_cache_independent_of_feedback_coverage() {
        assert_eq!(
            effective_feedback_status_for_cache(
                Some(PrFeedbackStatus::InProgress),
                PrFeedbackStatus::Open,
            ),
            (Some(PrFeedbackStatus::Open), true)
        );
    }

    #[test]
    fn unchanged_root_status_skips_cache_write() {
        assert_eq!(
            effective_feedback_status_for_cache(
                Some(PrFeedbackStatus::Approved),
                PrFeedbackStatus::Approved,
            ),
            (Some(PrFeedbackStatus::Approved), false)
        );
    }

    #[test]
    fn unavailable_root_status_falls_back_to_cached_response_status() {
        let previous = Some(PrFeedbackStatus::InProgress);
        let fetched = None;

        assert_eq!(fetched.or(previous), Some(PrFeedbackStatus::InProgress));
    }

    #[test]
    fn successful_open_root_status_overrides_cached_response_status() {
        let previous = Some(PrFeedbackStatus::InProgress);
        let fetched = Some(PrFeedbackStatus::Open);

        assert_eq!(fetched.or(previous), Some(PrFeedbackStatus::Open));
    }

    fn association(owner: &str, repo: &str, number: u64) -> crate::db::WorkScopePrAssociation {
        crate::db::WorkScopePrAssociation {
            work_scope_id: crate::work_scope::WorkScopeId::parse("scope-1").unwrap(),
            repo_owner: owner.to_string(),
            repo_name: repo.to_string(),
            pr_number: number,
            title: format!("PR {number}"),
            url: format!("https://example.test/{owner}/{repo}/{number}"),
            state: "OPEN".to_string(),
            draft: false,
            display_state: crate::api::types::PrDisplayState::Open,
            base: "main".to_string(),
            head: format!("feature/{number}"),
            github_updated_at: None,
            feedback_status: PrFeedbackStatus::Open,
            first_seen_at: "2024-01-01T00:00:00Z".to_string(),
            last_seen_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    fn observation(owner: &str, repo: &str, number: u64) -> crate::db::WorkScopePrObservation {
        crate::db::WorkScopePrObservation {
            repo_owner: owner.to_string(),
            repo_name: repo.to_string(),
            pr_number: number,
            title: format!("PR {number}"),
            url: format!("https://example.test/{owner}/{repo}/{number}"),
            state: "OPEN".to_string(),
            draft: false,
            display_state: crate::api::types::PrDisplayState::Open,
            base: "main".to_string(),
            head: format!("feature/{number}"),
            github_updated_at: None,
        }
    }

    fn baseline(
        owner: &str,
        repo: &str,
        number: u64,
    ) -> crate::db::WorkScopePrFeedbackBaselineInput {
        crate::db::WorkScopePrFeedbackBaselineInput {
            repo_owner: owner.to_string(),
            repo_name: repo.to_string(),
            pr_number: number,
            captured_at: "2024-01-01T00:00:00Z".to_string(),
            github_updated_at: None,
            feedback_identities: Vec::new(),
            feedback_fingerprints: Vec::new(),
        }
    }

    #[test]
    fn refreshed_pr_precedes_older_ranked_primary_association() {
        let mut response = PrStatusResponse::not_found();
        response.number = Some(22);
        let associations = vec![
            association("acme", "old", 11),
            association("Acme", "New", 22),
        ];
        let observations = vec![observation("acme", "new", 22)];

        let selected = refreshed_pr_association(&response, &observations, &associations).unwrap();

        assert_eq!(
            (selected.repo_name.as_str(), selected.pr_number),
            ("New", 22)
        );
    }

    #[test]
    fn complete_artifact_identity_selects_exact_repository_case_insensitively() {
        let associations = vec![
            association("other", "repo", 7),
            association("Acme", "App", 7),
        ];

        let selected =
            association_for_artifact_baseline(&associations, &baseline("acme", "app", 7)).unwrap();

        assert_eq!(
            (selected.repo_owner.as_str(), selected.repo_name.as_str()),
            ("Acme", "App")
        );
    }

    #[test]
    fn legacy_artifact_identity_selects_unique_number_without_active_selection() {
        let associations = vec![association("acme", "app", 7), association("acme", "app", 8)];

        let selected =
            association_for_artifact_baseline(&associations, &baseline("", "", 7)).unwrap();

        assert_eq!(selected.pr_number, 7);
    }

    #[test]
    fn legacy_artifact_identity_rejects_same_number_across_repositories() {
        let associations = vec![
            association("acme", "app", 7),
            association("other", "repo", 7),
        ];

        let error =
            association_for_artifact_baseline(&associations, &baseline("", "", 7)).unwrap_err();

        assert!(matches!(error, AppError::BadRequest(message) if message.contains("ambiguous")));
    }

    fn conversation_with_mode(
        cwd: &std::path::Path,
        conv_mode: ConvMode,
    ) -> crate::db::Conversation {
        let now = chrono::Utc::now();
        crate::db::Conversation {
            work_scope_id: Some(crate::work_scope::WorkScopeId::parse("test-work").unwrap()),
            runtime_role: crate::work_scope::RuntimeRole::User,
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
            transcript_generation: 1,
            model: None,
            project_id: None,
            conv_mode,
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
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

    fn clone_repo(source: &std::path::Path, dest: &std::path::Path) {
        run_git(dest, &["clone", "--quiet", source.to_str().unwrap(), "."]).unwrap();
        run_git(dest, &["config", "user.email", "probe@test"]).unwrap();
        run_git(dest, &["config", "user.name", "probe"]).unwrap();
    }

    #[test]
    fn parses_porcelain_v2_records_and_counts() {
        let output = concat!(
            "1 MM N... 100644 100644 100644 1111111111111111111111111111111111111111 1111111111111111111111111111111111111111 src/lib.rs\0",
            "1 D. N... 100644 000000 000000 2222222222222222222222222222222222222222 0000000000000000000000000000000000000000 deleted.txt\0",
            "2 R. N... 100644 100644 100644 3333333333333333333333333333333333333333 3333333333333333333333333333333333333333 R100 renamed new.txt\0old name.txt\0",
            "u UU N... 100644 100644 100644 100644 4444444444444444444444444444444444444444 5555555555555555555555555555555555555555 6666666666666666666666666666666666666666 conflicted.txt\0",
            "? untracked/é.txt\0",
            "! ignored.tmp\0"
        )
        .as_bytes();

        let parsed = parse_git_status_porcelain_v2(output).unwrap();
        assert_eq!(parsed.counts.changed_paths, 5);
        assert_eq!(parsed.counts.staged_paths, 3);
        assert_eq!(parsed.counts.unstaged_paths, 1);
        assert_eq!(parsed.counts.untracked_paths, 1);
        assert_eq!(parsed.counts.conflicted_paths, 1);
        assert!(matches!(
            &parsed.changed_paths[2],
            GitChangedPath::Renamed {
                path,
                previous_path,
                worktree_status: GitFileStatus::Unmodified,
            } if path == "renamed new.txt" && previous_path == "old name.txt"
        ));
    }

    #[test]
    fn snapshot_excludes_ignored_and_keeps_all_untracked_files() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(
            repo.path().join(".gitignore"),
            "ignored-dir/\nignored.log\n",
        )
        .unwrap();
        run_git(repo.path(), &["add", ".gitignore"]).unwrap();
        run_git(repo.path(), &["commit", "-q", "-m", "ignore"]).unwrap();
        std::fs::create_dir_all(repo.path().join("untracked-dir")).unwrap();
        std::fs::write(repo.path().join("untracked-dir/a.txt"), "a").unwrap();
        std::fs::write(repo.path().join("untracked-dir/b.txt"), "b").unwrap();
        std::fs::create_dir_all(repo.path().join("ignored-dir")).unwrap();
        std::fs::write(repo.path().join("ignored-dir/c.txt"), "c").unwrap();
        std::fs::write(repo.path().join("ignored.log"), "log").unwrap();

        let snapshot = capture_git_status_snapshot(repo.path()).unwrap();
        assert_eq!(snapshot.counts.changed_paths, 2);
        assert_eq!(snapshot.counts.untracked_paths, 2);
        assert_eq!(
            snapshot
                .changed_paths
                .iter()
                .filter_map(|entry| match entry {
                    GitChangedPath::Untracked { path } => Some(path.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec!["untracked-dir/a.txt", "untracked-dir/b.txt"]
        );
    }

    #[test]
    fn snapshot_treats_scoped_pathspec_metacharacters_as_literals() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::create_dir_all(repo.path().join("scope*literal")).unwrap();
        std::fs::create_dir_all(repo.path().join("scope-other")).unwrap();
        std::fs::write(repo.path().join("scope*literal/in-scope.txt"), "scope\n").unwrap();
        std::fs::write(repo.path().join("scope-other/out.txt"), "other\n").unwrap();

        let snapshot = capture_git_status_snapshot(&repo.path().join("scope*literal")).unwrap();
        assert_eq!(snapshot.counts.changed_paths, 1);
        assert_eq!(snapshot.counts.untracked_paths, 1);
        assert_eq!(snapshot.counts.unstaged_paths, 0);
        assert!(matches!(
            snapshot.changed_paths.as_slice(),
            [GitChangedPath::Untracked { path }] if path == "in-scope.txt"
        ));
    }

    #[test]
    fn snapshot_forces_rename_detection_over_repository_config() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(repo.path().join("old.txt"), "rename me\n").unwrap();
        run_git(repo.path(), &["add", "old.txt"]).unwrap();
        run_git(repo.path(), &["commit", "-q", "-m", "base"]).unwrap();
        run_git(repo.path(), &["config", "status.renames", "false"]).unwrap();
        std::fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
        run_git(repo.path(), &["add", "old.txt", "new.txt"]).unwrap();

        let snapshot = capture_git_status_snapshot(repo.path()).unwrap();
        assert_eq!(snapshot.counts.changed_paths, 1);
        assert!(matches!(
            snapshot.changed_paths.as_slice(),
            [GitChangedPath::Renamed { path, previous_path, .. }]
                if path == "new.txt" && previous_path == "old.txt"
        ));
    }

    #[test]
    fn snapshot_preserves_copy_detection_over_repository_config() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(repo.path().join("source.txt"), "copy me\n").unwrap();
        run_git(repo.path(), &["add", "source.txt"]).unwrap();
        run_git(repo.path(), &["commit", "-q", "-m", "base"]).unwrap();
        run_git(repo.path(), &["config", "status.renames", "false"]).unwrap();
        std::fs::copy(repo.path().join("source.txt"), repo.path().join("copy.txt")).unwrap();
        std::fs::write(repo.path().join("source.txt"), "changed source\n").unwrap();
        run_git(repo.path(), &["add", "source.txt", "copy.txt"]).unwrap();

        let snapshot = capture_git_status_snapshot(repo.path()).unwrap();
        assert!(snapshot.changed_paths.iter().any(|path| matches!(
            path,
            GitChangedPath::Copied { path, source_path: previous_path, .. }
                if path == "copy.txt" && previous_path == "source.txt"
        )));
    }

    #[test]
    fn snapshot_scopes_paths_to_a_conversation_subdirectory() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::create_dir_all(repo.path().join("sub/nested")).unwrap();
        std::fs::create_dir_all(repo.path().join("other")).unwrap();
        std::fs::write(repo.path().join("sub/nested/in-scope.txt"), "scope\n").unwrap();
        std::fs::write(repo.path().join("other/out-of-scope.txt"), "other\n").unwrap();

        let snapshot = capture_git_status_snapshot(&repo.path().join("sub")).unwrap();
        assert_eq!(snapshot.counts.changed_paths, 1);
        assert!(matches!(
            snapshot.changed_paths.as_slice(),
            [GitChangedPath::Untracked { path }] if path == "nested/in-scope.txt"
        ));
    }

    #[test]
    fn snapshot_captures_real_repo_status_semantics() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        std::fs::write(repo.path().join("rename old é.txt"), "rename\n").unwrap();
        std::fs::write(repo.path().join("delete.txt"), "delete\n").unwrap();
        run_git(
            repo.path(),
            &["add", "tracked.txt", "rename old é.txt", "delete.txt"],
        )
        .unwrap();
        run_git(repo.path(), &["commit", "-q", "-m", "files"]).unwrap();

        std::fs::write(repo.path().join("tracked.txt"), "worktree change\n").unwrap();
        run_git(repo.path(), &["add", "tracked.txt"]).unwrap();
        std::fs::write(repo.path().join("tracked.txt"), "worktree plus index\n").unwrap();
        std::fs::rename(
            repo.path().join("rename old é.txt"),
            repo.path().join("rename new é.txt"),
        )
        .unwrap();
        run_git(
            repo.path(),
            &["add", "rename old é.txt", "rename new é.txt"],
        )
        .unwrap();
        std::fs::remove_file(repo.path().join("delete.txt")).unwrap();
        run_git(repo.path(), &["rm", "--cached", "delete.txt"]).unwrap();
        std::fs::write(repo.path().join("delete.txt"), "back as untracked\n").unwrap();
        std::fs::write(repo.path().join("new file.txt"), "new\n").unwrap();
        run_git(repo.path(), &["add", "new file.txt"]).unwrap();
        std::fs::write(repo.path().join("scratch.txt"), "scratch\n").unwrap();

        let snapshot = capture_git_status_snapshot(repo.path()).unwrap();
        assert!(snapshot.changed_paths.iter().any(|entry| matches!(entry,
            GitChangedPath::Ordinary {
                path,
                index_status: GitFileStatus::Modified,
                worktree_status: GitFileStatus::Modified,
            } if path == "tracked.txt"
        )));
        assert!(snapshot.changed_paths.iter().any(|entry| matches!(entry,
            GitChangedPath::Renamed {
                path,
                previous_path,
                worktree_status: GitFileStatus::Unmodified,
            } if path == "rename new é.txt" && previous_path == "rename old é.txt"
        )));
        assert!(snapshot.changed_paths.iter().any(|entry| matches!(entry,
            GitChangedPath::Ordinary {
                path,
                index_status: GitFileStatus::Deleted,
                worktree_status: GitFileStatus::Unmodified,
            } if path == "delete.txt"
        )));
        assert!(snapshot.changed_paths.iter().any(|entry| matches!(entry,
            GitChangedPath::Ordinary {
                path,
                index_status: GitFileStatus::Added,
                worktree_status: GitFileStatus::Unmodified,
            } if path == "new file.txt"
        )));
        assert!(snapshot.changed_paths.iter().any(|entry| matches!(entry,
            GitChangedPath::Untracked { path } if path == "scratch.txt"
        )));
    }

    #[test]
    fn capture_checkout_status_reports_non_git_as_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            checkout_status_from_live_observation(dir.path()),
            CheckoutStatus::Unavailable { .. }
        ));
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

    #[tokio::test]
    async fn selection_envelope_reports_active_pr_plural_and_latest_branch() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let scope = crate::work_scope::WorkScopeId::parse("/tmp/ws-envelope").unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                crate::db::WorkScopePrObservation {
                    repo_owner: "acme".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 11,
                    title: "A".to_string(),
                    url: "https://example.test/acme/repo/11".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    display_state: crate::api::types::PrDisplayState::Open,
                    base: "main".to_string(),
                    head: "feature/a".to_string(),
                    github_updated_at: Some("2024-01-01T00:00:00Z".to_string()),
                },
                crate::db::WorkScopePrObservation {
                    repo_owner: "acme".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 22,
                    title: "B".to_string(),
                    url: "https://example.test/acme/repo/22".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    display_state: crate::api::types::PrDisplayState::Open,
                    base: "main".to_string(),
                    head: "feature/b".to_string(),
                    github_updated_at: Some("2024-01-02T00:00:00Z".to_string()),
                },
            ],
        )
        .await
        .unwrap();
        db.upsert_work_scope_observed_branch(
            &scope,
            &crate::db::WorkScopeObservedBranchUpsert {
                repository_identity: "acme/repo".to_string(),
                branch_name: "feature/b".to_string(),
                head_oid: "bbbb".to_string(),
            },
        )
        .await
        .unwrap();
        db.pin_active_work_scope_pr_selection(
            &scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: "acme".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 22,
            },
        )
        .await
        .unwrap();

        let envelope = selection_envelope_for_scope(&db, &scope).await.unwrap();
        assert_eq!(envelope.associated_prs.len(), 2);
        assert_eq!(envelope.active_pr.unwrap().pr.pr_number, 22);
        assert_eq!(
            envelope.latest_observed_branch.unwrap().branch_name,
            "feature/b"
        );
    }

    #[tokio::test]
    async fn missing_worktree_response_retains_full_selection_envelope() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let scope = crate::work_scope::WorkScopeId::parse("/tmp/ws-missing").unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[crate::db::WorkScopePrObservation {
                repo_owner: "acme".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 22,
                title: "open".to_string(),
                url: "https://example.test/acme/repo/22".to_string(),
                state: "OPEN".to_string(),
                draft: false,
                display_state: crate::api::types::PrDisplayState::Open,
                base: "main".to_string(),
                head: "feature/b".to_string(),
                github_updated_at: Some("2024-01-02T00:00:00Z".to_string()),
            }],
        )
        .await
        .unwrap();
        db.upsert_work_scope_observed_branch(
            &scope,
            &crate::db::WorkScopeObservedBranchUpsert {
                repository_identity: "acme/repo".to_string(),
                branch_name: "feature/b".to_string(),
                head_oid: "bbbb".to_string(),
            },
        )
        .await
        .unwrap();
        db.pin_active_work_scope_pr_selection(
            &scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: "acme".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 22,
            },
        )
        .await
        .unwrap();

        let active_selection = active_selection_target_for_scope(&db, &scope)
            .await
            .unwrap()
            .expect("selected PR");
        let mut response = crate::api::pr_monitoring::stale_response(
            active_selection,
            PrUnavailableReason::NotGitRepo,
            "2026-01-01T00:00:00Z".to_string(),
        );
        response.selection = selection_envelope_for_scope(&db, &scope).await.unwrap();
        response.work_change = WorkChangeSummary::Unavailable {
            reason: "worktree path is not a directory".to_string(),
        };
        assert_eq!(response.number, Some(22));
        assert_eq!(response.selection.associated_prs.len(), 1);
        let active = response.selection.active_pr.expect("active selection");
        assert_eq!(active.pr.repo_owner, "acme");
        assert_eq!(active.pr.repo_name, "repo");
        assert_eq!(active.pr.pr_number, 22);
        let observed = response
            .selection
            .latest_observed_branch
            .expect("observed branch");
        assert_eq!(observed.repository_identity, "acme/repo");
        assert_eq!(observed.branch_name, "feature/b");
    }

    #[tokio::test]
    async fn active_selection_target_uses_explicit_selection_not_ranked_primary() {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let scope = crate::work_scope::WorkScopeId::parse("/tmp/ws-active-target").unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                crate::db::WorkScopePrObservation {
                    repo_owner: "acme".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 11,
                    title: "merged".to_string(),
                    url: "https://example.test/acme/repo/11".to_string(),
                    state: "MERGED".to_string(),
                    draft: false,
                    display_state: crate::api::types::PrDisplayState::Merged,
                    base: "main".to_string(),
                    head: "feature/a".to_string(),
                    github_updated_at: Some("2024-01-03T00:00:00Z".to_string()),
                },
                crate::db::WorkScopePrObservation {
                    repo_owner: "acme".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 22,
                    title: "open".to_string(),
                    url: "https://example.test/acme/repo/22".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    display_state: crate::api::types::PrDisplayState::Open,
                    base: "main".to_string(),
                    head: "feature/b".to_string(),
                    github_updated_at: Some("2024-01-02T00:00:00Z".to_string()),
                },
            ],
        )
        .await
        .unwrap();
        db.pin_active_work_scope_pr_selection(
            &scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: "acme".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 22,
            },
        )
        .await
        .unwrap();

        let selected = active_selection_target_for_scope(&db, &scope)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(selected.pr_number, 22);
    }

    #[test]
    fn configured_upstream_ref_reads_tracking_ref() {
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let clone = tempfile::tempdir().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["checkout", "-q", "-b", "feature"]).unwrap();
        run_git(
            clone.path(),
            &["push", "--quiet", "-u", "origin", "feature"],
        )
        .unwrap();
        assert_eq!(
            configured_upstream_ref(clone.path(), "feature"),
            Some("refs/remotes/origin/feature".to_string())
        );
    }

    #[test]
    fn checkout_status_named_branch_reports_matching_origin_fallback() {
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let clone = tempfile::tempdir().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["checkout", "-q", "-b", "feature"]).unwrap();
        run_git(clone.path(), &["push", "--quiet", "origin", "HEAD:feature"]).unwrap();
        run_git(
            clone.path(),
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/acme/repo.git",
            ],
        )
        .unwrap();

        let head_oid = run_git(clone.path(), &["rev-parse", "HEAD"]).unwrap();

        assert_eq!(
            checkout_status_from_live_observation(clone.path()),
            CheckoutStatus::NamedBranch {
                branch_name: "feature".to_string(),
                head_oid: head_oid.trim().to_string(),
                remote_status: BranchRemoteStatus::Matching {
                    remote_ref: "refs/remotes/origin/feature".to_string(),
                    ahead: 0,
                    behind: 0,
                },
            }
        );
    }

    #[test]
    fn checkout_status_named_branch_finds_matching_non_origin_remote() {
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let clone = tempfile::tempdir().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["remote", "rename", "origin", "fork"]).unwrap();
        run_git(clone.path(), &["checkout", "-q", "-b", "feature"]).unwrap();
        run_git(clone.path(), &["push", "--quiet", "fork", "HEAD:feature"]).unwrap();

        let head_oid = run_git(clone.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            checkout_status_from_live_observation(clone.path()),
            CheckoutStatus::NamedBranch {
                branch_name: "feature".to_string(),
                head_oid: head_oid.trim().to_string(),
                remote_status: BranchRemoteStatus::Matching {
                    remote_ref: "refs/remotes/fork/feature".to_string(),
                    ahead: 0,
                    behind: 0,
                },
            }
        );
    }

    #[test]
    fn matching_remote_ref_uses_deterministic_remote_order() {
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let clone = tempfile::tempdir().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["checkout", "-q", "-b", "feature"]).unwrap();
        run_git(clone.path(), &["push", "--quiet", "origin", "HEAD:feature"]).unwrap();
        run_git(
            clone.path(),
            &["remote", "add", "aaa", upstream.path().to_str().unwrap()],
        )
        .unwrap();
        run_git(
            clone.path(),
            &[
                "fetch",
                "--quiet",
                "aaa",
                "feature:refs/remotes/aaa/feature",
            ],
        )
        .unwrap();

        assert_eq!(
            matching_remote_ref(clone.path(), "feature"),
            Some("refs/remotes/aaa/feature".to_string())
        );
    }

    #[test]
    fn checkout_status_named_branch_prefers_configured_non_origin_upstream_and_counts() {
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let clone = tempfile::tempdir().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["remote", "rename", "origin", "fork"]).unwrap();
        run_git(clone.path(), &["checkout", "-q", "-b", "feature/live"]).unwrap();
        run_git(
            clone.path(),
            &["push", "--quiet", "-u", "fork", "feature/live"],
        )
        .unwrap();
        commit_file(clone.path(), "local.txt", "L", "local ahead");
        run_git(upstream.path(), &["checkout", "--quiet", "feature/live"]).unwrap();
        commit_file(upstream.path(), "remote.txt", "R", "remote ahead");
        run_git(clone.path(), &["fetch", "--quiet", "fork"]).unwrap();

        let head_oid = run_git(clone.path(), &["rev-parse", "HEAD"]).unwrap();

        assert_eq!(
            checkout_status_from_live_observation(clone.path()),
            CheckoutStatus::NamedBranch {
                branch_name: "feature/live".to_string(),
                head_oid: head_oid.trim().to_string(),
                remote_status: BranchRemoteStatus::Tracked {
                    remote_ref: "refs/remotes/fork/feature/live".to_string(),
                    ahead: 1,
                    behind: 1,
                },
            }
        );
    }

    #[test]
    fn checkout_status_reports_configured_upstream_as_unavailable_when_ref_is_missing() {
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let clone = tempfile::tempdir().unwrap();
        clone_repo(upstream.path(), clone.path());
        run_git(clone.path(), &["checkout", "-q", "-b", "feature"]).unwrap();
        run_git(
            clone.path(),
            &["push", "--quiet", "-u", "origin", "feature"],
        )
        .unwrap();
        run_git(
            clone.path(),
            &["update-ref", "-d", "refs/remotes/origin/feature"],
        )
        .unwrap();

        assert_eq!(
            configured_upstream_ref(clone.path(), "feature"),
            Some("refs/remotes/origin/feature".to_string())
        );
        let CheckoutStatus::NamedBranch { remote_status, .. } =
            checkout_status_from_live_observation(clone.path())
        else {
            panic!("expected named branch");
        };
        assert_eq!(
            remote_status,
            BranchRemoteStatus::Unavailable {
                reason: "Last-fetched remote comparison is unavailable.".to_string(),
            }
        );
    }

    #[test]
    fn checkout_status_named_branch_reports_no_known_remote_when_untracked() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["checkout", "-q", "-b", "feature/live"]).unwrap();

        let head_oid = run_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();

        assert_eq!(
            checkout_status_from_live_observation(repo.path()),
            CheckoutStatus::NamedBranch {
                branch_name: "feature/live".to_string(),
                head_oid: head_oid.trim().to_string(),
                remote_status: BranchRemoteStatus::NoKnown,
            }
        );
    }

    #[test]
    fn checkout_status_detached_reports_pointing_refs() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["tag", "v1"]).unwrap();
        let head_oid = run_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
        run_git(repo.path(), &["checkout", "--quiet", head_oid.trim()]).unwrap();

        assert_eq!(
            checkout_status_from_live_observation(repo.path()),
            CheckoutStatus::Detached {
                head_oid: head_oid.trim().to_string(),
                pointing_refs: vec!["refs/heads/main".to_string(), "refs/tags/v1".to_string(),],
            }
        );
    }

    #[test]
    fn named_branch_status_omits_server_path_and_derived_exact_ref() {
        let status = CheckoutStatus::NamedBranch {
            branch_name: "feature".to_string(),
            head_oid: "abc123".to_string(),
            remote_status: BranchRemoteStatus::NoKnown,
        };
        let json = serde_json::to_value(status).unwrap();

        assert_eq!(json["branch_name"], "feature");
        assert!(json.get("repository_identity").is_none());
        assert!(json.get("exact_ref").is_none());
    }

    #[test]
    fn detached_status_always_serializes_empty_pointing_refs() {
        let status = CheckoutStatus::Detached {
            head_oid: "abc123".to_string(),
            pointing_refs: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(status).unwrap()["pointing_refs"],
            serde_json::json!([])
        );
    }

    #[test]
    fn stable_checkout_capture_retries_when_head_changes() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let mut captures = 0_u8;

        let (result, status) = capture_with_stable_checkout(repo.path(), || {
            captures += 1;
            if captures == 1 {
                run_git(repo.path(), &["checkout", "-q", "-b", "other"]).unwrap();
            }
            Ok::<_, AppError>(captures)
        })
        .unwrap();

        assert_eq!(result, 2);
        assert!(matches!(
            status,
            CheckoutStatus::NamedBranch {
                branch_name,
                ..
            } if branch_name == "other"
        ));
    }

    #[test]
    fn unstable_checkout_capture_returns_no_diff() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let mut captures = 0_u8;

        let error = capture_with_stable_checkout(repo.path(), || {
            captures += 1;
            if captures == 1 {
                run_git(repo.path(), &["checkout", "-q", "-b", "other"]).unwrap();
            } else {
                run_git(repo.path(), &["checkout", "-q", "main"]).unwrap();
            }
            Ok(captures)
        })
        .unwrap_err();

        assert!(matches!(
            error,
            AppError::Internal(message)
                if message == "The worktree checkout changed while the diff was captured. Reload to retry."
        ));
    }

    #[test]
    fn remote_comparison_error_is_display_safe() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let head_oid = run_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();

        assert_eq!(
            ahead_behind_counts(
                repo.path(),
                head_oid.trim(),
                "refs/remotes/missing/private/server/path"
            ),
            Err("Last-fetched remote comparison is unavailable.".to_string())
        );
    }

    #[test]
    fn unavailable_checkout_error_is_display_safe() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            checkout_status_from_live_observation(directory.path()),
            CheckoutStatus::Unavailable {
                reason: "Checkout status is unavailable.".to_string(),
            }
        );
    }

    #[test]
    fn checkout_status_unborn_reports_branch_name() {
        let repo = tempfile::tempdir().unwrap();
        run_git(repo.path(), &["init", "-q", "-b", "trunk"]).unwrap();
        assert_eq!(
            checkout_status_from_live_observation(repo.path()),
            CheckoutStatus::Unborn {
                branch_name: Some("trunk".to_string()),
            }
        );
    }

    #[test]
    fn build_diff_response_threads_checkout_status() {
        let response = build_diff_response(
            crate::git_ops::CapturedDiff {
                comparator: "origin/main".to_string(),
                commit_log: "abc test".to_string(),
                committed_diff: "diff --git a/a b/a".to_string(),
                committed_total_bytes: 20,
                committed_saturated: false,
                uncommitted_diff: String::new(),
                uncommitted_total_bytes: 0,
                uncommitted_saturated: false,
            },
            "Workspace Diff".to_string(),
            "workspace",
            None,
            CheckoutStatus::NamedBranch {
                branch_name: "feature".to_string(),
                head_oid: "abc123".to_string(),
                remote_status: BranchRemoteStatus::Matching {
                    remote_ref: "refs/remotes/origin/feature".to_string(),
                    ahead: 0,
                    behind: 0,
                },
            },
        );
        assert_eq!(
            response.checkout_status,
            CheckoutStatus::NamedBranch {
                branch_name: "feature".to_string(),
                head_oid: "abc123".to_string(),
                remote_status: BranchRemoteStatus::Matching {
                    remote_ref: "refs/remotes/origin/feature".to_string(),
                    ahead: 0,
                    behind: 0,
                },
            }
        );
    }

    #[test]
    fn active_pr_diff_rejects_cross_repo_selection() {
        let temp = tempfile::tempdir().unwrap();
        init_repo(temp.path());
        run_git(
            temp.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/repo.git",
            ],
        )
        .unwrap();
        let selected = crate::db::WorkScopePrAssociation {
            work_scope_id: crate::work_scope::WorkScopeId::parse("scope-1").unwrap(),
            repo_owner: "fork".to_string(),
            repo_name: "repo".to_string(),
            pr_number: 42,
            title: "fork change".to_string(),
            url: "https://example.test/fork/repo/42".to_string(),
            state: "OPEN".to_string(),
            draft: false,
            display_state: crate::api::types::PrDisplayState::Open,
            base: "main".to_string(),
            head: "feature/fork".to_string(),
            github_updated_at: None,
            feedback_status: phoenix_core::domain::pr_feedback_status::PrFeedbackStatus::Open,
            first_seen_at: "2024-01-01T00:00:00Z".to_string(),
            last_seen_at: "2024-01-01T00:00:00Z".to_string(),
        };

        let err = capture_active_pr_diff(temp.path(), &selected, 256 * 1024)
            .err()
            .expect("cross-repo selection should be rejected");
        match err {
            AppError::BadRequest(message) => {
                assert!(message.contains("selected repository fork/repo"));
                assert!(message.contains("attached to acme/repo"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn active_pr_diff_rejects_selected_head_that_is_not_checked_out() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["checkout", "-q", "-b", "feature/other"]).unwrap();
        let selected = crate::db::WorkScopePrAssociation {
            work_scope_id: crate::work_scope::WorkScopeId::parse("scope-1").unwrap(),
            repo_owner: "acme".to_string(),
            repo_name: "repo".to_string(),
            pr_number: 77,
            title: "selected".to_string(),
            url: "https://example.test/acme/repo/77".to_string(),
            state: "OPEN".to_string(),
            draft: false,
            display_state: crate::api::types::PrDisplayState::Open,
            base: "main".to_string(),
            head: "feature/selected".to_string(),
            github_updated_at: None,
            feedback_status: phoenix_core::domain::pr_feedback_status::PrFeedbackStatus::Open,
            first_seen_at: "2024-01-01T00:00:00Z".to_string(),
            last_seen_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let err = capture_active_pr_diff_for_repo_identity(
            repo.path(),
            Some("acme/repo"),
            &selected,
            256 * 1024,
        )
        .err()
        .expect("mismatched head should be rejected");
        assert!(
            matches!(err, AppError::BadRequest(message) if message.contains("selected head feature/selected") && message.contains("feature/other"))
        );
    }

    #[test]
    fn active_pr_diff_same_repo_uses_selected_pr_base_for_stacked_diff() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        run_git(repo.path(), &["checkout", "-q", "-b", "feature/base"]).unwrap();
        commit_file(repo.path(), "base.txt", "base", "base commit");
        run_git(repo.path(), &["checkout", "-q", "-b", "feature/top"]).unwrap();
        commit_file(repo.path(), "top.txt", "top", "top commit");

        let selected = crate::db::WorkScopePrAssociation {
            work_scope_id: crate::work_scope::WorkScopeId::parse("scope-1").unwrap(),
            repo_owner: "acme".to_string(),
            repo_name: "repo".to_string(),
            pr_number: 77,
            title: "stacked".to_string(),
            url: "https://example.test/acme/repo/77".to_string(),
            state: "OPEN".to_string(),
            draft: false,
            display_state: crate::api::types::PrDisplayState::Open,
            base: "feature/base".to_string(),
            head: "feature/top".to_string(),
            github_updated_at: None,
            feedback_status: phoenix_core::domain::pr_feedback_status::PrFeedbackStatus::Open,
            first_seen_at: "2024-01-01T00:00:00Z".to_string(),
            last_seen_at: "2024-01-01T00:00:00Z".to_string(),
        };

        let captured = capture_active_pr_diff_for_repo_identity(
            repo.path(),
            Some("acme/repo"),
            &selected,
            256 * 1024,
        )
        .unwrap();
        assert_eq!(captured.comparator, "feature/base");
        assert!(captured.commit_log.contains("top commit"));
        assert!(!captured.commit_log.contains("base commit"));
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
