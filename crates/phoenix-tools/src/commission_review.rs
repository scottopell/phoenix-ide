//! Phoenix-native commission review tool.

use super::{Tool, ToolContext, ToolLlmUsage, ToolOutput};
use async_trait::async_trait;
use phoenix_core::domain::llm_types::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, PromptCacheKey, SystemContent,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const MAX_REVIEW_BYTES: usize = 180_000;
const MAX_FILE_BYTES: usize = 80_000;
const MAX_CHUNK_BYTES: usize = 60_000;
const REVIEW_SYSTEM: &str = r#"You are an independent senior code reviewer for Phoenix IDE.
Return only JSON matching this shape:
{"findings":[{"severity":"critical|high|medium|low","confidence":"high|medium|low","file":"path","line":1,"title":"short","rationale":"why this matters","suggested_fix":"concrete fix"}],"summary":"short review summary"}
Focus on correctness, regressions, security, data loss, race conditions, and maintainability. Do not comment on unchanged code unless the diff makes it relevant."#;

#[derive(Debug, Deserialize)]
struct CommissionReviewInput {
    brief: String,
    #[serde(default)]
    focus: Option<String>,
    #[serde(default)]
    allow_dirty_working_tree: bool,
}

#[derive(Debug, Deserialize)]
struct ApprovedCommissionReviewInput {
    #[serde(flatten)]
    request: CommissionReviewInput,
    runtime_base_branch: Option<String>,
    approved_working_dir: String,
    approved_worktree_path: Option<String>,
    approved_head: Option<String>,
    approved_base: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReviewStatus {
    Success,
    Skipped,
    CompletedWithWarnings,
    #[allow(dead_code)]
    Rejected,
    #[allow(dead_code)]
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ReviewFinding {
    severity: String,
    confidence: String,
    file: String,
    line: Option<u64>,
    title: String,
    rationale: String,
    suggested_fix: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewWarning {
    kind: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReviewSummary {
    target: ReviewTargetSummary,
    files_changed: usize,
    files_reviewed: usize,
    insertions: u64,
    deletions: u64,
    findings_count: usize,
    elapsed_ms: u128,
    #[serde(skip)]
    usage: phoenix_core::domain::llm_types::Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewer_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReviewTargetKind {
    WorktreeDiff,
    WorkspaceDiff,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewTargetSummary {
    kind: ReviewTargetKind,
    repo_root: String,
    base: String,
    head: String,
    dirty: bool,
    allow_dirty_working_tree: bool,
}

/// Why a changed file was excluded from the review entirely. Distinct from a
/// `ReviewWarning`: an unreviewed file is a coverage gap the requester must see,
/// not advisory noise, so it is surfaced as a top-level result rather than
/// buried in the warnings stream.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum OversizedReason {
    /// The file's own diff exceeded `MAX_FILE_BYTES`.
    PerFileCap,
    /// The cumulative review body would exceed `MAX_REVIEW_BYTES`, so this file
    /// was dropped even though it fit the per-file cap.
    TotalReviewCap,
}

#[derive(Debug, Clone, Serialize)]
struct UnreviewedFile {
    file: String,
    reason: OversizedReason,
}

#[derive(Debug, Serialize)]
struct ReviewOutput {
    status: ReviewStatus,
    summary: ReviewSummary,
    /// Files changed by the diff that were NOT sent to the reviewer because they
    /// exceeded a size cap. Non-empty means the review did not cover everything.
    unreviewed: Vec<UnreviewedFile>,
    findings: Vec<ReviewFinding>,
    warnings: Vec<ReviewWarning>,
}

#[derive(Debug)]
struct ReviewTarget {
    summary: ReviewTargetSummary,
    diff_spec: DiffSpec,
}

#[derive(Debug)]
enum DiffSpec {
    Range {
        base: String,
        head: String,
        include_worktree: bool,
    },
    Workspace,
}

#[derive(Debug)]
struct DiffCollection {
    files_changed: usize,
    files_reviewed: usize,
    insertions: u64,
    deletions: u64,
    body: String,
    warnings: Vec<ReviewWarning>,
    unreviewed: Vec<UnreviewedFile>,
}

#[derive(Debug, Deserialize)]
struct ModelReviewResponse {
    #[serde(default)]
    findings: Vec<ModelFinding>,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelFinding {
    severity: Option<String>,
    confidence: Option<String>,
    file: Option<String>,
    line: Option<u64>,
    title: Option<String>,
    rationale: Option<String>,
    suggested_fix: Option<String>,
}

pub struct CommissionReviewTool;

#[async_trait]
impl Tool for CommissionReviewTool {
    fn name(&self) -> &'static str {
        "commission_review"
    }

    fn description(&self) -> String {
        "Request an independent Phoenix-native code review of the active git work. This is a capital-spend request: provide a concise executive brief explaining why the work is ready and why review tokens are useful now. Phoenix infers the review target from the active conversation/worktree. Set allow_dirty_working_tree only when reviewing uncommitted changes is intentional.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["brief"],
            "properties": {
                "brief": {
                    "type": "string",
                    "description": "Executive capital brief: why this work is ready for independent review and why spending review tokens is useful now"
                },
                "focus": {
                    "type": "string",
                    "description": "Optional review focus, e.g. security and correctness"
                },
                "allow_dirty_working_tree": {
                    "type": "boolean",
                    "description": "Default false. Required for git-aware task/worktree review when uncommitted changes are present",
                    "default": false
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        match run_review(input, ctx).await {
            Ok(out) => {
                let display = json!({
                    "kind": "commission_review",
                    "status": &out.status,
                    "summary": &out.summary,
                    "unreviewed": &out.unreviewed,
                    "findings": &out.findings,
                    "warnings": &out.warnings,
                });
                ToolOutput::success(pretty_json(&out))
                    .with_display(display)
                    .with_llm_usage(ToolLlmUsage {
                        model: "commission_review".to_string(),
                        usage: out.summary.usage,
                    })
            }
            Err(err) => ToolOutput::error(err),
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run_review(input: Value, ctx: ToolContext) -> Result<ReviewOutput, String> {
    let started = Instant::now();
    let approved: ApprovedCommissionReviewInput = serde_json::from_value(input)
        .map_err(|e| format!("Invalid approved commission_review input: {e}"))?;
    assert_approved_context_has_not_drifted(&ctx, &approved)?;
    if let Some(approved_head) = approved.approved_head.as_deref() {
        let current_head = git_capture(&ctx.working_dir, &["rev-parse", "HEAD"]).await?;
        if current_head != approved_head {
            return Err(format!(
                "commission_review target changed after approval: HEAD was `{approved_head}` at approval time but is now `{current_head}`. Request review again."
            ));
        }
    }
    if let (Some(base_branch), Some(approved_base)) = (
        approved.runtime_base_branch.as_deref(),
        approved.approved_base.as_deref(),
    ) {
        let current_base =
            git_capture(&ctx.working_dir, &["merge-base", base_branch, "HEAD"]).await?;
        if current_base != approved_base {
            return Err(format!(
                "commission_review target changed after approval: merge base was `{approved_base}` at approval time but is now `{current_base}`. Request review again."
            ));
        }
    }
    let input = approved.request;
    if input.brief.trim().is_empty() {
        return Err(
            "commission_review requires a non-empty brief explaining why review is useful now"
                .to_string(),
        );
    }

    let target = resolve_target(&ctx, &input, approved.runtime_base_branch.as_deref()).await?;
    let collection = collect_diff(&target, &ctx).await?;

    if ctx.cancel.is_cancelled() {
        return Err("commission_review cancelled before LLM review".to_string());
    }

    if collection.files_reviewed == 0 {
        // Nothing was reviewed, but distinguish "no reviewable text diff" from
        // "every changed file was excluded by a size cap". The latter is a
        // coverage gap, not a clean no-op, so it must not read as an ordinary
        // Skipped run with no findings.
        let (status, reviewer_summary) = if collection.unreviewed.is_empty() {
            (
                ReviewStatus::Skipped,
                "No reviewable text diff was found".to_string(),
            )
        } else {
            (
                ReviewStatus::CompletedWithWarnings,
                format!(
                    "No files were reviewed: all {} changed file(s) exceeded a size cap",
                    collection.unreviewed.len()
                ),
            )
        };
        return Ok(ReviewOutput {
            status,
            summary: ReviewSummary {
                target: target.summary,
                files_changed: collection.files_changed,
                files_reviewed: 0,
                insertions: collection.insertions,
                deletions: collection.deletions,
                findings_count: 0,
                elapsed_ms: started.elapsed().as_millis(),
                usage: phoenix_core::domain::llm_types::Usage::default(),
                input_tokens: None,
                output_tokens: None,
                reviewer_summary: Some(reviewer_summary),
            },
            unreviewed: collection.unreviewed,
            findings: Vec::new(),
            warnings: collection.warnings,
        });
    }

    let service = ctx.llm_selector().default_service().ok_or_else(|| {
        "commission_review cannot run: no Phoenix LLM model is configured".to_string()
    })?;
    let chunks = review_chunks(&collection.body);
    let mut findings = Vec::new();
    let mut reviewer_summaries = Vec::new();
    let mut warnings = Vec::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut cache_creation_tokens = 0;
    let mut cache_read_tokens = 0;

    for (index, chunk) in chunks.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Ok(failed_review_output(
                started,
                target.summary.clone(),
                &collection,
                findings,
                warnings,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                "commission_review cancelled during LLM review",
            ));
        }
        let request = LlmRequest {
            system: vec![SystemContent::new(REVIEW_SYSTEM)],
            messages: vec![LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(review_prompt(
                    &input,
                    &target.summary,
                    &collection,
                    chunk,
                    index + 1,
                    chunks.len(),
                ))],
            }],
            tools: vec![],
            max_tokens: Some(4096),
            cache_key: PromptCacheKey::stable(format!(
                "commission-review:{}:{index}",
                ctx.conversation_id
            )),
        };

        let response = tokio::select! {
            () = ctx.cancel.cancelled() => {
                return Ok(failed_review_output(
                    started,
                    target.summary.clone(),
                    &collection,
                    findings,
                    warnings,
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    "commission_review cancelled during LLM review",
                ));
            }
            response = service.complete(&request) => match response {
                Ok(response) => response,
                Err(e) => {
                    return Ok(failed_review_output(
                        started,
                        target.summary.clone(),
                        &collection,
                        findings,
                        warnings,
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                        &format!("commission_review LLM review failed: {e}"),
                    ));
                }
            },
        };
        input_tokens += response.usage.input_tokens;
        output_tokens += response.usage.output_tokens;
        cache_creation_tokens += response.usage.cache_creation_tokens;
        cache_read_tokens += response.usage.cache_read_tokens;
        let (mut chunk_findings, chunk_summary, chunk_warnings) = parse_findings(&response.text());
        findings.append(&mut chunk_findings);
        if let Some(summary) = chunk_summary.filter(|s| !s.trim().is_empty()) {
            reviewer_summaries.push(summary);
        }
        warnings.extend(chunk_warnings);
    }

    normalize_findings(&mut findings, &mut warnings);
    warnings.extend(collection.warnings);
    let unreviewed = collection.unreviewed;
    // Unreviewed files are a coverage gap, not advisory noise: a clean run that
    // nonetheless skipped files must not report Success and read as full coverage.
    let status = if warnings.is_empty() && unreviewed.is_empty() {
        ReviewStatus::Success
    } else {
        ReviewStatus::CompletedWithWarnings
    };

    Ok(ReviewOutput {
        status,
        summary: ReviewSummary {
            target: target.summary,
            files_changed: collection.files_changed,
            files_reviewed: collection.files_reviewed,
            insertions: collection.insertions,
            deletions: collection.deletions,
            findings_count: findings.len(),
            elapsed_ms: started.elapsed().as_millis(),
            usage: phoenix_core::domain::llm_types::Usage {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            },
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            reviewer_summary: if reviewer_summaries.is_empty() {
                None
            } else {
                Some(reviewer_summaries.join("\n\n"))
            },
        },
        unreviewed,
        findings,
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
fn failed_review_output(
    started: Instant,
    target: ReviewTargetSummary,
    collection: &DiffCollection,
    findings: Vec<ReviewFinding>,
    mut warnings: Vec<ReviewWarning>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reason: &str,
) -> ReviewOutput {
    warnings.extend(collection.warnings.clone());
    warnings.push(warning("review_failed", reason, None));
    ReviewOutput {
        status: ReviewStatus::Failed,
        summary: ReviewSummary {
            target,
            files_changed: collection.files_changed,
            files_reviewed: collection.files_reviewed,
            insertions: collection.insertions,
            deletions: collection.deletions,
            findings_count: findings.len(),
            elapsed_ms: started.elapsed().as_millis(),
            usage: phoenix_core::domain::llm_types::Usage {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            },
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            reviewer_summary: Some(reason.to_string()),
        },
        unreviewed: collection.unreviewed.clone(),
        findings,
        warnings,
    }
}

fn assert_approved_context_has_not_drifted(
    ctx: &ToolContext,
    approved: &ApprovedCommissionReviewInput,
) -> Result<(), String> {
    let current_worktree = ctx
        .worktree_path
        .as_ref()
        .map(|path| path.display().to_string());
    assert_approved_paths_match(
        &ctx.working_dir.display().to_string(),
        current_worktree.as_ref(),
        approved,
    )
}

fn assert_approved_paths_match(
    current_cwd: &str,
    current_worktree: Option<&String>,
    approved: &ApprovedCommissionReviewInput,
) -> Result<(), String> {
    if current_cwd != approved.approved_working_dir {
        return Err(format!(
            "commission_review target changed after approval: working directory was `{}` at approval time but is now `{current_cwd}`. Request review again.",
            approved.approved_working_dir
        ));
    }

    if current_worktree != approved.approved_worktree_path.as_ref() {
        return Err(format!(
            "commission_review target changed after approval: worktree was `{:?}` at approval time but is now `{:?}`. Request review again.",
            approved.approved_worktree_path,
            current_worktree
        ));
    }
    Ok(())
}

/// Resolve the review comparator, preferring the remote-tracking ref
/// `origin/<base>` over the bare local branch.
///
/// The local `<base>` ref (e.g. `main`) is whatever the worktree last
/// fast-forwarded it to, which on a long-lived clone is routinely months
/// behind. Diffing a feature branch against a stale local base pulls in every
/// commit merged upstream since — inflating the review with already-landed code
/// and, on large files, fabricating diffs big enough to matter. `origin/<base>`
/// is what the branch actually merges into, so it is the correct comparator and
/// matches what the conversation diff endpoint shows the user. Falls back to the
/// local ref when no remote-tracking ref exists (no remote, never fetched).
async fn effective_base_ref(repo: &Path, base_branch: &str) -> String {
    let remote = format!("origin/{base_branch}");
    if git_capture(repo, &["rev-parse", "--verify", "--quiet", &remote])
        .await
        .is_ok()
    {
        remote
    } else {
        base_branch.to_string()
    }
}

async fn resolve_target(
    ctx: &ToolContext,
    input: &CommissionReviewInput,
    runtime_base_branch: Option<&str>,
) -> Result<ReviewTarget, String> {
    let repo_root = git_capture(&ctx.working_dir, &["rev-parse", "--show-toplevel"]).await?;
    let repo = PathBuf::from(repo_root.trim());
    let dirty = !git_capture(&repo, &["status", "--porcelain"])
        .await?
        .trim()
        .is_empty();
    let head = git_capture(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;

    if ctx.worktree_path.is_some() {
        if dirty && !input.allow_dirty_working_tree {
            return Err("commission_review refused dirty worktree review. Commit/stash changes, or set allow_dirty_working_tree=true to include uncommitted changes.".to_string());
        }
        let base = effective_base_ref(&repo, runtime_base_branch.unwrap_or("main")).await;
        Ok(ReviewTarget {
            summary: ReviewTargetSummary {
                kind: ReviewTargetKind::WorktreeDiff,
                repo_root: repo.display().to_string(),
                base: base.clone(),
                head: head.trim().to_string(),
                dirty,
                allow_dirty_working_tree: input.allow_dirty_working_tree,
            },
            diff_spec: DiffSpec::Range {
                base,
                head: "HEAD".to_string(),
                include_worktree: dirty && input.allow_dirty_working_tree,
            },
        })
    } else {
        Ok(ReviewTarget {
            summary: ReviewTargetSummary {
                kind: ReviewTargetKind::WorkspaceDiff,
                repo_root: repo.display().to_string(),
                base: "workspace-base".to_string(),
                head: "working-tree".to_string(),
                dirty,
                allow_dirty_working_tree: input.allow_dirty_working_tree,
            },
            diff_spec: DiffSpec::Workspace,
        })
    }
}

#[allow(clippy::too_many_lines)]
async fn collect_diff(target: &ReviewTarget, ctx: &ToolContext) -> Result<DiffCollection, String> {
    let repo = Path::new(&target.summary.repo_root);
    let mut warnings = Vec::new();
    let mut unreviewed = Vec::new();
    let effective_range_base = match &target.diff_spec {
        DiffSpec::Range { base, head, .. } => {
            Some(git_capture_cancel(repo, &["merge-base", base, head], &ctx.cancel).await?)
        }
        DiffSpec::Workspace => None,
    };
    let numstat = match &target.diff_spec {
        DiffSpec::Range {
            base,
            head,
            include_worktree,
        } => {
            if *include_worktree {
                let merge_base = effective_range_base.as_deref().unwrap_or(base);
                git_capture_cancel(
                    repo,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--numstat",
                        merge_base,
                        "--",
                    ],
                    &ctx.cancel,
                )
                .await?
            } else {
                git_capture_cancel(
                    repo,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--numstat",
                        &format!("{base}...{head}"),
                    ],
                    &ctx.cancel,
                )
                .await?
            }
        }
        DiffSpec::Workspace => {
            git_capture_cancel(
                repo,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--numstat",
                    "HEAD",
                    "--",
                ],
                &ctx.cancel,
            )
            .await?
        }
    };
    let mut insertions = 0;
    let mut deletions = 0;
    let mut files_changed = 0;
    let mut files = Vec::new();
    let untracked = git_capture_cancel(
        repo,
        &["ls-files", "--others", "--exclude-standard"],
        &ctx.cancel,
    )
    .await
    .unwrap_or_default();
    for file in untracked.lines().filter(|line| !line.trim().is_empty()) {
        warnings.push(warning(
            "untracked_file",
            "untracked file is not included in git diff review",
            Some(file),
        ));
    }
    for line in numstat.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() >= 3 {
            files_changed += 1;
            if parts[0] == "-" || parts[1] == "-" {
                warnings.push(warning(
                    "unsupported_file",
                    "binary diff skipped",
                    Some(parts[2]),
                ));
                continue;
            }
            insertions += parts[0].parse::<u64>().unwrap_or(0);
            deletions += parts[1].parse::<u64>().unwrap_or(0);
            files.push(parts[2].to_string());
        }
    }

    let mut body = String::new();
    let mut files_reviewed = 0;
    for file in &files {
        if ctx.cancel.is_cancelled() {
            return Err("commission_review cancelled during diff collection".to_string());
        }
        if is_unsupported(file) {
            warnings.push(warning(
                "unsupported_file",
                "unsupported file type skipped",
                Some(file),
            ));
            continue;
        }
        let diff = match &target.diff_spec {
            DiffSpec::Range {
                base,
                head,
                include_worktree,
            } => {
                let capture_result = if *include_worktree {
                    let merge_base = effective_range_base.as_deref().unwrap_or(base);
                    git_capture_limited_cancel(
                        repo,
                        &[
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            merge_base,
                            "--",
                            file,
                        ],
                        MAX_FILE_BYTES + 1,
                        &ctx.cancel,
                    )
                    .await
                } else {
                    let merge_base_range = format!("{base}...{head}");
                    git_capture_limited_cancel(
                        repo,
                        &[
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            &merge_base_range,
                            "--",
                            file,
                        ],
                        MAX_FILE_BYTES + 1,
                        &ctx.cancel,
                    )
                    .await
                };
                let (diff_output, truncated) = match capture_result {
                    Ok(result) => result,
                    Err(e) => {
                        warnings.push(warning(
                            "diff_capture_failed",
                            &format!("failed to capture file diff: {e}"),
                            Some(file),
                        ));
                        continue;
                    }
                };
                if truncated {
                    unreviewed.push(UnreviewedFile {
                        file: file.clone(),
                        reason: OversizedReason::PerFileCap,
                    });
                    continue;
                }
                diff_output
            }
            DiffSpec::Workspace => {
                let (diff_output, truncated) = match git_capture_limited_cancel(
                    repo,
                    &["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--", file],
                    MAX_FILE_BYTES + 1,
                    &ctx.cancel,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => {
                        warnings.push(warning(
                            "diff_capture_failed",
                            &format!("failed to capture file diff: {e}"),
                            Some(file),
                        ));
                        continue;
                    }
                };
                if truncated {
                    unreviewed.push(UnreviewedFile {
                        file: file.clone(),
                        reason: OversizedReason::PerFileCap,
                    });
                    continue;
                }
                diff_output
            }
        };
        if diff.len() > MAX_FILE_BYTES {
            unreviewed.push(UnreviewedFile {
                file: file.clone(),
                reason: OversizedReason::PerFileCap,
            });
            continue;
        }
        if body.len() + diff.len() > MAX_REVIEW_BYTES {
            unreviewed.push(UnreviewedFile {
                file: file.clone(),
                reason: OversizedReason::TotalReviewCap,
            });
            continue;
        }
        if !diff.trim().is_empty() {
            files_reviewed += 1;
            let _ = write!(body, "\n\n--- FILE: {file} ---\n{diff}");
        }
    }

    Ok(DiffCollection {
        files_changed,
        files_reviewed,
        insertions,
        deletions,
        body,
        warnings,
        unreviewed,
    })
}

fn review_prompt(
    input: &CommissionReviewInput,
    target: &ReviewTargetSummary,
    collection: &DiffCollection,
    diff_chunk: &str,
    chunk_index: usize,
    chunk_count: usize,
) -> String {
    format!(
        "Brief:\n{}\n\nFocus:\n{}\n\nTarget:\n{}\n\nStats: {} changed files, {} reviewed files, +{}/-{}. Dirty: {}, dirty opt-in: {}. Chunk {}/{}.\n\nDiff:\n{}",
        input.brief.trim(),
        input.focus.as_deref().unwrap_or("general correctness review"),
        serde_json::to_string_pretty(target).unwrap_or_default(),
        collection.files_changed,
        collection.files_reviewed,
        collection.insertions,
        collection.deletions,
        target.dirty,
        target.allow_dirty_working_tree,
        chunk_index,
        chunk_count,
        diff_chunk
    )
}

fn review_chunks(body: &str) -> Vec<String> {
    if body.len() <= MAX_CHUNK_BYTES {
        return vec![body.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for section in body
        .split("\n\n--- FILE: ")
        .filter(|s| !s.trim().is_empty())
    {
        let section = if section.starts_with("--- FILE: ") {
            section.to_string()
        } else {
            format!("\n\n--- FILE: {section}")
        };
        if !current.is_empty() && current.len() + section.len() > MAX_CHUNK_BYTES {
            chunks.push(std::mem::take(&mut current));
        }
        if section.len() > MAX_CHUNK_BYTES {
            chunks.push(section);
        } else {
            current.push_str(&section);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn parse_findings(text: &str) -> (Vec<ReviewFinding>, Option<String>, Vec<ReviewWarning>) {
    let mut warnings = Vec::new();
    let cleaned = strip_json_fence(text);
    let mut parse_repaired = false;
    let parsed = serde_json::from_str::<ModelReviewResponse>(&cleaned).or_else(|_| {
        parse_repaired = true;
        let Some(start) = cleaned.find('{') else {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no JSON object start",
            )));
        };
        let Some(end) = cleaned.rfind('}').map(|idx| idx + 1) else {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no JSON object end",
            )));
        };
        if start >= end {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid JSON object bounds",
            )));
        }
        serde_json::from_str::<ModelReviewResponse>(cleaned.get(start..end).unwrap_or_default())
    });

    let Ok(parsed) = parsed else {
        warnings.push(warning(
            "model_output_parse",
            "reviewer returned non-JSON output",
            None,
        ));
        return (Vec::new(), Some(text.trim().to_string()), warnings);
    };

    if text.trim() != cleaned {
        warnings.push(warning(
            "model_output_repaired",
            "reviewer JSON was wrapped in a markdown fence; parsed fenced body",
            None,
        ));
    } else if parse_repaired {
        warnings.push(warning(
            "model_output_repaired",
            "reviewer JSON was embedded in surrounding text; parsed JSON object body",
            None,
        ));
    }

    let mut dropped_findings = 0usize;
    let findings = parsed
        .findings
        .into_iter()
        .filter_map(|f| {
            let Some(file) = f.file.filter(|file| !file.trim().is_empty()) else {
                dropped_findings += 1;
                return None;
            };
            Some(ReviewFinding {
                severity: f.severity.unwrap_or_else(|| "medium".to_string()),
                confidence: f.confidence.unwrap_or_else(|| "medium".to_string()),
                file,
                line: f.line,
                title: f.title.unwrap_or_else(|| "Review finding".to_string()),
                rationale: f.rationale.unwrap_or_default(),
                suggested_fix: f.suggested_fix.unwrap_or_default(),
            })
        })
        .collect();
    if dropped_findings > 0 {
        warnings.push(warning(
            "dropped_findings",
            &format!("dropped {dropped_findings} reviewer finding(s) without a file"),
            None,
        ));
    }
    (findings, parsed.summary, warnings)
}

fn strip_json_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    trimmed
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn normalize_findings(findings: &mut Vec<ReviewFinding>, warnings: &mut Vec<ReviewWarning>) {
    for finding in findings.iter_mut() {
        finding.severity = normalize_enum(
            &finding.severity,
            &["critical", "high", "medium", "low"],
            "medium",
            warnings,
            Some(&finding.file),
            "invalid_severity",
        );
        finding.confidence = normalize_enum(
            &finding.confidence,
            &["high", "medium", "low"],
            "medium",
            warnings,
            Some(&finding.file),
            "invalid_confidence",
        );
        finding.file = finding.file.trim().to_string();
        finding.title = finding.title.trim().to_string();
    }

    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.title.cmp(&b.title))
    });

    let mut seen = HashSet::new();
    findings.retain(|finding| {
        seen.insert((
            finding.file.clone(),
            finding.line,
            finding.title.to_ascii_lowercase(),
        ))
    });
}

