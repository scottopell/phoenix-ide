use super::types::{
    PrAutoFixContextResponse, PrCheckDetail, PrCheckLogSnippet, PrCheckLogSource, PrCheckState,
    PrCheckSummary, PrDisplayState, PrFeedbackCoverage, PrFeedbackCoverageStatus,
    PrFeedbackCoverageSurface, PrFeedbackFreshness, PrFeedbackFreshnessState, PrFeedbackItem,
    PrFeedbackSource, PrFeedbackSummary, PrIdentity, PrRefreshMetadata, PrRefreshState,
    PrStatusResponse, PrUnavailableReason,
};
use crate::db::{
    WorkScopePrAssociation, WorkScopePrFeedbackBaseline, WorkScopePrFeedbackBaselineInput,
    WorkScopePrObservation,
};
use crate::git_ops::run_git;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

const ARTIFACT_VERSION: u32 = 1;
const LOG_SNIPPET_LIMIT: usize = 16 * 1024;

#[derive(Debug)]
pub(crate) enum PrMonitorError {
    BadRequest(String),
    BadRequestWithObservations {
        message: String,
        observations: Vec<WorkScopePrObservation>,
    },
    Internal(String),
}

impl std::fmt::Display for PrMonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message)
            | Self::BadRequestWithObservations { message, .. }
            | Self::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for PrMonitorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GhFailureKind {
    GhMissing,
    NotAuthenticated,
    CommandFailed,
}

impl GhFailureKind {
    fn unavailable_reason(&self) -> PrUnavailableReason {
        match self {
            Self::GhMissing => PrUnavailableReason::GhMissing,
            Self::NotAuthenticated => PrUnavailableReason::NotAuthenticated,
            Self::CommandFailed => PrUnavailableReason::CommandFailed,
        }
    }

    fn coverage_status(&self) -> PrFeedbackCoverageStatus {
        match self {
            Self::NotAuthenticated => PrFeedbackCoverageStatus::AuthFailed,
            Self::GhMissing | Self::CommandFailed => PrFeedbackCoverageStatus::Unavailable,
        }
    }
}

#[derive(Debug, Clone)]
struct GhFailure {
    kind: GhFailureKind,
    message: String,
}

trait GhClient {
    fn pr_list_for_head(&self, branch: &str) -> Result<Vec<GhPrListItem>, GhFailure>;
    fn pr_view(&self, number: u64) -> Result<GhPrListItem, GhFailure>;
    fn pr_checks(&self, number: u64) -> Result<Vec<GhPrCheck>, GhFailure>;
    fn repo_view(&self) -> Result<GhRepoView, GhFailure>;
    fn issue_comments(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhIssueComment>, GhFailure>;
    fn review_comments(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhReviewComment>, GhFailure>;
    fn review_summaries(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhReviewSummary>, GhFailure>;
    fn review_threads(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhReviewThread>, GhFailure>;
    fn failed_log_snippet(&self, check: &GhPrCheck)
        -> Result<Option<PrCheckLogSnippet>, GhFailure>;
}

struct ShellGhClient<'a> {
    cwd: &'a Path,
    deadline: Option<Instant>,
}

impl<'a> ShellGhClient<'a> {
    fn new(cwd: &'a Path) -> Self {
        Self {
            cwd,
            deadline: None,
        }
    }

    fn with_deadline(cwd: &'a Path, deadline: Instant) -> Self {
        Self {
            cwd,
            deadline: Some(deadline),
        }
    }

    fn run_json<T: for<'de> Deserialize<'de>>(
        &self,
        args: &[&str],
        label: &str,
    ) -> Result<T, GhFailure> {
        let raw = run_gh_with_deadline(self.cwd, args, self.deadline)?;
        serde_json::from_str(&raw).map_err(|e| GhFailure {
            kind: GhFailureKind::CommandFailed,
            message: format!("failed to parse {label}: {e}"),
        })
    }
    fn rest_paginated_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        label: &str,
    ) -> Result<Vec<T>, GhFailure> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let page_arg = page.to_string();
            let mut items: Vec<T> = self.run_json(
                &[
                    "api",
                    path,
                    "--paginate",
                    "-f",
                    "per_page=100",
                    "-f",
                    &format!("page={page_arg}"),
                ],
                label,
            )?;
            if items.is_empty() {
                break;
            }
            let count = items.len();
            all.append(&mut items);
            if count < 100 {
                break;
            }
            page = page.saturating_add(1);
        }
        Ok(all)
    }
}

impl GhClient for ShellGhClient<'_> {
    fn pr_list_for_head(&self, branch: &str) -> Result<Vec<GhPrListItem>, GhFailure> {
        self.run_json(
            &[
                "pr",
                "list",
                "--head",
                branch,
                "--state",
                "all",
                "--limit",
                "20",
                "--json",
                "number,title,url,state,isDraft,baseRefName,headRefName,updatedAt",
            ],
            "PR list",
        )
    }

    fn pr_view(&self, number: u64) -> Result<GhPrListItem, GhFailure> {
        let number = number.to_string();
        self.run_json(
            &[
                "pr",
                "view",
                &number,
                "--json",
                "number,title,url,state,isDraft,baseRefName,headRefName,updatedAt",
            ],
            "PR view",
        )
    }

    fn pr_checks(&self, number: u64) -> Result<Vec<GhPrCheck>, GhFailure> {
        let number = number.to_string();
        let out = run_gh_raw_with_deadline(
            self.cwd,
            &[
                "pr",
                "checks",
                &number,
                "--json",
                "name,state,bucket,link,description,workflow,startedAt,completedAt",
                "--watch=false",
            ],
            self.deadline,
        )?;
        if out.stdout.trim().is_empty() {
            return Err(GhFailure {
                kind: GhFailureKind::CommandFailed,
                message: format!("gh pr checks produced no output: {}", out.stderr),
            });
        }
        serde_json::from_str(&out.stdout).map_err(|e| GhFailure {
            kind: GhFailureKind::CommandFailed,
            message: format!("failed to parse PR checks: {e}"),
        })
    }

    fn repo_view(&self) -> Result<GhRepoView, GhFailure> {
        self.run_json(&["repo", "view", "--json", "owner,name"], "repo view")
    }

    fn issue_comments(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhIssueComment>, GhFailure> {
        self.rest_paginated_json(
            &format!(
                "repos/{}/{}/issues/{number}/comments",
                repo.owner.login, repo.name
            ),
            "issue comments",
        )
    }

    fn review_comments(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhReviewComment>, GhFailure> {
        self.rest_paginated_json(
            &format!(
                "repos/{}/{}/pulls/{number}/comments",
                repo.owner.login, repo.name
            ),
            "review comments",
        )
    }

    fn review_summaries(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhReviewSummary>, GhFailure> {
        self.rest_paginated_json(
            &format!(
                "repos/{}/{}/pulls/{number}/reviews",
                repo.owner.login, repo.name
            ),
            "review summaries",
        )
    }

    fn review_threads(
        &self,
        repo: &GhRepoView,
        number: u64,
    ) -> Result<Vec<GhReviewThread>, GhFailure> {
        let query = r"query($owner:String!, $name:String!, $number:Int!) {
          repository(owner:$owner, name:$name) {
            pullRequest(number:$number) {
              reviewThreads(first:100) {
                nodes { isResolved path comments(first:100) { nodes { id body url createdAt author { login } } } }
              }
            }
          }
        }";
        let parsed: GhReviewThreadsResponse = self.run_json(
            &[
                "api",
                "graphql",
                "-f",
                &format!("query={query}"),
                "-F",
                &format!("owner={}", repo.owner.login),
                "-F",
                &format!("name={}", repo.name),
                "-F",
                &format!("number={number}"),
            ],
            "review threads",
        )?;
        Ok(parsed
            .data
            .and_then(|d| d.repository)
            .and_then(|r| r.pull_request)
            .map(|pr| pr.review_threads.nodes)
            .unwrap_or_default())
    }

    fn failed_log_snippet(
        &self,
        check: &GhPrCheck,
    ) -> Result<Option<PrCheckLogSnippet>, GhFailure> {
        if classify_check(check) != CheckBucket::Failing {
            return Ok(None);
        }
        let Some(url) = check.link.clone() else {
            return Ok(None);
        };
        Ok(Some(PrCheckLogSnippet {
            check_name: check.name.clone().unwrap_or_else(|| "unnamed check".to_string()),
            source: PrCheckLogSource::CheckUrl,
            url: Some(url),
            snippet: "Phoenix captured a check URL for this failure, but direct log extraction is not available for this check provider. Open the URL for full logs.".to_string(),
            truncated: false,
        }))
    }
}

pub(crate) struct PrStatusRefresh {
    pub response: PrStatusResponse,
    pub observations: Vec<WorkScopePrObservation>,
}

pub(crate) fn get_pr_status_for_branch(cwd: &Path, branch_name: &str) -> PrStatusRefresh {
    if run_git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return PrStatusRefresh {
            response: PrStatusResponse::unavailable(PrUnavailableReason::NotGitRepo),
            observations: Vec::new(),
        };
    }
    get_pr_status_with_client(&ShellGhClient::new(cwd), branch_name)
}