fn normalize_enum(
    raw: &str,
    allowed: &[&str],
    fallback: &str,
    warnings: &mut Vec<ReviewWarning>,
    file: Option<&str>,
    kind: &str,
) -> String {
    let value = raw.trim().to_ascii_lowercase();
    if allowed.contains(&value.as_str()) {
        value
    } else {
        warnings.push(warning(
            kind,
            &format!("reviewer returned unsupported value `{raw}`; normalized to `{fallback}`"),
            file,
        ));
        fallback.to_string()
    }
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

fn is_unsupported(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".pdf", ".zip", ".gz", ".lock",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn warning(kind: &str, message: &str, file: Option<&str>) -> ReviewWarning {
    ReviewWarning {
        kind: kind.to_string(),
        message: message.to_string(),
        file: file.map(str::to_string),
    }
}

fn git_command() -> Command {
    let mut command = Command::new("git");
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_NO_LAZY_FETCH", "1");
    command.kill_on_drop(true);
    command
}

async fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_command()
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

async fn git_capture_cancel(
    cwd: &Path,
    args: &[&str],
    cancel: &CancellationToken,
) -> Result<String, String> {
    let child = git_command()
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output();
    tokio::select! {
        () = cancel.cancelled() => Err(format!("git {} cancelled", args.join(" "))),
        output = child => {
            let output = output.map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
            } else {
                Err(format!(
                    "git {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
            }
        }
    }
}

async fn git_capture_limited_cancel(
    cwd: &Path,
    args: &[&str],
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<(String, bool), String> {
    tokio::select! {
        () = cancel.cancelled() => Err(format!("git {} cancelled", args.join(" "))),
        result = git_capture_limited(cwd, args, max_bytes) => result,
    }
}

async fn git_capture_limited(
    cwd: &Path,
    args: &[&str],
    max_bytes: usize,
) -> Result<(String, bool), String> {
    let mut child = git_command()
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("failed to capture git {} stdout", args.join(" ")))?;

    // Drain stdout by looping to EOF, bounded at `max_bytes + 1` bytes. A single
    // `read()` returns only what currently sits in the OS pipe buffer (a few KB
    // to ~64KB), never the whole diff. Reading once and then waiting for the
    // child deadlocks the instant the diff exceeds the pipe buffer: git blocks
    // writing the remainder into a pipe nobody drains, so it never exits and the
    // wait never returns. Draining to EOF keeps the pipe moving; the bound caps
    // memory and lets us detect an over-cap diff.
    let read_cap = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut buf = Vec::new();
    let bytes_read = stdout
        .take(read_cap)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| format!("failed reading git {} stdout: {e}", args.join(" ")))?;

    if bytes_read > max_bytes {
        // Over the cap. We hold the read end and stop draining, so git would
        // wedge on its next write — SIGKILL reaps it regardless of pipe state,
        // which cannot deadlock (unlike waiting for a normal exit).
        let _ = child.kill().await;
        return Ok((String::new(), true));
    }

    // EOF under the cap means git closed stdout and is exiting, so this wait
    // completes promptly and yields the exit status plus any stderr.
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("failed waiting for git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok((String::from_utf8_lossy(&buf).trim_end().to_string(), false))
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{\"status\":\"failed\"}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `git` in `cwd`, asserting success. Test helper for repo setup.
    async fn git_ok(cwd: &Path, args: &[&str]) {
        let out = git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A committed file whose diff far exceeds the OS pipe buffer must return
    /// promptly as truncated, not deadlock. Regression: a single `read()` then
    /// `wait_with_output()` on the taken stdout wedged git on its next write
    /// once the diff outgrew the pipe buffer, parking the tool indefinitely.
    #[tokio::test]
    async fn large_diff_returns_truncated_without_deadlock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).await;
        git_ok(repo, &["config", "user.email", "t@t.t"]).await;
        git_ok(repo, &["config", "user.name", "t"]).await;
        // ~1MB single-line-free blob: orders of magnitude past any pipe buffer.
        let big = "x\n".repeat(512 * 1024);
        std::fs::write(repo.join("big.txt"), &big).expect("write");
        git_ok(repo, &["add", "."]).await;
        git_ok(repo, &["commit", "-qm", "add big"]).await;

        // Diff of the whole file against the empty tree, capped at 64KB.
        let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
        let args = [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            empty_tree,
            "--",
            "big.txt",
        ];
        let fut = git_capture_limited(repo, &args, 64 * 1024);
        let (body, truncated) = tokio::time::timeout(std::time::Duration::from_secs(20), fut)
            .await
            .expect("git_capture_limited must not hang on a large diff")
            .expect("git diff should succeed");
        assert!(truncated, "a >cap diff must be reported truncated");
        assert!(body.is_empty(), "truncated diffs return an empty body");
    }

    /// A diff comfortably under the cap is returned whole — the bounded drain
    /// must not silently truncate output that arrives across multiple reads.
    #[tokio::test]
    async fn small_diff_is_captured_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).await;
        git_ok(repo, &["config", "user.email", "t@t.t"]).await;
        git_ok(repo, &["config", "user.name", "t"]).await;
        let body_text = "line\n".repeat(2000); // ~10KB, well under the 64KB cap
        std::fs::write(repo.join("small.txt"), &body_text).expect("write");
        git_ok(repo, &["add", "."]).await;
        git_ok(repo, &["commit", "-qm", "add small"]).await;

        let empty_tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
        let (body, truncated) = git_capture_limited(
            repo,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                empty_tree,
                "--",
                "small.txt",
            ],
            64 * 1024,
        )
        .await
        .expect("git diff should succeed");
        assert!(!truncated, "an under-cap diff must not be truncated");
        assert!(
            body.matches("+line").count() >= 2000,
            "every added line must survive the capture, got {} lines",
            body.matches("+line").count()
        );
    }

    /// The review comparator must prefer origin/<base> over the (often stale)
    /// local base ref, and fall back to the local ref only when no
    /// remote-tracking ref exists. Diffing against a stale local base is what
    /// inflated a 3-commit PR into a 118-commit review in production.
    #[tokio::test]
    async fn effective_base_ref_prefers_remote_tracking_ref() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).await;
        git_ok(repo, &["config", "user.email", "t@t.t"]).await;
        git_ok(repo, &["config", "user.name", "t"]).await;
        std::fs::write(repo.join("f.txt"), "x").expect("write");
        git_ok(repo, &["add", "."]).await;
        git_ok(repo, &["commit", "-qm", "c1"]).await;
        let head = git_capture(repo, &["rev-parse", "HEAD"])
            .await
            .expect("head");

        // No remote-tracking ref yet: fall back to the bare local branch.
        assert_eq!(effective_base_ref(repo, "main").await, "main");

        // Once origin/main exists, it must win.
        git_ok(repo, &["update-ref", "refs/remotes/origin/main", &head]).await;
        assert_eq!(effective_base_ref(repo, "main").await, "origin/main");
    }

    /// A diff whose only changed files all exceed the per-file cap reviews
    /// nothing, but must report the coverage gap (`completed_with_warnings` +
    /// unreviewed list), not look like an ordinary empty-diff skip.
    #[tokio::test]
    async fn all_files_over_cap_reports_coverage_gap_not_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).await;
        git_ok(repo, &["config", "user.email", "t@t.t"]).await;
        git_ok(repo, &["config", "user.name", "t"]).await;
        git_ok(repo, &["commit", "-qm", "base", "--allow-empty"]).await;
        let base = git_capture(repo, &["rev-parse", "HEAD"])
            .await
            .expect("base");
        git_ok(repo, &["update-ref", "refs/remotes/origin/main", &base]).await;
        // One changed file, far over MAX_FILE_BYTES, on a committed branch.
        let big = "let x = 0;\n".repeat(MAX_FILE_BYTES / 5);
        std::fs::write(repo.join("big.rs"), &big).expect("write");
        git_ok(repo, &["add", "."]).await;
        git_ok(repo, &["commit", "-qm", "huge"]).await;

        let ctx = ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            repo.to_path_buf(),
            std::sync::Arc::new(crate::BrowserSessionManager::default()),
            std::sync::Arc::new(crate::BashHandleRegistry::new()),
            std::sync::Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            std::sync::Arc::new(crate::TmuxRegistry::new()),
            Some(repo.to_path_buf()),
        );
        let target = resolve_target(
            &ctx,
            &CommissionReviewInput {
                brief: "ready".to_string(),
                focus: None,
                allow_dirty_working_tree: false,
            },
            Some("main"),
        )
        .await
        .expect("resolve");
        let collection = collect_diff(&target, &ctx).await.expect("collect");

        assert_eq!(collection.files_reviewed, 0, "the only file is over-cap");
        assert!(
            !collection.unreviewed.is_empty(),
            "over-cap file must be recorded as unreviewed"
        );
    }

    #[test]
    fn unreviewed_files_serialize_as_top_level_snake_case() {
        let out = ReviewOutput {
            status: ReviewStatus::CompletedWithWarnings,
            summary: ReviewSummary {
                target: ReviewTargetSummary {
                    kind: ReviewTargetKind::WorktreeDiff,
                    repo_root: "/r".to_string(),
                    base: "main".to_string(),
                    head: "HEAD".to_string(),
                    dirty: false,
                    allow_dirty_working_tree: false,
                },
                files_changed: 2,
                files_reviewed: 0,
                insertions: 0,
                deletions: 0,
                findings_count: 0,
                elapsed_ms: 0,
                usage: phoenix_core::domain::llm_types::Usage::default(),
                input_tokens: None,
                output_tokens: None,
                reviewer_summary: None,
            },
            unreviewed: vec![
                UnreviewedFile {
                    file: "big.rs".to_string(),
                    reason: OversizedReason::PerFileCap,
                },
                UnreviewedFile {
                    file: "also_big.rs".to_string(),
                    reason: OversizedReason::TotalReviewCap,
                },
            ],
            findings: Vec::new(),
            warnings: Vec::new(),
        };
        let v = serde_json::to_value(&out).expect("serialize");
        assert_eq!(v["unreviewed"][0]["file"], "big.rs");
        assert_eq!(v["unreviewed"][0]["reason"], "per_file_cap");
        assert_eq!(v["unreviewed"][1]["reason"], "total_review_cap");
    }

    #[test]
    fn parse_json_findings() {
        let (findings, summary, warnings) = parse_findings(
            r#"{"summary":"ok","findings":[{"severity":"high","confidence":"high","file":"src/lib.rs","line":7,"title":"bug","rationale":"bad","suggested_fix":"fix"}]}"#,
        );
        assert_eq!(summary.as_deref(), Some("ok"));
        assert!(warnings.is_empty());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/lib.rs");
    }

    #[test]
    fn non_json_model_output_becomes_warning() {
        let (findings, summary, warnings) = parse_findings("looks good");
        assert!(findings.is_empty());
        assert_eq!(summary.as_deref(), Some("looks good"));
        assert_eq!(warnings[0].kind, "model_output_parse");
    }

    #[test]
    fn unsupported_files_are_detected() {
        assert!(is_unsupported("ui/pnpm-lock.yaml.lock"));
        assert!(is_unsupported("image.png"));
        assert!(!is_unsupported("src/lib.rs"));
    }

    #[test]
    fn fenced_json_is_parsed() {
        let (findings, summary, warnings) =
            parse_findings("```json\n{\"summary\":\"ok\",\"findings\":[]}\n```");
        assert_eq!(summary.as_deref(), Some("ok"));
        assert!(findings.is_empty());
        assert_eq!(warnings[0].kind, "model_output_repaired");
    }

    #[test]
    fn dropped_findings_are_reported() {
        let (findings, _summary, warnings) = parse_findings(
            r#"{"findings":[{"severity":"high","confidence":"high","title":"missing file"}]}"#,
        );
        assert!(findings.is_empty());
        assert_eq!(warnings[0].kind, "dropped_findings");
    }

    #[test]
    fn large_diffs_are_split_into_chunks() {
        let body = format!(
            "\n\n--- FILE: a.rs ---\n{}\n\n--- FILE: b.rs ---\n{}",
            "a".repeat(MAX_CHUNK_BYTES / 2),
            "b".repeat(MAX_CHUNK_BYTES / 2)
        );
        let chunks = review_chunks(&body);
        assert!(chunks.len() > 1);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.len() <= MAX_CHUNK_BYTES || chunks.len() == 1));
    }

    #[test]
    fn malformed_braces_do_not_panic() {
        let (findings, summary, warnings) = parse_findings("prefix } suffix");
        assert!(findings.is_empty());
        assert_eq!(summary.as_deref(), Some("prefix } suffix"));
        assert_eq!(warnings[0].kind, "model_output_parse");
    }

    #[test]
    fn approved_context_drift_is_rejected() {
        let approved = ApprovedCommissionReviewInput {
            request: CommissionReviewInput {
                brief: "Ready".to_string(),
                focus: None,
                allow_dirty_working_tree: false,
            },
            runtime_base_branch: Some("main".to_string()),
            approved_working_dir: "/repo/approved".to_string(),
            approved_worktree_path: Some("/repo/approved-wt".to_string()),
            approved_head: None,
            approved_base: None,
        };

        let cwd_err = assert_approved_paths_match(
            "/repo/current",
            Some(&"/repo/approved-wt".to_string()),
            &approved,
        )
        .expect_err("changed cwd should reject");
        assert!(cwd_err.contains("working directory"));

        let wt_err = assert_approved_paths_match(
            "/repo/approved",
            Some(&"/repo/current-wt".to_string()),
            &approved,
        )
        .expect_err("changed worktree should reject");
        assert!(wt_err.contains("worktree"));
    }

    #[test]
    fn approved_context_match_is_allowed() {
        let approved = ApprovedCommissionReviewInput {
            request: CommissionReviewInput {
                brief: "Ready".to_string(),
                focus: None,
                allow_dirty_working_tree: false,
            },
            runtime_base_branch: Some("main".to_string()),
            approved_working_dir: "/repo/approved".to_string(),
            approved_worktree_path: None,
            approved_head: None,
            approved_base: None,
        };

        assert!(assert_approved_paths_match("/repo/approved", None, &approved).is_ok());
    }

    #[test]
    fn findings_are_normalized_sorted_and_deduped() {
        let mut findings = vec![
            ReviewFinding {
                severity: "LOW".to_string(),
                confidence: "certain".to_string(),
                file: "b.rs".to_string(),
                line: Some(2),
                title: "Dup".to_string(),
                rationale: String::new(),
                suggested_fix: String::new(),
            },
            ReviewFinding {
                severity: "critical".to_string(),
                confidence: "high".to_string(),
                file: "a.rs".to_string(),
                line: Some(1),
                title: "Bad".to_string(),
                rationale: String::new(),
                suggested_fix: String::new(),
            },
            ReviewFinding {
                severity: "low".to_string(),
                confidence: "low".to_string(),
                file: "b.rs".to_string(),
                line: Some(2),
                title: "dup".to_string(),
                rationale: String::new(),
                suggested_fix: String::new(),
            },
        ];
        let mut warnings = Vec::new();
        normalize_findings(&mut findings, &mut warnings);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, "critical");
        assert_eq!(findings[1].confidence, "medium");
        assert_eq!(warnings[0].kind, "invalid_confidence");
    }
}