pub(crate) fn get_pr_status_for_branch_with_deadline(
    cwd: &Path,
    branch_name: &str,
    deadline: Instant,
) -> PrStatusRefresh {
    if run_git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return PrStatusRefresh {
            response: PrStatusResponse::unavailable(PrUnavailableReason::NotGitRepo),
            observations: Vec::new(),
        };
    }
    get_pr_status_with_client(&ShellGhClient::with_deadline(cwd, deadline), branch_name)
}

fn get_pr_status_with_client(client: &dyn GhClient, branch_name: &str) -> PrStatusRefresh {
    let attempted_at = Utc::now().to_rfc3339();
    let prs = match client.pr_list_for_head(branch_name) {
        Ok(prs) => prs,
        Err(e) => {
            tracing::debug!(branch = %branch_name, error = %e.message, "gh pr list failed");
            return PrStatusRefresh {
                response: unavailable_at(e.kind.unavailable_reason(), attempted_at),
                observations: Vec::new(),
            };
        }
    };

    let Some(pr) = choose_pr(prs.clone()) else {
        return PrStatusRefresh {
            response: not_found_at(attempted_at),
            observations: Vec::new(),
        };
    };
    let display_state = normalize_pr_display_state(&pr.state, pr.is_draft);
    let checks = if matches!(display_state, PrDisplayState::Open) {
        match client.pr_checks(pr.number) {
            Ok(checks) => capture_checks(&checks, Vec::new()),
            Err(e) => {
                tracing::debug!(pr = pr.number, error = %e.message, "gh pr checks could not run");
                unknown_checks()
            }
        }
    } else {
        unknown_checks()
    };

    let observations: Vec<_> = match client.repo_view() {
        Ok(repo) => prs
            .iter()
            .cloned()
            .map(|pr| gh_pr_to_observation(&repo, pr))
            .collect(),
        Err(e) => {
            tracing::debug!(branch = %branch_name, error = %e.message, "gh repo view failed; PR status will not persist observations");
            Vec::new()
        }
    };
    let pr_identity = gh_pr_to_identity(&pr);
    PrStatusRefresh {
        response: fresh_response(pr_identity, checks, attempted_at),
        observations,
    }
}

#[derive(Debug)]
pub(crate) struct PrAutoFixCapture {
    pub response: PrAutoFixContextResponse,
    pub observations: Vec<WorkScopePrObservation>,
    pub baseline: WorkScopePrFeedbackBaselineInput,
}

pub(crate) fn fetch_pr_feedback_for_pr(
    worktree: &Path,
    pr_number: u64,
) -> Result<PrFeedbackSummary, PrMonitorError> {
    if !worktree.is_dir() || run_git(worktree, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Err(PrMonitorError::BadRequest(
            "Conversation worktree is not a git repository".to_string(),
        ));
    }
    Ok(fetch_pr_feedback(&ShellGhClient::new(worktree), pr_number))
}

pub(crate) fn capture_pr_auto_fix_context_for_pr(
    worktree: &Path,
    pr_number: u64,
    llm_language: phoenix_core::llm_language::LlmLanguage,
) -> Result<PrAutoFixCapture, PrMonitorError> {
    if !worktree.is_dir() || run_git(worktree, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Err(PrMonitorError::BadRequest(
            "Conversation worktree is not a git repository".to_string(),
        ));
    }
    let client = ShellGhClient::new(worktree);
    let pr = client.pr_view(pr_number).map_err(|e| {
        let reason = e.kind.unavailable_reason();
        PrMonitorError::BadRequest(format!(
            "PR context unavailable: {}",
            unavailable_reason_message(&reason)
        ))
    })?;
    let observation = match client.repo_view() {
        Ok(repo) => Some(gh_pr_to_observation(&repo, pr.clone())),
        Err(e) => {
            tracing::debug!(pr = pr_number, error = %e.message, "gh repo view failed; associated PR auto-fix will not persist observation");
            None
        }
    };
    let CapturedPrAutoFixContext { response, baseline } =
        match capture_pr_auto_fix_context_for_pr_item(worktree, pr, &client, llm_language) {
            Ok(captured) => captured,
            Err(PrMonitorError::BadRequest(message)) => {
                if let Some(observation) = observation {
                    return Err(PrMonitorError::BadRequestWithObservations {
                        message,
                        observations: vec![observation],
                    });
                }
                return Err(PrMonitorError::BadRequest(message));
            }
            Err(err) => return Err(err),
        };
    Ok(PrAutoFixCapture {
        response,
        observations: observation.into_iter().collect(),
        baseline,
    })
}

pub(crate) fn capture_pr_auto_fix_context_for_branch(
    worktree: &Path,
    branch_name: &str,
    llm_language: phoenix_core::llm_language::LlmLanguage,
) -> Result<PrAutoFixCapture, PrMonitorError> {
    if !worktree.is_dir() || run_git(worktree, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Err(PrMonitorError::BadRequest(
            "Conversation worktree is not a git repository".to_string(),
        ));
    }
    capture_pr_auto_fix_context_for_branch_with_client(
        worktree,
        branch_name,
        &ShellGhClient::new(worktree),
        llm_language,
    )
}

fn capture_pr_auto_fix_context_for_branch_with_client(
    worktree: &Path,
    branch_name: &str,
    client: &dyn GhClient,
    llm_language: phoenix_core::llm_language::LlmLanguage,
) -> Result<PrAutoFixCapture, PrMonitorError> {
    let prs = client.pr_list_for_head(branch_name).map_err(|e| {
        let reason = e.kind.unavailable_reason();
        tracing::debug!(branch = %branch_name, reason = ?reason, error = %e.message, "failed to capture PR context");
        PrMonitorError::BadRequest(format!(
            "PR context unavailable: {}",
            unavailable_reason_message(&reason)
        ))
    })?;
    let repo = match client.repo_view() {
        Ok(repo) => Some(repo),
        Err(e) => {
            tracing::debug!(branch = %branch_name, error = %e.message, "gh repo view failed; PR auto-fix will not persist observations");
            None
        }
    };
    let observations: Vec<WorkScopePrObservation> = repo
        .as_ref()
        .map(|repo| {
            prs.iter()
                .cloned()
                .map(|pr| gh_pr_to_observation(repo, pr))
                .collect()
        })
        .unwrap_or_default();
    let pr = choose_pr(prs).ok_or_else(|| {
        PrMonitorError::BadRequest("No pull request found for this branch".to_string())
    })?;
    let CapturedPrAutoFixContext { response, baseline } =
        capture_pr_auto_fix_context_for_pr_item(worktree, pr, client, llm_language).map_err(
            |err| {
                if let PrMonitorError::BadRequest(message) = err {
                    if !observations.is_empty() {
                        return PrMonitorError::BadRequestWithObservations {
                            message,
                            observations: observations.clone(),
                        };
                    }
                    return PrMonitorError::BadRequest(message);
                }
                err
            },
        )?;
    Ok(PrAutoFixCapture {
        response,
        observations,
        baseline,
    })
}

struct CapturedPrAutoFixContext {
    response: PrAutoFixContextResponse,
    baseline: WorkScopePrFeedbackBaselineInput,
}

fn capture_pr_auto_fix_context_for_pr_item(
    worktree: &Path,
    pr: GhPrListItem,
    client: &dyn GhClient,
    llm_language: phoenix_core::llm_language::LlmLanguage,
) -> Result<CapturedPrAutoFixContext, PrMonitorError> {
    let display_state = normalize_pr_display_state(&pr.state, pr.is_draft);
    if display_state != PrDisplayState::Open {
        return Err(PrMonitorError::BadRequest(
            "Auto-fix is only available for open, non-draft PRs".to_string(),
        ));
    }
    let pr_updated_at = pr.updated_at.clone();

    let raw_checks = match client.pr_checks(pr.number) {
        Ok(checks) => checks,
        Err(e) => {
            tracing::debug!(pr = pr.number, error = %e.message, "failed to capture PR checks for context");
            Vec::new()
        }
    };
    let snippets = capture_log_snippets(client, &raw_checks);
    let checks = capture_checks(&raw_checks, snippets);
    let feedback = fetch_pr_feedback(client, pr.number);
    let fetched_at = Utc::now().to_rfc3339();
    let artifact = PrAutoFixContextArtifact {
        manifest_version: ARTIFACT_VERSION,
        fetched_at: fetched_at.clone(),
        pr: PrArtifactMetadata {
            number: pr.number,
            title: pr.title,
            url: pr.url,
            state: pr.state,
            draft: pr.is_draft,
            base: pr.base_ref_name,
            head: pr.head_ref_name,
            updated_at: pr_updated_at.clone(),
        },
        checks: PrArtifactChecks {
            state: checks.state,
            summary: checks.summary,
            details: checks.details,
            log_snippets: checks.log_snippets,
        },
        feedback,
    };

    let dir = worktree.join(".phoenix").join("pr-context");
    std::fs::create_dir_all(&dir).map_err(|e| {
        PrMonitorError::Internal(format!("Failed to create PR context directory: {e}"))
    })?;
    let safe_ts = fetched_at.replace([':', '.'], "-");
    let rel_path = format!(
        ".phoenix/pr-context/pr-{}-{safe_ts}.json",
        artifact.pr.number
    );
    let path = worktree.join(&rel_path);
    let body = serde_json::to_string_pretty(&artifact)
        .map_err(|e| PrMonitorError::Internal(format!("Failed to encode PR context: {e}")))?;
    std::fs::write(&path, body).map_err(|e| {
        PrMonitorError::Internal(format!("Failed to write PR context artifact: {e}"))
    })?;

    let message = phoenix_core::llm_language::pr_auto_fix_instruction(llm_language, &rel_path);
    let baseline = artifact.baseline();
    Ok(CapturedPrAutoFixContext {
        response: PrAutoFixContextResponse {
            artifact_path: rel_path,
            pr_number: artifact.pr.number,
            message,
        },
        baseline,
    })
}

fn fresh_response(
    pr: PrIdentity,
    checks: CapturedPrChecks,
    attempted_at: String,
) -> PrStatusResponse {
    let refreshed_at = Utc::now().to_rfc3339();
    PrStatusResponse {
        found: true,
        unavailable_reason: None,
        number: Some(pr.number),
        title: Some(pr.title.clone()),
        url: Some(pr.url.clone()),
        state: Some(pr.state.clone()),
        draft: Some(pr.draft),
        base: Some(pr.base.clone()),
        head: Some(pr.head.clone()),
        check_state: matches!(pr.display_state, PrDisplayState::Open).then_some(checks.state),
        check_summary: matches!(pr.display_state, PrDisplayState::Open).then_some(checks.summary),
        feedback_summary: None,
        updated_at: pr.updated_at.clone().or_else(|| Some(refreshed_at.clone())),
        display_state: Some(pr.display_state.clone()),
        feedback_freshness: None,
        pr: Some(pr),
        refresh: PrRefreshMetadata {
            state: PrRefreshState::Fresh,
            reason: None,
            last_attempted_at: attempted_at,
            last_refreshed_at: Some(refreshed_at),
            stale: false,
        },
    }
}

fn not_found_at(attempted_at: String) -> PrStatusResponse {
    PrStatusResponse {
        refresh: PrRefreshMetadata {
            state: PrRefreshState::NotFound,
            reason: None,
            last_attempted_at: attempted_at.clone(),
            last_refreshed_at: Some(attempted_at),
            stale: false,
        },
        ..PrStatusResponse::not_found()
    }
}

fn unavailable_at(reason: PrUnavailableReason, attempted_at: String) -> PrStatusResponse {
    PrStatusResponse {
        refresh: PrRefreshMetadata {
            state: PrRefreshState::Unavailable,
            reason: Some(reason.clone()),
            last_attempted_at: attempted_at,
            last_refreshed_at: None,
            stale: false,
        },
        unavailable_reason: Some(reason),
        ..PrStatusResponse::not_found()
    }
}

pub(crate) fn stale_response(
    pr: WorkScopePrAssociation,
    reason: PrUnavailableReason,
    attempted_at: String,
) -> PrStatusResponse {
    stale_response_with_refresh_state(
        pr,
        PrRefreshState::Unavailable,
        Some(reason.clone()),
        attempted_at,
        Some(reason),
    )
}

pub(crate) fn stale_response_with_refresh_state(
    pr: WorkScopePrAssociation,
    refresh_state: PrRefreshState,
    refresh_reason: Option<PrUnavailableReason>,
    attempted_at: String,
    legacy_unavailable_reason: Option<PrUnavailableReason>,
) -> PrStatusResponse {
    let identity = association_to_identity(&pr);
    PrStatusResponse {
        found: true,
        unavailable_reason: legacy_unavailable_reason,
        number: Some(identity.number),
        title: Some(identity.title.clone()),
        url: Some(identity.url.clone()),
        state: Some(identity.state.clone()),
        draft: Some(identity.draft),
        base: Some(identity.base.clone()),
        head: Some(identity.head.clone()),
        check_state: None,
        check_summary: None,
        feedback_summary: None,
        updated_at: identity.updated_at.clone(),
        display_state: Some(identity.display_state.clone()),
        feedback_freshness: None,
        pr: Some(identity),
        refresh: PrRefreshMetadata {
            state: refresh_state,
            reason: refresh_reason,
            last_attempted_at: attempted_at,
            last_refreshed_at: Some(pr.last_seen_at),
            stale: true,
        },
    }
}

pub(crate) fn stale_primary_response_with_refresh_state(
    pr: &WorkScopePrAssociation,
    refresh_state: PrRefreshState,
    refresh_reason: Option<PrUnavailableReason>,
    attempted_at: String,
) -> PrStatusResponse {
    persisted_primary_response(
        pr,
        PrRefreshMetadata {
            state: refresh_state,
            reason: refresh_reason,
            last_attempted_at: attempted_at,
            last_refreshed_at: Some(pr.last_seen_at.clone()),
            stale: true,
        },
        true,
    )
}

pub(crate) fn persisted_primary_response(
    pr: &WorkScopePrAssociation,
    mut refresh: PrRefreshMetadata,
    stale: bool,
) -> PrStatusResponse {
    let identity = association_to_identity(pr);
    refresh.stale = stale;
    if refresh.last_refreshed_at.is_none() && !stale {
        refresh.last_refreshed_at = Some(pr.last_seen_at.clone());
    }
    PrStatusResponse {
        found: true,
        unavailable_reason: refresh.reason.clone(),
        number: Some(identity.number),
        title: Some(identity.title.clone()),
        url: Some(identity.url.clone()),
        state: Some(identity.state.clone()),
        draft: Some(identity.draft),
        base: Some(identity.base.clone()),
        head: Some(identity.head.clone()),
        check_state: None,
        check_summary: None,
        feedback_summary: None,
        updated_at: identity.updated_at.clone(),
        display_state: Some(identity.display_state.clone()),
        feedback_freshness: None,
        pr: Some(identity),
        refresh,
    }
}

fn gh_pr_to_identity(pr: &GhPrListItem) -> PrIdentity {
    PrIdentity {
        number: pr.number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        state: pr.state.clone(),
        draft: pr.is_draft,
        display_state: normalize_pr_display_state(&pr.state, pr.is_draft),
        base: pr.base_ref_name.clone(),
        head: pr.head_ref_name.clone(),
        updated_at: pr.updated_at.clone(),
    }
}

fn association_to_identity(pr: &WorkScopePrAssociation) -> PrIdentity {
    PrIdentity {
        number: pr.pr_number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        state: pr.state.clone(),
        draft: pr.draft,
        display_state: pr.display_state.clone(),
        base: pr.base.clone(),
        head: pr.head.clone(),
        updated_at: pr.github_updated_at.clone(),
    }
}

fn gh_pr_to_observation(repo: &GhRepoView, pr: GhPrListItem) -> WorkScopePrObservation {
    WorkScopePrObservation {
        repo_owner: repo.owner.login.clone(),
        repo_name: repo.name.clone(),
        pr_number: pr.number,
        title: pr.title,
        url: pr.url,
        state: pr.state.clone(),
        draft: pr.is_draft,
        display_state: normalize_pr_display_state(&pr.state, pr.is_draft),
        base: pr.base_ref_name,
        head: pr.head_ref_name,
        github_updated_at: pr.updated_at,
    }
}

fn unavailable_reason_message(reason: &PrUnavailableReason) -> &'static str {
    match reason {
        PrUnavailableReason::GhMissing => "GitHub CLI is not installed",
        PrUnavailableReason::NotAuthenticated => "GitHub CLI is not authenticated",
        PrUnavailableReason::NotGitRepo => "conversation worktree is not a git repository",
        PrUnavailableReason::CommandFailed => "GitHub CLI command failed",
    }
}

fn capture_log_snippets(client: &dyn GhClient, checks: &[GhPrCheck]) -> Vec<PrCheckLogSnippet> {
    checks
        .iter()
        .filter_map(|check| match client.failed_log_snippet(check) {
            Ok(snippet) => snippet.map(limit_log_snippet),
            Err(e) => {
                tracing::debug!(check = ?check.name, error = %e.message, "failed to capture check log snippet");
                None
            }
        })
        .collect()
}

fn limit_log_snippet(mut snippet: PrCheckLogSnippet) -> PrCheckLogSnippet {
    if snippet.snippet.len() > LOG_SNIPPET_LIMIT {
        let boundary = snippet
            .snippet
            .char_indices()
            .map(|(idx, _)| idx)
            .take_while(|idx| *idx <= LOG_SNIPPET_LIMIT)
            .last()
            .unwrap_or(0);
        snippet.snippet.truncate(boundary);
        snippet.truncated = true;
    }
    snippet
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PrAutoFixContextArtifact {
    manifest_version: u32,
    fetched_at: String,
    pr: PrArtifactMetadata,
    checks: PrArtifactChecks,
    feedback: PrFeedbackSummary,
}

impl PrAutoFixContextArtifact {
    pub(crate) fn baseline(&self) -> WorkScopePrFeedbackBaselineInput {
        WorkScopePrFeedbackBaselineInput {
            pr_number: self.pr.number,
            captured_at: self.fetched_at.clone(),
            github_updated_at: self.pr.updated_at.clone(),
            feedback_identities: self
                .feedback
                .items
                .iter()
                .map(feedback_identity)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect(),
            feedback_fingerprints: self
                .feedback
                .items
                .iter()
                .map(feedback_fingerprint)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect(),
        }
    }
}

pub(crate) fn read_pr_auto_fix_context_artifact(
    path: &Path,
) -> Result<PrAutoFixContextArtifact, PrMonitorError> {
    let body = std::fs::read_to_string(path).map_err(|e| {
        PrMonitorError::Internal(format!("Failed to read PR context artifact: {e}"))
    })?;
    serde_json::from_str(&body)
        .map_err(|e| PrMonitorError::Internal(format!("Failed to parse PR context artifact: {e}")))
}

#[derive(Debug, Serialize, Deserialize)]
struct PrArtifactMetadata {
    number: u64,
    title: String,
    url: String,
    state: String,
    draft: bool,
    base: String,
    head: String,
    updated_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PrArtifactChecks {
    state: PrCheckState,
    summary: PrCheckSummary,
    details: Vec<PrCheckDetail>,
    log_snippets: Vec<PrCheckLogSnippet>,
}

#[derive(Debug, Clone)]
struct CapturedPrChecks {
    state: PrCheckState,
    summary: PrCheckSummary,
    details: Vec<PrCheckDetail>,
    log_snippets: Vec<PrCheckLogSnippet>,
}

fn unknown_checks() -> CapturedPrChecks {
    CapturedPrChecks {
        state: PrCheckState::Unknown,
        summary: PrCheckSummary {
            unknown: 1,
            ..PrCheckSummary::default()
        },
        details: Vec::new(),
        log_snippets: Vec::new(),
    }
}

fn capture_checks(checks: &[GhPrCheck], log_snippets: Vec<PrCheckLogSnippet>) -> CapturedPrChecks {
    let mut summary = PrCheckSummary::default();
    let mut details = Vec::with_capacity(checks.len());
    for check in checks {
        let name = check
            .name
            .clone()
            .unwrap_or_else(|| "unnamed check".to_string());
        match classify_check(check) {
            CheckBucket::Passing => summary.passing += 1,
            CheckBucket::Pending => {
                summary.pending += 1;
                summary.pending_names.push(name.clone());
            }
            CheckBucket::Failing => {
                summary.failing += 1;
                summary.failing_names.push(name.clone());
            }
            CheckBucket::Skipped => summary.skipped += 1,
            CheckBucket::Unknown => summary.unknown += 1,
        }
        details.push(PrCheckDetail {
            name,
            state: check.state.clone().unwrap_or_default(),
            bucket: check.bucket.clone().unwrap_or_default(),
            url: check.link.clone(),
            description: check.description.clone(),
        });
    }
    let state = normalize_check_summary(&summary);
    CapturedPrChecks {
        state,
        summary,
        details,
        log_snippets,
    }
}

#[cfg(test)]
fn normalize_checks(checks: &[GhPrCheck]) -> PrCheckState {
    normalize_check_summary(&capture_checks(checks, Vec::new()).summary)
}

fn normalize_check_summary(summary: &PrCheckSummary) -> PrCheckState {
    if summary.failing > 0 {
        PrCheckState::Failing
    } else if summary.pending > 0 {
        PrCheckState::Pending
    } else if summary.passing > 0 || summary.skipped > 0 {
        PrCheckState::Passing
    } else {
        PrCheckState::Unknown
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CheckBucket {
    Passing,
    Pending,
    Failing,
    Skipped,
    Unknown,
}

fn classify_check(check: &GhPrCheck) -> CheckBucket {
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
        CheckBucket::Failing
    } else if matches!(state.as_str(), "SKIPPED") || matches!(bucket.as_str(), "SKIP") {
        CheckBucket::Skipped
    } else if matches!(state.as_str(), "SUCCESS" | "PASS") || matches!(bucket.as_str(), "PASS") {
        CheckBucket::Passing
    } else if state.is_empty() && bucket.is_empty() {
        CheckBucket::Unknown
    } else {
        CheckBucket::Pending
    }
}

fn fetch_pr_feedback(client: &dyn GhClient, number: u64) -> PrFeedbackSummary {
    let mut items = Vec::new();
    let mut coverage = Vec::new();
    let repo = match client.repo_view() {
        Ok(repo) => repo,
        Err(e) => {
            for surface in PrFeedbackCoverageSurface::all() {
                coverage.push(PrFeedbackCoverage {
                    surface,
                    status: e.kind.coverage_status(),
                    detail: Some("repository owner/name discovery failed".to_string()),
                });
            }
            return PrFeedbackSummary {
                total: 0,
                unresolved: 0,
                items,
                coverage,
            };
        }
    };

    extend_feedback(
        &mut items,
        &mut coverage,
        PrFeedbackCoverageSurface::IssueComments,
        client
            .issue_comments(&repo, number)
            .map(|v| v.into_iter().map(PrFeedbackItem::from).collect()),
    );
    extend_feedback(
        &mut items,
        &mut coverage,
        PrFeedbackCoverageSurface::ReviewComments,
        client
            .review_comments(&repo, number)
            .map(|v| v.into_iter().map(PrFeedbackItem::from).collect()),
    );
    extend_feedback(
        &mut items,
        &mut coverage,
        PrFeedbackCoverageSurface::ReviewSummaries,
        client.review_summaries(&repo, number).map(|v| {
            v.into_iter()
                .filter(|r| r.body.as_deref().is_some_and(|b| !b.trim().is_empty()))
                .map(PrFeedbackItem::from)
                .collect()
        }),
    );
    extend_feedback(
        &mut items,
        &mut coverage,
        PrFeedbackCoverageSurface::ReviewThreads,
        client
            .review_threads(&repo, number)
            .map(review_threads_to_items),
    );

    let items = dedupe_feedback(items);
    let unresolved = u32::try_from(
        items
            .iter()
            .filter(|item| item.resolved != Some(true))
            .count(),
    )
    .unwrap_or(u32::MAX);
    PrFeedbackSummary {
        total: u32::try_from(items.len()).unwrap_or(u32::MAX),
        unresolved,
        items,
        coverage,
    }
}

fn extend_feedback(
    items: &mut Vec<PrFeedbackItem>,
    coverage: &mut Vec<PrFeedbackCoverage>,
    surface: PrFeedbackCoverageSurface,
    result: Result<Vec<PrFeedbackItem>, GhFailure>,
) {
    match result {
        Ok(mut fetched) => {
            items.append(&mut fetched);
            coverage.push(PrFeedbackCoverage {
                surface,
                status: PrFeedbackCoverageStatus::Fetched,
                detail: None,
            });
        }
        Err(e) => {
            tracing::debug!(surface = ?surface, error = %e.message, "failed to fetch PR feedback surface");
            coverage.push(PrFeedbackCoverage {
                surface,
                status: e.kind.coverage_status(),
                detail: Some("surface unavailable from gh".to_string()),
            });
        }
    }
}

fn review_threads_to_items(threads: Vec<GhReviewThread>) -> Vec<PrFeedbackItem> {
    threads
        .into_iter()
        .flat_map(|thread| {
            thread
                .comments
                .nodes
                .into_iter()
                .map(move |comment| PrFeedbackItem {
                    id: comment.id,
                    source: PrFeedbackSource::ReviewThread,
                    author: comment
                        .author
                        .map_or_else(|| "unknown".to_string(), |a| a.login),
                    body: comment.body,
                    path: thread.path.clone(),
                    url: comment.url,
                    created_at: comment.created_at,
                    resolved: Some(thread.is_resolved),
                })
        })
        .collect()
}

fn feedback_identity(item: &PrFeedbackItem) -> String {
    if let Some(id) = &item.id {
        return format!("{:?}:{id}", item.source);
    }
    if let Some(url) = &item.url {
        return format!("{:?}:url:{url}", item.source);
    }
    format!(
        "{:?}:fingerprint:{}:{}:{}:{}",
        item.source,
        item.author,
        item.path.clone().unwrap_or_default(),
        item.created_at.clone().unwrap_or_default(),
        item.body
    )
}

fn feedback_fingerprint(item: &PrFeedbackItem) -> String {
    format!(
        "{:?}:{}:{}:{}:{}:{}:{}|{}",
        item.source,
        item.id.clone().unwrap_or_default(),
        item.url.clone().unwrap_or_default(),
        item.author,
        item.path.clone().unwrap_or_default(),
        item.created_at.clone().unwrap_or_default(),
        item.resolved
            .map(|resolved| resolved.to_string())
            .unwrap_or_default(),
        item.body
    )
}

pub(crate) fn pr_updated_after_baseline(
    baseline: &WorkScopePrFeedbackBaseline,
    current_pr_updated_at: &str,
) -> bool {
    let Some(baseline_updated_at) = baseline.github_updated_at.as_deref() else {
        return true;
    };
    match (
        chrono::DateTime::parse_from_rfc3339(current_pr_updated_at),
        chrono::DateTime::parse_from_rfc3339(baseline_updated_at),
    ) {
        (Ok(current), Ok(baseline)) => current > baseline,
        _ => current_pr_updated_at != baseline_updated_at,
    }
}

pub(crate) fn feedback_freshness_from_baseline(
    baseline: &WorkScopePrFeedbackBaseline,
    current_pr_updated_at: Option<&str>,
    current_feedback: Option<&PrFeedbackSummary>,
) -> Option<PrFeedbackFreshness> {
    if let Some(feedback) = current_feedback {
        let baseline_ids: HashSet<&str> = baseline
            .feedback_identities
            .iter()
            .map(String::as_str)
            .collect();
        let new_count = feedback
            .items
            .iter()
            .map(feedback_identity)
            .filter(|identity| !baseline_ids.contains(identity.as_str()))
            .count();
        if new_count > 0 {
            return Some(PrFeedbackFreshness {
                state: PrFeedbackFreshnessState::New,
                new_count: Some(u32::try_from(new_count).unwrap_or(u32::MAX)),
            });
        }
        let baseline_fingerprints: HashSet<&str> = baseline
            .feedback_fingerprints
            .iter()
            .map(String::as_str)
            .collect();
        let content_changed = feedback
            .items
            .iter()
            .map(feedback_fingerprint)
            .any(|fingerprint| !baseline_fingerprints.contains(fingerprint.as_str()));
        if content_changed
            || feedback
                .coverage
                .iter()
                .any(|coverage| coverage.status != PrFeedbackCoverageStatus::Fetched)
                && current_pr_updated_at.is_some()
        {
            return Some(PrFeedbackFreshness {
                state: PrFeedbackFreshnessState::Updated,
                new_count: None,
            });
        }
        return None;
    }

    let updated_at = current_pr_updated_at?;
    if pr_updated_after_baseline(baseline, updated_at) {
        return Some(PrFeedbackFreshness {
            state: PrFeedbackFreshnessState::Updated,
            new_count: None,
        });
    }

    None
}

fn dedupe_feedback(items: Vec<PrFeedbackItem>) -> Vec<PrFeedbackItem> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for item in items {
        let key = item
            .id
            .clone()
            .or_else(|| item.url.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}|{}|{}|{}",
                    item.author,
                    item.path.clone().unwrap_or_default(),
                    item.created_at.clone().unwrap_or_default(),
                    item.body
                )
            });
        if seen.insert(key) {
            deduped.push(item);
        }
    }
    deduped
}

fn choose_pr(mut prs: Vec<GhPrListItem>) -> Option<GhPrListItem> {
    prs.sort_by(|a, b| {
        pr_rank(a)
            .cmp(&pr_rank(b))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| b.number.cmp(&a.number))
    });
    prs.into_iter().next()
}

fn pr_rank(pr: &GhPrListItem) -> u8 {
    match normalize_pr_display_state(&pr.state, pr.is_draft) {
        PrDisplayState::Open => 0,
        PrDisplayState::Draft => 1,
        PrDisplayState::Merged => 2,
        PrDisplayState::Closed => 3,
    }
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

#[derive(Debug)]
struct GhOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_gh_raw_with_deadline(
    cwd: &Path,
    args: &[&str],
    deadline: Option<Instant>,
) -> Result<GhOutput, GhFailure> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GhFailure {
            kind: if e.kind() == std::io::ErrorKind::NotFound {
                GhFailureKind::GhMissing
            } else {
                GhFailureKind::CommandFailed
            },
            message: format!("Failed to run gh {}: {e}", args.join(" ")),
        })?;
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
        if let Some(status) = child.try_wait().map_err(|e| GhFailure {
            kind: GhFailureKind::CommandFailed,
            message: format!("gh {} wait failed: {e}", args.join(" ")),
        })? {
            break status;
        }
        let command_timed_out = started.elapsed() > Duration::from_secs(8);
        let deadline_expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if command_timed_out || deadline_expired {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_h.join();
            let _ = stderr_h.join();
            return Err(GhFailure {
                kind: GhFailureKind::CommandFailed,
                message: if deadline_expired {
                    format!("gh {} aborted at refresh deadline", args.join(" "))
                } else {
                    format!("gh {} timed out", args.join(" "))
                },
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

fn run_gh_with_deadline(
    cwd: &Path,
    args: &[&str],
    deadline: Option<Instant>,
) -> Result<String, GhFailure> {
    let out = run_gh_raw_with_deadline(cwd, args, deadline)?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let lower = out.stderr.to_lowercase();
    let kind = if lower.contains("not logged")
        || lower.contains("not authenticated")
        || lower.contains("authentication")
        || lower.contains("gh auth login")
    {
        GhFailureKind::NotAuthenticated
    } else {
        GhFailureKind::CommandFailed
    };
    Err(GhFailure {
        kind,
        message: format!("gh {} failed: {}", args.join(" "), out.stderr),
    })
}

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(rename = "updatedAt", default)]
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhPrCheck {
    name: Option<String>,
    state: Option<String>,
    bucket: Option<String>,
    link: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRepoView {
    owner: GhRepoOwner,
    name: String,
}
#[derive(Debug, Clone, Deserialize)]
struct GhRepoOwner {
    login: String,
}
#[derive(Debug, Clone, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GhIssueComment {
    id: Option<u64>,
    user: Option<GhUser>,
    body: Option<String>,
    #[serde(rename = "html_url")]
    html_url: Option<String>,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
}

impl From<GhIssueComment> for PrFeedbackItem {
    fn from(comment: GhIssueComment) -> Self {
        Self {
            id: comment.id.map(|id| id.to_string()),
            source: PrFeedbackSource::IssueComment,
            author: comment
                .user
                .map_or_else(|| "unknown".to_string(), |u| u.login),
            body: comment.body.unwrap_or_default(),
            path: None,
            url: comment.html_url,
            created_at: comment.created_at,
            resolved: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GhReviewComment {
    id: Option<u64>,
    user: Option<GhUser>,
    body: Option<String>,
    path: Option<String>,
    #[serde(rename = "html_url")]
    html_url: Option<String>,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
}

impl From<GhReviewComment> for PrFeedbackItem {
    fn from(comment: GhReviewComment) -> Self {
        Self {
            id: comment.id.map(|id| id.to_string()),
            source: PrFeedbackSource::ReviewComment,
            author: comment
                .user
                .map_or_else(|| "unknown".to_string(), |u| u.login),
            body: comment.body.unwrap_or_default(),
            path: comment.path,
            url: comment.html_url,
            created_at: comment.created_at,
            resolved: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GhReviewSummary {
    id: Option<u64>,
    user: Option<GhUser>,
    body: Option<String>,
    #[serde(rename = "html_url")]
    html_url: Option<String>,
    #[serde(rename = "submitted_at")]
    submitted_at: Option<String>,
}

impl From<GhReviewSummary> for PrFeedbackItem {
    fn from(review: GhReviewSummary) -> Self {
        Self {
            id: review.id.map(|id| id.to_string()),
            source: PrFeedbackSource::ReviewSummary,
            author: review
                .user
                .map_or_else(|| "unknown".to_string(), |u| u.login),
            body: review.body.unwrap_or_default(),
            path: None,
            url: review.html_url,
            created_at: review.submitted_at,
            resolved: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhReviewThreadsResponse {
    data: Option<GhReviewThreadsData>,
}
#[derive(Debug, Deserialize)]
struct GhReviewThreadsData {
    repository: Option<GhReviewThreadsRepo>,
}
#[derive(Debug, Deserialize)]
struct GhReviewThreadsRepo {
    #[serde(rename = "pullRequest")]
    pull_request: Option<GhReviewThreadsPr>,
}
#[derive(Debug, Deserialize)]
struct GhReviewThreadsPr {
    #[serde(rename = "reviewThreads")]
    review_threads: GhReviewThreadsConnection,
}
#[derive(Debug, Deserialize)]
struct GhReviewThreadsConnection {
    nodes: Vec<GhReviewThread>,
}
#[derive(Debug, Clone, Deserialize)]
struct GhReviewThread {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    path: Option<String>,
    comments: GhReviewThreadComments,
}
#[derive(Debug, Clone, Deserialize)]
struct GhReviewThreadComments {
    nodes: Vec<GhReviewThreadComment>,
}
#[derive(Debug, Clone, Deserialize)]
struct GhReviewThreadComment {
    id: Option<String>,
    body: String,
    url: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    author: Option<GhGraphqlAuthor>,
}
#[derive(Debug, Clone, Deserialize)]
struct GhGraphqlAuthor {
    login: String,
}

impl PrFeedbackCoverageSurface {
    fn all() -> [Self; 4] {
        [
            Self::IssueComments,
            Self::ReviewComments,
            Self::ReviewSummaries,
            Self::ReviewThreads,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct FakeGh {
        prs: Result<Vec<GhPrListItem>, GhFailure>,
        checks: Result<Vec<GhPrCheck>, GhFailure>,
        repo: Result<GhRepoView, GhFailure>,
        issue_comments: Result<Vec<GhIssueComment>, GhFailure>,
        review_comments: Result<Vec<GhReviewComment>, GhFailure>,
        review_summaries: Result<Vec<GhReviewSummary>, GhFailure>,
        review_threads: Result<Vec<GhReviewThread>, GhFailure>,
    }

    impl Default for FakeGh {
        fn default() -> Self {
            Self {
                prs: Ok(Vec::new()),
                checks: Ok(Vec::new()),
                repo: Err(GhFailure::default()),
                issue_comments: Ok(Vec::new()),
                review_comments: Ok(Vec::new()),
                review_summaries: Ok(Vec::new()),
                review_threads: Ok(Vec::new()),
            }
        }
    }

    impl GhClient for FakeGh {
        fn pr_list_for_head(&self, _: &str) -> Result<Vec<GhPrListItem>, GhFailure> {
            self.prs.clone()
        }
        fn pr_view(&self, number: u64) -> Result<GhPrListItem, GhFailure> {
            self.prs
                .clone()?
                .into_iter()
                .find(|pr| pr.number == number)
                .ok_or_else(|| GhFailure {
                    kind: GhFailureKind::CommandFailed,
                    message: "not found".to_string(),
                })
        }
        fn pr_checks(&self, _: u64) -> Result<Vec<GhPrCheck>, GhFailure> {
            self.checks.clone()
        }
        fn repo_view(&self) -> Result<GhRepoView, GhFailure> {
            self.repo.clone()
        }
        fn issue_comments(&self, _: &GhRepoView, _: u64) -> Result<Vec<GhIssueComment>, GhFailure> {
            self.issue_comments.clone()
        }
        fn review_comments(
            &self,
            _: &GhRepoView,
            _: u64,
        ) -> Result<Vec<GhReviewComment>, GhFailure> {
            self.review_comments.clone()
        }
        fn review_summaries(
            &self,
            _: &GhRepoView,
            _: u64,
        ) -> Result<Vec<GhReviewSummary>, GhFailure> {
            self.review_summaries.clone()
        }
        fn review_threads(&self, _: &GhRepoView, _: u64) -> Result<Vec<GhReviewThread>, GhFailure> {
            self.review_threads.clone()
        }
        fn failed_log_snippet(
            &self,
            check: &GhPrCheck,
        ) -> Result<Option<PrCheckLogSnippet>, GhFailure> {
            Ok(
                (classify_check(check) == CheckBucket::Failing).then(|| PrCheckLogSnippet {
                    check_name: check.name.clone().unwrap(),
                    source: PrCheckLogSource::CheckUrl,
                    url: check.link.clone(),
                    snippet: "failure log".to_string(),
                    truncated: false,
                }),
            )
        }
    }

    impl Default for GhFailure {
        fn default() -> Self {
            Self {
                kind: GhFailureKind::CommandFailed,
                message: "failed".to_string(),
            }
        }
    }

    fn pr(number: u64, state: &str, draft: bool, updated: &str) -> GhPrListItem {
        GhPrListItem {
            number,
            title: format!("PR {number}"),
            url: format!("https://example.test/{number}"),
            state: state.to_string(),
            is_draft: draft,
            base_ref_name: "main".to_string(),
            head_ref_name: "branch".to_string(),
            updated_at: Some(updated.to_string()),
        }
    }

    fn check(name: &str, state: &str, bucket: &str) -> GhPrCheck {
        GhPrCheck {
            name: Some(name.to_string()),
            state: Some(state.to_string()),
            bucket: Some(bucket.to_string()),
            link: Some(format!("https://checks/{name}")),
            description: None,
        }
    }

    fn repo() -> GhRepoView {
        GhRepoView {
            owner: GhRepoOwner {
                login: "owner".to_string(),
            },
            name: "repo".to_string(),
        }
    }

    #[test]
    fn choose_pr_prefers_open_over_newer_closed() {
        let chosen = choose_pr(vec![
            pr(1, "CLOSED", false, "2026-01-02"),
            pr(2, "OPEN", false, "2026-01-01"),
        ])
        .unwrap();
        assert_eq!(chosen.number, 2);
    }

    #[test]
    fn normalize_checks_classifies_pass_pending_fail_and_skip() {
        assert_eq!(
            normalize_checks(&[check("ok", "SUCCESS", "pass")]),
            PrCheckState::Passing
        );
        assert_eq!(
            normalize_checks(&[check("wait", "PENDING", "pending")]),
            PrCheckState::Pending
        );
        assert_eq!(
            normalize_checks(&[check("bad", "FAILURE", "fail")]),
            PrCheckState::Failing
        );
        assert_eq!(
            normalize_checks(&[check("skip", "SKIPPED", "skip")]),
            PrCheckState::Passing
        );
    }

    #[test]
    fn stale_primary_response_marks_mismatch_stale_with_primary_timestamp() {
        let primary = WorkScopePrAssociation {
            work_scope_id: 1,
            repo_owner: "owner".to_string(),
            repo_name: "repo".to_string(),
            pr_number: 42,
            title: "cached primary".to_string(),
            url: "https://example.test/42".to_string(),
            state: "OPEN".to_string(),
            draft: false,
            display_state: PrDisplayState::Open,
            base: "main".to_string(),
            head: "old-branch".to_string(),
            github_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
            first_seen_at: "2026-01-01T00:00:00Z".to_string(),
            last_seen_at: "2026-01-02T00:00:00Z".to_string(),
        };

        let response = stale_primary_response_with_refresh_state(
            &primary,
            PrRefreshState::Fresh,
            None,
            "2026-01-03T00:00:00Z".to_string(),
        );

        assert!(response.found);
        assert_eq!(response.number, Some(42));
        assert_eq!(response.refresh.state, PrRefreshState::Fresh);
        assert!(response.refresh.stale);
        assert_eq!(response.refresh.last_attempted_at, "2026-01-03T00:00:00Z");
        assert_eq!(
            response.refresh.last_refreshed_at,
            Some("2026-01-02T00:00:00Z".to_string())
        );
        assert_eq!(response.unavailable_reason, None);
    }

    #[test]
    fn status_poll_does_not_fetch_feedback() {
        let gh = FakeGh {
            prs: Ok(vec![pr(7, "OPEN", false, "2026-01-01")]),
            checks: Ok(vec![check("ok", "SUCCESS", "pass")]),
            repo: Err(GhFailure::default()),
            ..FakeGh::default()
        };
        let status = get_pr_status_with_client(&gh, "branch");
        assert!(status.response.found);
        assert_eq!(status.response.check_state, Some(PrCheckState::Passing));
        assert!(status.response.feedback_summary.is_none());
    }

    #[test]
    fn feedback_records_typed_coverage_and_dedupes_urls() {
        let gh = FakeGh {
            repo: Ok(repo()),
            issue_comments: Ok(vec![GhIssueComment {
                id: None,
                user: Some(GhUser {
                    login: "u".to_string(),
                }),
                body: Some("same".to_string()),
                html_url: Some("https://c/1".to_string()),
                created_at: Some("t".to_string()),
            }]),
            review_comments: Ok(vec![GhReviewComment {
                id: None,
                user: Some(GhUser {
                    login: "u".to_string(),
                }),
                body: Some("same".to_string()),
                path: Some("src/lib.rs".to_string()),
                html_url: Some("https://c/1".to_string()),
                created_at: Some("t".to_string()),
            }]),
            review_summaries: Ok(vec![]),
            review_threads: Err(GhFailure::default()),
            ..FakeGh::default()
        };
        let feedback = fetch_pr_feedback(&gh, 7);
        assert_eq!(feedback.total, 1);
        assert!(feedback
            .coverage
            .iter()
            .any(|c| c.surface == PrFeedbackCoverageSurface::ReviewThreads
                && c.status == PrFeedbackCoverageStatus::Unavailable));
    }

    #[test]
    fn feedback_freshness_counts_unseen_identities() {
        let baseline = WorkScopePrFeedbackBaseline {
            work_scope_id: 1,
            pr_number: 7,
            captured_at: "2026-01-01T00:00:00Z".to_string(),
            github_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
            feedback_identities: vec!["IssueComment:1".to_string()],
            feedback_fingerprints: vec!["IssueComment:1::::::|old".to_string()],
        };
        let feedback = PrFeedbackSummary {
            total: 2,
            unresolved: 2,
            coverage: vec![PrFeedbackCoverage {
                surface: PrFeedbackCoverageSurface::IssueComments,
                status: PrFeedbackCoverageStatus::Fetched,
                detail: None,
            }],
            items: vec![
                PrFeedbackItem {
                    id: Some("1".to_string()),
                    source: PrFeedbackSource::IssueComment,
                    author: "u".to_string(),
                    body: "old".to_string(),
                    path: None,
                    url: None,
                    created_at: None,
                    resolved: None,
                },
                PrFeedbackItem {
                    id: Some("2".to_string()),
                    source: PrFeedbackSource::IssueComment,
                    author: "u".to_string(),
                    body: "new".to_string(),
                    path: None,
                    url: None,
                    created_at: None,
                    resolved: None,
                },
            ],
        };

        let freshness = feedback_freshness_from_baseline(
            &baseline,
            Some("2026-01-02T00:00:00Z"),
            Some(&feedback),
        )
        .unwrap();
        assert_eq!(freshness.state, PrFeedbackFreshnessState::New);
        assert_eq!(freshness.new_count, Some(1));
    }

    #[test]
    fn feedback_freshness_degrades_to_updated_when_surface_unavailable() {
        let baseline = WorkScopePrFeedbackBaseline {
            work_scope_id: 1,
            pr_number: 7,
            captured_at: "2026-01-01T00:00:00Z".to_string(),
            github_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
            feedback_identities: vec!["IssueComment:1".to_string()],
            feedback_fingerprints: vec!["IssueComment:1::::::|old".to_string()],
        };
        let feedback = PrFeedbackSummary {
            total: 1,
            unresolved: 1,
            coverage: vec![PrFeedbackCoverage {
                surface: PrFeedbackCoverageSurface::ReviewThreads,
                status: PrFeedbackCoverageStatus::Unavailable,
                detail: None,
            }],
            items: vec![PrFeedbackItem {
                id: Some("1".to_string()),
                source: PrFeedbackSource::IssueComment,
                author: "u".to_string(),
                body: "old".to_string(),
                path: None,
                url: None,
                created_at: None,
                resolved: None,
            }],
        };

        let freshness = feedback_freshness_from_baseline(
            &baseline,
            Some("2026-01-02T00:00:00Z"),
            Some(&feedback),
        )
        .unwrap();
        assert_eq!(freshness.state, PrFeedbackFreshnessState::Updated);
        assert_eq!(freshness.new_count, None);
    }

    #[test]
    fn feedback_freshness_marks_existing_feedback_edits_as_updated() {
        let baseline = WorkScopePrFeedbackBaseline {
            work_scope_id: 1,
            pr_number: 7,
            captured_at: "2026-01-01T00:00:00Z".to_string(),
            github_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
            feedback_identities: vec!["IssueComment:1".to_string()],
            feedback_fingerprints: vec!["IssueComment:1::::::|old".to_string()],
        };
        let feedback = PrFeedbackSummary {
            total: 1,
            unresolved: 1,
            coverage: vec![PrFeedbackCoverage {
                surface: PrFeedbackCoverageSurface::IssueComments,
                status: PrFeedbackCoverageStatus::Fetched,
                detail: None,
            }],
            items: vec![PrFeedbackItem {
                id: Some("1".to_string()),
                source: PrFeedbackSource::IssueComment,
                author: "u".to_string(),
                body: "edited".to_string(),
                path: None,
                url: None,
                created_at: None,
                resolved: None,
            }],
        };

        let freshness = feedback_freshness_from_baseline(
            &baseline,
            Some("2026-01-02T00:00:00Z"),
            Some(&feedback),
        )
        .unwrap();
        assert_eq!(freshness.state, PrFeedbackFreshnessState::Updated);
        assert_eq!(freshness.new_count, None);
    }

    #[test]
    fn context_writes_typed_artifact_and_no_push_prompt() {
        let temp = TempDir::new().unwrap();
        let gh = FakeGh {
            prs: Ok(vec![pr(7, "OPEN", false, "2026-01-01")]),
            checks: Ok(vec![check("test", "FAILURE", "fail")]),
            repo: Ok(repo()),
            issue_comments: Ok(vec![]),
            review_comments: Ok(vec![]),
            review_summaries: Ok(vec![]),
            review_threads: Ok(vec![]),
        };
        let response = capture_pr_auto_fix_context_for_branch_with_client(
            temp.path(),
            "branch",
            &gh,
            phoenix_core::llm_language::LlmLanguage::PhoenixNative,
        )
        .unwrap()
        .response;
        assert_eq!(
            response.message,
            phoenix_core::llm_language::pr_auto_fix_instruction(
                phoenix_core::llm_language::LlmLanguage::PhoenixNative,
                &response.artifact_path,
            )
        );
        assert!(response
            .message
            .contains(&format!("`{}`", response.artifact_path)));
        assert!(!response.message.to_lowercase().contains("push"));
        let artifact: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp.path().join(&response.artifact_path)).unwrap(),
        )
        .unwrap();
        assert_eq!(artifact["manifest_version"], ARTIFACT_VERSION);
        assert_eq!(artifact["pr"]["number"], 7);
        assert_eq!(
            artifact["checks"]["log_snippets"][0]["snippet"],
            "failure log"
        );
        assert!(artifact["feedback"]["coverage"].is_array());
    }

    #[test]
    fn log_snippet_truncates_on_utf8_boundary() {
        let snippet = limit_log_snippet(PrCheckLogSnippet {
            check_name: "unicode".to_string(),
            source: PrCheckLogSource::CheckUrl,
            url: None,
            snippet: format!("{}é", "x".repeat(LOG_SNIPPET_LIMIT - 1)),
            truncated: false,
        });
        assert!(snippet.truncated);
        assert_eq!(snippet.snippet.len(), LOG_SNIPPET_LIMIT - 1);
        assert!(snippet.snippet.is_char_boundary(snippet.snippet.len()));
    }

    #[test]
    fn unavailable_reason_messages_are_human_readable() {
        assert_eq!(
            unavailable_reason_message(&PrUnavailableReason::NotAuthenticated),
            "GitHub CLI is not authenticated"
        );
    }

    #[test]
    fn non_open_auto_fix_error_carries_observations() {
        let temp = TempDir::new().unwrap();
        let gh = FakeGh {
            prs: Ok(vec![pr(7, "MERGED", false, "2026-01-01")]),
            repo: Ok(repo()),
            ..FakeGh::default()
        };
        let err = capture_pr_auto_fix_context_for_branch_with_client(
            temp.path(),
            "branch",
            &gh,
            phoenix_core::llm_language::LlmLanguage::PhoenixNative,
        )
        .unwrap_err();
        match err {
            PrMonitorError::BadRequestWithObservations {
                message,
                observations,
            } => {
                assert!(message.contains("Auto-fix is only available"));
                assert_eq!(observations.len(), 1);
                assert_eq!(observations[0].pr_number, 7);
                assert_eq!(observations[0].state, "MERGED");
            }
            other @ (PrMonitorError::BadRequest(_) | PrMonitorError::Internal(_)) => {
                panic!("expected observations on non-open error, got {other:?}")
            }
        }
        assert!(!temp.path().join(".phoenix/pr-context").exists());
    }

    #[test]
    fn context_failure_writes_no_artifact() {
        let temp = TempDir::new().unwrap();
        let gh = FakeGh {
            prs: Ok(vec![]),
            ..FakeGh::default()
        };
        let err = capture_pr_auto_fix_context_for_branch_with_client(
            temp.path(),
            "branch",
            &gh,
            phoenix_core::llm_language::LlmLanguage::PhoenixNative,
        )
        .unwrap_err();
        assert!(matches!(err, PrMonitorError::BadRequest(_)));
        assert!(!temp.path().join(".phoenix/pr-context").exists());
    }
}
