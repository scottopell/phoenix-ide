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
use phoenix_core::llm_language::COMMISSION_REVIEW_SYSTEM as REVIEW_SYSTEM;

#[derive(Debug, Deserialize)]
struct CommissionReviewInput {
    brief: String,
    #[serde(default)]
    focus: Option<String>,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReviewStatus {
    Success,
    Partial,
    Failed,
    Skipped,
    #[allow(dead_code)]
    Rejected,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReviewCompletionStatus {
    Completed,
    CompletedWithWarnings,
    ModelTimeoutAfterOutput,
    ModelTimeoutNoOutput,
    ModelFailedAfterOutput,
    ModelFailedNoOutput,
    Cancelled,
    Unavailable,
    #[allow(dead_code)]
    Rejected,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FindingsStatus {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FindingsTrust {
    Complete,
    Partial,
    Repaired,
    Low,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StageStatus {
    Ok,
    Partial,
    Timeout,
    Failed,
    Cancelled,
    Skipped,
    Repaired,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ReviewStageStatus {
    target_collection: StageStatus,
    diff_collection: StageStatus,
    llm_review: StageStatus,
    json_parse: StageStatus,
    finding_extraction: StageStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FindingSummary {
    total: usize,
    critical: usize,
    high: usize,
    medium: usize,
    low: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RetryRecommendation {
    Retry,
    DoNotRetry,
    ReviewFindingsFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ReviewFinding {
    severity: String,
    confidence: String,
    file: String,
    line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
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
    elapsed_ms: u128,
    #[serde(skip)]
    usage: phoenix_core::domain::llm_types::Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewer_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReviewTargetKind {
    CommittedBranchDiff,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewTargetSummary {
    kind: ReviewTargetKind,
    repo_root: String,
    base: String,
    head: String,
    dirty: bool,
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
    review_status: ReviewCompletionStatus,
    findings_status: FindingsStatus,
    findings_trust: FindingsTrust,
    stage_status: ReviewStageStatus,
    finding_summary: FindingSummary,
    warnings_summary: Vec<String>,
    retry_recommendation: RetryRecommendation,
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
    Range { base: String, head: String },
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
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    symbol: Option<String>,
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
        "Request an independent Phoenix-native code review of the active git work. This is a capital-spend request: provide a concise executive brief explaining why the work is ready and why review tokens are useful now. Phoenix reviews committed changes only, comparing HEAD against the approved origin base branch (or origin default branch when no base is approved), and refuses dirty working trees.".to_string()
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
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        match run_review(input, ctx).await {
            Ok(out) => {
                let display = review_display(&out);
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

fn review_display(out: &ReviewOutput) -> Value {
    json!({
        "kind": "commission_review",
        "status_panel": [
            {"name": "status", "value": &out.status},
            {"name": "review_status", "value": &out.review_status},
            {"name": "findings_status", "value": &out.findings_status},
            {"name": "findings_trust", "value": &out.findings_trust},
            {"name": "finding_summary", "value": &out.finding_summary},
            {"name": "warnings_summary", "value": &out.warnings_summary},
            {"name": "retry_recommendation", "value": &out.retry_recommendation},
        ],
        "status": &out.status,
        "review_status": &out.review_status,
        "findings_status": &out.findings_status,
        "findings_trust": &out.findings_trust,
        "stage_status": &out.stage_status,
        "finding_summary": &out.finding_summary,
        "warnings_summary": &out.warnings_summary,
        "retry_recommendation": &out.retry_recommendation,
        "summary": &out.summary,
        "unreviewed": &out.unreviewed,
        "findings": &out.findings,
        "warnings": &out.warnings,
    })
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

    let target = match resolve_target(&ctx, &input, approved.runtime_base_branch.as_deref()).await {
        Ok(target) => target,
        Err(reason) => {
            return Ok(ReviewRun::CollectionFailed {
                target: fallback_target_summary(&ctx),
                stage: CollectionFailureStage::TargetCollection,
                reason,
            }
            .into_output(started.elapsed().as_millis()));
        }
    };
    let collection = match collect_diff(&target, &ctx).await {
        Ok(collection) => collection,
        Err(reason) => {
            return Ok(ReviewRun::CollectionFailed {
                target: target.summary,
                stage: CollectionFailureStage::DiffCollection,
                reason,
            }
            .into_output(started.elapsed().as_millis()));
        }
    };

    if ctx.cancel.is_cancelled() {
        return Err("commission_review cancelled before LLM review".to_string());
    }

    if collection.files_reviewed == 0 {
        let has_unreviewed = !collection.unreviewed.is_empty();
        let reviewer_summary = if has_unreviewed {
            format!(
                "No files were reviewed: all {} changed file(s) exceeded a size cap",
                collection.unreviewed.len()
            )
        } else {
            "No reviewable text diff was found".to_string()
        };
        return Ok(ReviewRun::SkippedNoReviewableDiff {
            target: target.summary,
            coverage: ReviewCoverage::from_collection(collection),
            reason: reviewer_summary,
        }
        .into_output(started.elapsed().as_millis()));
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
            return Ok(interrupted_review_output(
                started,
                target.summary.clone(),
                &collection,
                InterruptedReview {
                    findings,
                    warnings,
                    reviewer_summaries,
                    usage: phoenix_core::domain::llm_types::Usage {
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                    },
                    reason: "commission_review cancelled during LLM review".to_string(),
                    interruption: ReviewInterruption::Cancelled,
                },
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
                return Ok(interrupted_review_output(
                    started,
                    target.summary.clone(),
                    &collection,
                    InterruptedReview {
                        findings,
                        warnings,
                        reviewer_summaries,
                        usage: phoenix_core::domain::llm_types::Usage {
                            input_tokens,
                            output_tokens,
                            cache_creation_tokens,
                            cache_read_tokens,
                        },
                        reason: "commission_review cancelled during LLM review".to_string(),
                        interruption: ReviewInterruption::Cancelled,
                    },
                ));
            }
            response = service.complete(&request) => match response {
                Ok(response) => response,
                Err(e) => {
                    return Ok(interrupted_review_output(
                        started,
                        target.summary.clone(),
                        &collection,
                        InterruptedReview {
                            findings,
                            warnings,
                            reviewer_summaries,
                            usage: phoenix_core::domain::llm_types::Usage {
                                input_tokens,
                                output_tokens,
                                cache_creation_tokens,
                                cache_read_tokens,
                            },
                            reason: format!("commission_review LLM review failed: {e}"),
                            interruption: classify_llm_error(&e),
                        },
                    ));
                }
            },
        };
        input_tokens += response.usage.input_tokens;
        output_tokens += response.usage.output_tokens;
        cache_creation_tokens += response.usage.cache_creation_tokens;
        cache_read_tokens += response.usage.cache_read_tokens;
        let (mut chunk_findings, chunk_summary, chunk_warnings) = parse_findings(&response.text());
        let chunk_parse_failed = has_warning(&chunk_warnings, "model_output_parse");
        findings.append(&mut chunk_findings);
        if !chunk_parse_failed {
            if let Some(summary) = chunk_summary.filter(|s| !s.trim().is_empty()) {
                reviewer_summaries.push(summary);
            }
        }
        warnings.extend(chunk_warnings);
    }

    normalize_findings(&mut findings, &mut warnings);
    Ok(ReviewRun::Completed {
        target: target.summary,
        coverage: ReviewCoverage::from_collection(collection),
        parsed: ParsedReviewOutput::from_parts(findings, &reviewer_summaries),
        warnings,
        usage: phoenix_core::domain::llm_types::Usage {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        },
    }
    .into_output(started.elapsed().as_millis()))
}

#[derive(Debug)]
struct ReviewOutputDraft {
    status: ReviewStatus,
    review_status: ReviewCompletionStatus,
    findings_status: FindingsStatus,
    findings_trust: FindingsTrust,
    stage_status: ReviewStageStatus,
    retry_recommendation: RetryRecommendation,
    summary: ReviewSummary,
    unreviewed: Vec<UnreviewedFile>,
    findings: Vec<ReviewFinding>,
    warnings: Vec<ReviewWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewInterruption {
    Timeout,
    Failed,
    Cancelled,
}

#[derive(Debug)]
struct InterruptedReview {
    findings: Vec<ReviewFinding>,
    warnings: Vec<ReviewWarning>,
    reviewer_summaries: Vec<String>,
    usage: phoenix_core::domain::llm_types::Usage,
    reason: String,
    interruption: ReviewInterruption,
}

#[derive(Debug)]
struct ReviewCoverage {
    files_changed: usize,
    files_reviewed: usize,
    insertions: u64,
    deletions: u64,
    warnings: Vec<ReviewWarning>,
    unreviewed: Vec<UnreviewedFile>,
}

impl ReviewCoverage {
    fn from_collection(collection: DiffCollection) -> Self {
        Self {
            files_changed: collection.files_changed,
            files_reviewed: collection.files_reviewed,
            insertions: collection.insertions,
            deletions: collection.deletions,
            warnings: collection.warnings,
            unreviewed: collection.unreviewed,
        }
    }

    fn from_collection_ref(collection: &DiffCollection) -> Self {
        Self {
            files_changed: collection.files_changed,
            files_reviewed: collection.files_reviewed,
            insertions: collection.insertions,
            deletions: collection.deletions,
            warnings: collection.warnings.clone(),
            unreviewed: collection.unreviewed.clone(),
        }
    }

    fn has_unreviewed(&self) -> bool {
        !self.unreviewed.is_empty()
    }
}

#[derive(Debug)]
struct ParsedReviewOutput {
    findings: Vec<ReviewFinding>,
    reviewer_summary: Option<String>,
}

impl ParsedReviewOutput {
    fn from_parts(findings: Vec<ReviewFinding>, reviewer_summaries: &[String]) -> Self {
        Self {
            findings,
            reviewer_summary: if reviewer_summaries.is_empty() {
                None
            } else {
                Some(reviewer_summaries.join("\n\n"))
            },
        }
    }

    fn has_output(&self) -> bool {
        !self.findings.is_empty()
            || self
                .reviewer_summary
                .as_ref()
                .is_some_and(|summary| !summary.trim().is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionFailureStage {
    TargetCollection,
    DiffCollection,
}

#[derive(Debug)]
enum ReviewRun {
    CollectionFailed {
        target: ReviewTargetSummary,
        stage: CollectionFailureStage,
        reason: String,
    },
    SkippedNoReviewableDiff {
        target: ReviewTargetSummary,
        coverage: ReviewCoverage,
        reason: String,
    },
    Completed {
        target: ReviewTargetSummary,
        coverage: ReviewCoverage,
        parsed: ParsedReviewOutput,
        warnings: Vec<ReviewWarning>,
        usage: phoenix_core::domain::llm_types::Usage,
    },
    InterruptedAfterOutput {
        target: ReviewTargetSummary,
        coverage: ReviewCoverage,
        parsed: ParsedReviewOutput,
        warnings: Vec<ReviewWarning>,
        usage: phoenix_core::domain::llm_types::Usage,
        reason: String,
        interruption: ReviewInterruption,
    },
    InterruptedNoOutput {
        target: ReviewTargetSummary,
        coverage: ReviewCoverage,
        warnings: Vec<ReviewWarning>,
        usage: phoenix_core::domain::llm_types::Usage,
        reason: String,
        interruption: ReviewInterruption,
    },
}

impl ReviewRun {
    fn into_output(self, elapsed_ms: u128) -> ReviewOutput {
        match self {
            ReviewRun::CollectionFailed {
                target,
                stage,
                reason,
            } => collection_failed_output(target, stage, &reason, elapsed_ms),
            ReviewRun::SkippedNoReviewableDiff {
                target,
                coverage,
                reason,
            } => skipped_no_reviewable_output(target, coverage, reason, elapsed_ms),
            ReviewRun::Completed {
                target,
                coverage,
                parsed,
                warnings,
                usage,
            } => completed_review_output(target, coverage, parsed, warnings, usage, elapsed_ms),
            ReviewRun::InterruptedAfterOutput {
                target,
                coverage,
                parsed,
                warnings,
                usage,
                reason,
                interruption,
            } => interrupted_after_output(
                target,
                coverage,
                parsed,
                warnings,
                usage,
                ReviewInterruptionContext {
                    reason: &reason,
                    interruption,
                },
                elapsed_ms,
            ),
            ReviewRun::InterruptedNoOutput {
                target,
                coverage,
                warnings,
                usage,
                reason,
                interruption,
            } => interrupted_no_output(
                target,
                coverage,
                warnings,
                usage,
                &reason,
                interruption,
                elapsed_ms,
            ),
        }
    }
}

fn fallback_target_summary(ctx: &ToolContext) -> ReviewTargetSummary {
    ReviewTargetSummary {
        kind: ReviewTargetKind::CommittedBranchDiff,
        repo_root: ctx.working_dir.display().to_string(),
        base: "unknown".to_string(),
        head: "unknown".to_string(),
        dirty: false,
    }
}

fn collection_failed_output(
    target: ReviewTargetSummary,
    stage: CollectionFailureStage,
    reason: &str,
    elapsed_ms: u128,
) -> ReviewOutput {
    let cancelled = reason.to_ascii_lowercase().contains("cancelled");
    let coverage = ReviewCoverage {
        files_changed: 0,
        files_reviewed: 0,
        insertions: 0,
        deletions: 0,
        warnings: vec![warning(
            if cancelled {
                "review_cancelled"
            } else {
                "collection_failed"
            },
            reason,
            None,
        )],
        unreviewed: Vec::new(),
    };
    finalize_review_output(ReviewOutputDraft {
        status: ReviewStatus::Failed,
        review_status: if cancelled {
            ReviewCompletionStatus::Cancelled
        } else {
            ReviewCompletionStatus::Unavailable
        },
        findings_status: FindingsStatus::Unavailable,
        findings_trust: FindingsTrust::Low,
        stage_status: ReviewStageStatus {
            target_collection: if stage == CollectionFailureStage::TargetCollection {
                if cancelled {
                    StageStatus::Cancelled
                } else {
                    StageStatus::Failed
                }
            } else {
                StageStatus::Ok
            },
            diff_collection: if stage == CollectionFailureStage::DiffCollection {
                if cancelled {
                    StageStatus::Cancelled
                } else {
                    StageStatus::Failed
                }
            } else {
                StageStatus::Skipped
            },
            llm_review: StageStatus::Skipped,
            json_parse: StageStatus::Skipped,
            finding_extraction: StageStatus::Skipped,
        },
        retry_recommendation: if cancelled {
            RetryRecommendation::DoNotRetry
        } else {
            RetryRecommendation::Retry
        },
        summary: review_summary(
            target,
            &coverage,
            elapsed_ms,
            phoenix_core::domain::llm_types::Usage::default(),
            None,
        ),
        unreviewed: coverage.unreviewed,
        findings: Vec::new(),
        warnings: coverage.warnings,
    })
}

fn review_summary(
    target: ReviewTargetSummary,
    coverage: &ReviewCoverage,
    elapsed_ms: u128,
    usage: phoenix_core::domain::llm_types::Usage,
    reviewer_summary: Option<String>,
) -> ReviewSummary {
    ReviewSummary {
        target,
        files_changed: coverage.files_changed,
        files_reviewed: coverage.files_reviewed,
        insertions: coverage.insertions,
        deletions: coverage.deletions,
        elapsed_ms,
        usage,
        reviewer_summary,
    }
}

fn skipped_no_reviewable_output(
    target: ReviewTargetSummary,
    coverage: ReviewCoverage,
    reason: String,
    elapsed_ms: u128,
) -> ReviewOutput {
    let has_unreviewed = coverage.has_unreviewed();
    finalize_review_output(ReviewOutputDraft {
        status: if has_unreviewed {
            ReviewStatus::Partial
        } else {
            ReviewStatus::Skipped
        },
        review_status: if has_unreviewed {
            ReviewCompletionStatus::CompletedWithWarnings
        } else {
            ReviewCompletionStatus::Unavailable
        },
        findings_status: FindingsStatus::Unavailable,
        findings_trust: FindingsTrust::Low,
        stage_status: ReviewStageStatus {
            target_collection: StageStatus::Ok,
            diff_collection: diff_stage_status(&coverage.warnings, has_unreviewed),
            llm_review: StageStatus::Skipped,
            json_parse: StageStatus::Skipped,
            finding_extraction: StageStatus::Skipped,
        },
        retry_recommendation: RetryRecommendation::DoNotRetry,
        summary: review_summary(
            target,
            &coverage,
            elapsed_ms,
            phoenix_core::domain::llm_types::Usage::default(),
            Some(reason),
        ),
        unreviewed: coverage.unreviewed,
        findings: Vec::new(),
        warnings: coverage.warnings,
    })
}

fn completed_review_output(
    target: ReviewTargetSummary,
    coverage: ReviewCoverage,
    parsed: ParsedReviewOutput,
    mut warnings: Vec<ReviewWarning>,
    usage: phoenix_core::domain::llm_types::Usage,
    elapsed_ms: u128,
) -> ReviewOutput {
    warnings.extend(coverage.warnings.clone());
    let has_unreviewed = coverage.has_unreviewed();
    let has_diff_coverage_gap =
        diff_stage_status(&warnings, has_unreviewed) == StageStatus::Partial;
    let (status, findings_status, findings_trust, retry_recommendation) =
        completed_result_contract(&warnings, parsed.has_output(), has_diff_coverage_gap);
    let review_status = completed_review_status(&status, warnings.is_empty() && !has_unreviewed);

    finalize_review_output(ReviewOutputDraft {
        status,
        review_status,
        findings_status,
        findings_trust,
        stage_status: ReviewStageStatus {
            target_collection: StageStatus::Ok,
            diff_collection: diff_stage_status(&warnings, has_unreviewed),
            llm_review: StageStatus::Ok,
            json_parse: json_parse_stage_status(&warnings),
            finding_extraction: finding_extraction_stage_status(&warnings),
        },
        retry_recommendation,
        summary: review_summary(
            target,
            &coverage,
            elapsed_ms,
            usage,
            parsed.reviewer_summary,
        ),
        unreviewed: coverage.unreviewed,
        findings: parsed.findings,
        warnings,
    })
}

#[derive(Clone, Copy)]
struct ReviewInterruptionContext<'a> {
    reason: &'a str,
    interruption: ReviewInterruption,
}

fn interrupted_after_output(
    target: ReviewTargetSummary,
    coverage: ReviewCoverage,
    parsed: ParsedReviewOutput,
    mut warnings: Vec<ReviewWarning>,
    usage: phoenix_core::domain::llm_types::Usage,
    context: ReviewInterruptionContext<'_>,
    elapsed_ms: u128,
) -> ReviewOutput {
    let ReviewInterruptionContext {
        reason,
        interruption,
    } = context;
    warnings.extend(coverage.warnings.clone());
    warnings.push(warning(
        interrupted_warning_kind(interruption, true),
        reason,
        None,
    ));
    let (status, review_status, findings_status, findings_trust, retry_recommendation) =
        interrupted_contract(interruption, true);
    finalize_review_output(ReviewOutputDraft {
        status,
        review_status,
        findings_status,
        findings_trust,
        stage_status: ReviewStageStatus {
            target_collection: StageStatus::Ok,
            diff_collection: diff_stage_status(&warnings, coverage.has_unreviewed()),
            llm_review: llm_interruption_stage(interruption),
            json_parse: interrupted_json_parse_stage_status(&warnings, true),
            finding_extraction: interrupted_finding_extraction_stage_status(&warnings, true),
        },
        retry_recommendation,
        summary: review_summary(
            target,
            &coverage,
            elapsed_ms,
            usage,
            parsed.reviewer_summary,
        ),
        unreviewed: coverage.unreviewed,
        findings: parsed.findings,
        warnings,
    })
}

fn interrupted_no_output(
    target: ReviewTargetSummary,
    coverage: ReviewCoverage,
    mut warnings: Vec<ReviewWarning>,
    usage: phoenix_core::domain::llm_types::Usage,
    reason: &str,
    interruption: ReviewInterruption,
    elapsed_ms: u128,
) -> ReviewOutput {
    warnings.extend(coverage.warnings.clone());
    warnings.push(warning(
        interrupted_warning_kind(interruption, false),
        reason,
        None,
    ));
    let (status, review_status, findings_status, findings_trust, retry_recommendation) =
        interrupted_contract(interruption, false);
    finalize_review_output(ReviewOutputDraft {
        status,
        review_status,
        findings_status,
        findings_trust,
        stage_status: ReviewStageStatus {
            target_collection: StageStatus::Ok,
            diff_collection: diff_stage_status(&warnings, coverage.has_unreviewed()),
            llm_review: llm_interruption_stage(interruption),
            json_parse: interrupted_json_parse_stage_status(&warnings, false),
            finding_extraction: interrupted_finding_extraction_stage_status(&warnings, false),
        },
        retry_recommendation,
        summary: review_summary(target, &coverage, elapsed_ms, usage, None),
        unreviewed: coverage.unreviewed,
        findings: Vec::new(),
        warnings,
    })
}

fn interrupted_warning_kind(interruption: ReviewInterruption, has_output: bool) -> &'static str {
    match (interruption, has_output) {
        (ReviewInterruption::Timeout, true) => "review_timeout_partial",
        (ReviewInterruption::Timeout, false) => "review_timeout_no_output",
        (ReviewInterruption::Failed, _) => "review_failed",
        (ReviewInterruption::Cancelled, _) => "review_cancelled",
    }
}

fn llm_interruption_stage(interruption: ReviewInterruption) -> StageStatus {
    match interruption {
        ReviewInterruption::Timeout => StageStatus::Timeout,
        ReviewInterruption::Failed => StageStatus::Failed,
        ReviewInterruption::Cancelled => StageStatus::Cancelled,
    }
}

fn interrupted_review_output(
    started: Instant,
    target: ReviewTargetSummary,
    collection: &DiffCollection,
    mut interrupted: InterruptedReview,
) -> ReviewOutput {
    normalize_findings(&mut interrupted.findings, &mut interrupted.warnings);
    let parsed =
        ParsedReviewOutput::from_parts(interrupted.findings, &interrupted.reviewer_summaries);
    let coverage = ReviewCoverage::from_collection_ref(collection);
    let run = if interrupted.interruption == ReviewInterruption::Cancelled || !parsed.has_output() {
        ReviewRun::InterruptedNoOutput {
            target,
            coverage,
            warnings: interrupted.warnings,
            usage: interrupted.usage,
            reason: interrupted.reason,
            interruption: interrupted.interruption,
        }
    } else {
        ReviewRun::InterruptedAfterOutput {
            target,
            coverage,
            parsed,
            warnings: interrupted.warnings,
            usage: interrupted.usage,
            reason: interrupted.reason,
            interruption: interrupted.interruption,
        }
    };
    run.into_output(started.elapsed().as_millis())
}

fn completed_review_status(status: &ReviewStatus, clean_complete: bool) -> ReviewCompletionStatus {
    if matches!(status, ReviewStatus::Failed) {
        ReviewCompletionStatus::Unavailable
    } else if clean_complete {
        ReviewCompletionStatus::Completed
    } else {
        ReviewCompletionStatus::CompletedWithWarnings
    }
}

fn completed_result_contract(
    warnings: &[ReviewWarning],
    has_parsed_output: bool,
    has_diff_coverage_gap: bool,
) -> (
    ReviewStatus,
    FindingsStatus,
    FindingsTrust,
    RetryRecommendation,
) {
    if has_warning(warnings, "model_output_parse") {
        if has_parsed_output {
            (
                ReviewStatus::Partial,
                FindingsStatus::Partial,
                FindingsTrust::Low,
                RetryRecommendation::ReviewFindingsFirst,
            )
        } else {
            (
                ReviewStatus::Failed,
                FindingsStatus::Unavailable,
                FindingsTrust::Low,
                RetryRecommendation::Retry,
            )
        }
    } else if has_warning(warnings, "dropped_findings") && !has_parsed_output {
        (
            ReviewStatus::Failed,
            FindingsStatus::Unavailable,
            FindingsTrust::Low,
            RetryRecommendation::Retry,
        )
    } else if has_diff_coverage_gap {
        (
            ReviewStatus::Partial,
            findings_status_for_completed(warnings),
            findings_trust_for_completed(warnings),
            RetryRecommendation::DoNotRetry,
        )
    } else {
        (
            ReviewStatus::Success,
            findings_status_for_completed(warnings),
            findings_trust_for_completed(warnings),
            RetryRecommendation::DoNotRetry,
        )
    }
}

fn interrupted_contract(
    interruption: ReviewInterruption,
    has_output: bool,
) -> (
    ReviewStatus,
    ReviewCompletionStatus,
    FindingsStatus,
    FindingsTrust,
    RetryRecommendation,
) {
    match (interruption, has_output) {
        (ReviewInterruption::Timeout, true) => (
            ReviewStatus::Partial,
            ReviewCompletionStatus::ModelTimeoutAfterOutput,
            FindingsStatus::Partial,
            FindingsTrust::Partial,
            RetryRecommendation::ReviewFindingsFirst,
        ),
        (ReviewInterruption::Timeout, false) => (
            ReviewStatus::Failed,
            ReviewCompletionStatus::ModelTimeoutNoOutput,
            FindingsStatus::Unavailable,
            FindingsTrust::Low,
            RetryRecommendation::Retry,
        ),
        (ReviewInterruption::Failed, true) => (
            ReviewStatus::Partial,
            ReviewCompletionStatus::ModelFailedAfterOutput,
            FindingsStatus::Partial,
            FindingsTrust::Partial,
            RetryRecommendation::ReviewFindingsFirst,
        ),
        (ReviewInterruption::Failed, false) => (
            ReviewStatus::Failed,
            ReviewCompletionStatus::ModelFailedNoOutput,
            FindingsStatus::Unavailable,
            FindingsTrust::Low,
            RetryRecommendation::Retry,
        ),
        (ReviewInterruption::Cancelled, true | false) => (
            ReviewStatus::Failed,
            ReviewCompletionStatus::Cancelled,
            FindingsStatus::Unavailable,
            FindingsTrust::Low,
            RetryRecommendation::DoNotRetry,
        ),
    }
}

fn finalize_review_output(draft: ReviewOutputDraft) -> ReviewOutput {
    let finding_summary = summarize_findings(&draft.findings);
    let warnings_summary = summarize_warnings(&draft.warnings);
    let output = ReviewOutput {
        status: draft.status,
        review_status: draft.review_status,
        findings_status: draft.findings_status,
        findings_trust: draft.findings_trust,
        stage_status: draft.stage_status,
        finding_summary,
        warnings_summary,
        retry_recommendation: draft.retry_recommendation,
        summary: draft.summary,
        unreviewed: draft.unreviewed,
        findings: draft.findings,
        warnings: draft.warnings,
    };
    debug_assert!(
        review_output_invariant_error(&output).is_none(),
        "invalid commission review output: {:?}",
        review_output_invariant_error(&output)
    );
    output
}

fn review_output_invariant_error(output: &ReviewOutput) -> Option<&'static str> {
    if output.finding_summary.total != output.findings.len() {
        return Some("finding_summary.total must equal findings.len()");
    }
    if matches!(output.status, ReviewStatus::Failed) {
        if !output.findings.is_empty() || output.finding_summary.total != 0 {
            return Some("failed output must not carry findings");
        }
        if output.summary.reviewer_summary.is_some() {
            return Some("failed output must not carry reviewer summary");
        }
    }
    if matches!(output.findings_status, FindingsStatus::Unavailable)
        && (!output.findings.is_empty() || output.finding_summary.total != 0)
    {
        return Some("unavailable findings must not carry findings");
    }
    match output.review_status {
        ReviewCompletionStatus::ModelTimeoutAfterOutput
        | ReviewCompletionStatus::ModelFailedAfterOutput => {
            if !matches!(output.status, ReviewStatus::Partial) {
                return Some("after-output review status requires partial top-level status");
            }
            if output.findings.is_empty() && output.summary.reviewer_summary.is_none() {
                return Some("after-output review status requires parsed output");
            }
        }
        ReviewCompletionStatus::ModelTimeoutNoOutput
        | ReviewCompletionStatus::ModelFailedNoOutput => {
            if !matches!(output.status, ReviewStatus::Failed)
                || !matches!(output.findings_status, FindingsStatus::Unavailable)
            {
                return Some("no-output review status requires failed unavailable result");
            }
        }
        ReviewCompletionStatus::Completed
        | ReviewCompletionStatus::CompletedWithWarnings
        | ReviewCompletionStatus::Cancelled
        | ReviewCompletionStatus::Unavailable
        | ReviewCompletionStatus::Rejected => {}
    }
    None
}

fn classify_llm_error(error: &str) -> ReviewInterruption {
    let lower = error.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        ReviewInterruption::Timeout
    } else {
        ReviewInterruption::Failed
    }
}

fn summarize_findings(findings: &[ReviewFinding]) -> FindingSummary {
    let mut summary = FindingSummary {
        total: findings.len(),
        critical: 0,
        high: 0,
        medium: 0,
        low: 0,
    };
    for finding in findings {
        match finding.severity.as_str() {
            "critical" => summary.critical += 1,
            "high" => summary.high += 1,
            "medium" => summary.medium += 1,
            "low" => summary.low += 1,
            _ => {}
        }
    }
    summary
}

fn summarize_warnings(warnings: &[ReviewWarning]) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut seen = HashSet::new();
    for warning in warnings {
        let summary = match warning.kind.as_str() {
            "model_output_repaired" => "model output repaired".to_string(),
            "model_output_parse" => "model output could not be parsed as JSON".to_string(),
            "review_timeout_partial" => "review request timed out after partial output".to_string(),
            "review_timeout_no_output" => {
                "review request timed out before parsed output".to_string()
            }
            "review_failed" => "review request failed".to_string(),
            "review_cancelled" => "review request was cancelled".to_string(),
            "review_truncated" => "review material was truncated".to_string(),
            "file_too_large" => "some file diffs exceeded review limits".to_string(),
            "unsupported_file" => "some changed files were not reviewable".to_string(),
            "diff_capture_failed" => "some file diffs could not be captured".to_string(),
            "collection_failed" => "review target or diff collection failed".to_string(),
            "untracked_file" => "some untracked files were not reviewed".to_string(),
            "dropped_findings" => warning.message.clone(),
            "invalid_severity" => "some finding severities were normalized".to_string(),
            "invalid_confidence" => "some finding confidences were normalized".to_string(),
            _ => continue,
        };
        if seen.insert(summary.clone()) {
            summaries.push(summary);
        }
    }
    summaries
}

fn findings_status_for_completed(warnings: &[ReviewWarning]) -> FindingsStatus {
    if has_warning(warnings, "dropped_findings") {
        FindingsStatus::Partial
    } else {
        FindingsStatus::Complete
    }
}

fn findings_trust_for_completed(warnings: &[ReviewWarning]) -> FindingsTrust {
    if has_warning(warnings, "model_output_parse") {
        FindingsTrust::Low
    } else if has_warning(warnings, "dropped_findings") {
        FindingsTrust::Partial
    } else if has_normalized_finding_warning(warnings)
        || has_warning(warnings, "model_output_repaired")
    {
        FindingsTrust::Repaired
    } else {
        FindingsTrust::Complete
    }
}

fn has_normalized_finding_warning(warnings: &[ReviewWarning]) -> bool {
    has_warning(warnings, "invalid_severity") || has_warning(warnings, "invalid_confidence")
}

fn diff_stage_status(warnings: &[ReviewWarning], has_unreviewed: bool) -> StageStatus {
    if has_unreviewed
        || warnings.iter().any(|w| {
            matches!(
                w.kind.as_str(),
                "review_truncated"
                    | "file_too_large"
                    | "unsupported_file"
                    | "diff_capture_failed"
                    | "untracked_file"
            )
        })
    {
        StageStatus::Partial
    } else {
        StageStatus::Ok
    }
}

fn json_parse_stage_status(warnings: &[ReviewWarning]) -> StageStatus {
    if has_warning(warnings, "model_output_parse") {
        StageStatus::Failed
    } else if has_warning(warnings, "model_output_repaired") {
        StageStatus::Repaired
    } else {
        StageStatus::Ok
    }
}

fn finding_extraction_stage_status(warnings: &[ReviewWarning]) -> StageStatus {
    if has_warning(warnings, "dropped_findings") {
        StageStatus::Partial
    } else if has_warning(warnings, "model_output_parse") {
        StageStatus::Failed
    } else {
        StageStatus::Ok
    }
}

fn interrupted_json_parse_stage_status(
    warnings: &[ReviewWarning],
    has_output: bool,
) -> StageStatus {
    if !has_output && !has_warning(warnings, "model_output_parse") {
        StageStatus::Skipped
    } else {
        json_parse_stage_status(warnings)
    }
}

fn interrupted_finding_extraction_stage_status(
    warnings: &[ReviewWarning],
    has_output: bool,
) -> StageStatus {
    if !has_output && !has_warning(warnings, "model_output_parse") {
        StageStatus::Skipped
    } else if has_warning(warnings, "model_output_parse") {
        StageStatus::Failed
    } else if has_warning(warnings, "dropped_findings") {
        StageStatus::Partial
    } else {
        StageStatus::Ok
    }
}

fn has_warning(warnings: &[ReviewWarning], kind: &str) -> bool {
    warnings.iter().any(|warning| warning.kind == kind)
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

async fn origin_default_ref(repo: &Path) -> Result<String, String> {
    let remote = git_capture(
        repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )
    .await
    .map_err(|_| {
        "commission_review requires fetched origin default branch ref `origin/HEAD`. Fetch origin before requesting review.".to_string()
    })?;
    let remote = remote.trim();
    if remote.starts_with("refs/remotes/origin/") {
        Ok(remote.to_string())
    } else {
        Err(format!(
            "commission_review expected `origin/HEAD` to resolve to an origin remote-tracking branch, got `{remote}`."
        ))
    }
}

async fn remote_base_ref(
    repo: &Path,
    approved_base_branch: Option<&str>,
) -> Result<String, String> {
    if let Some(base_branch) = approved_base_branch.filter(|branch| !branch.trim().is_empty()) {
        let branch = base_branch
            .trim()
            .strip_prefix("refs/remotes/origin/")
            .or_else(|| base_branch.trim().strip_prefix("origin/"))
            .unwrap_or_else(|| base_branch.trim());
        let remote = format!("refs/remotes/origin/{branch}");
        if git_capture(repo, &["rev-parse", "--verify", "--quiet", &remote])
            .await
            .is_ok()
        {
            Ok(remote)
        } else {
            Err(format!(
                "commission_review requires fetched approved base ref `{remote}`. Fetch origin before requesting review."
            ))
        }
    } else {
        origin_default_ref(repo).await
    }
}

async fn resolve_target(
    ctx: &ToolContext,
    _input: &CommissionReviewInput,
    runtime_base_branch: Option<&str>,
) -> Result<ReviewTarget, String> {
    let repo_root = git_capture(&ctx.working_dir, &["rev-parse", "--show-toplevel"]).await?;
    let repo = PathBuf::from(repo_root.trim());
    let dirty = !git_capture(&repo, &["status", "--porcelain"])
        .await?
        .trim()
        .is_empty();
    let head = git_capture(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;

    if dirty {
        return Err("commission_review refused dirty working tree. Commit or stash changes before requesting review; commission_review only reviews committed changes against the approved origin base branch.".to_string());
    }
    let base = remote_base_ref(&repo, runtime_base_branch).await?;
    Ok(ReviewTarget {
        summary: ReviewTargetSummary {
            kind: ReviewTargetKind::CommittedBranchDiff,
            repo_root: repo.display().to_string(),
            base: base.clone(),
            head: head.trim().to_string(),
            dirty,
        },
        diff_spec: DiffSpec::Range {
            base,
            head: "HEAD".to_string(),
        },
    })
}

#[allow(clippy::too_many_lines)]
async fn collect_diff(target: &ReviewTarget, ctx: &ToolContext) -> Result<DiffCollection, String> {
    let repo = Path::new(&target.summary.repo_root);
    let mut warnings = Vec::new();
    let mut unreviewed = Vec::new();
    let DiffSpec::Range { base, head } = &target.diff_spec;
    let numstat = git_capture_cancel(
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
    .await?;
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
        let DiffSpec::Range { base, head } = &target.diff_spec;
        let merge_base_range = format!("{base}...{head}");
        let capture_result = git_capture_limited_cancel(
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
        .await;
        let (diff, truncated) = match capture_result {
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
        "Brief:\n{}\n\nFocus:\n{}\n\nTarget:\n{}\n\nStats: {} changed files, {} reviewed files, +{}/-{}. Dirty: {}. Chunk {}/{}.\n\nDiff:\n{}",
        input.brief.trim(),
        input.focus.as_deref().unwrap_or("general correctness review"),
        serde_json::to_string_pretty(target).unwrap_or_default(),
        collection.files_changed,
        collection.files_reviewed,
        collection.insertions,
        collection.deletions,
        target.dirty,
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

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::String(value)) => Some(value),
        _ => None,
    })
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
                symbol: f.symbol.filter(|symbol| !symbol.trim().is_empty()),
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
        finding.symbol = finding
            .symbol
            .as_ref()
            .map(|symbol| symbol.trim().to_string())
            .filter(|symbol| !symbol.is_empty());
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

    /// The review target resolves the fetched origin default branch through
    /// `origin/HEAD` instead of assuming a branch name such as `main`.
    #[tokio::test]
    async fn origin_default_ref_uses_origin_head() {
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

        assert!(origin_default_ref(repo).await.is_err());

        git_ok(repo, &["update-ref", "refs/remotes/origin/master", &head]).await;
        git_ok(
            repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/master",
            ],
        )
        .await;
        assert_eq!(
            origin_default_ref(repo).await.unwrap(),
            "refs/remotes/origin/master"
        );
    }

    #[tokio::test]
    async fn dirty_working_tree_is_always_refused() {
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
        git_ok(
            repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )
        .await;
        std::fs::write(repo.join("dirty.rs"), "uncommitted\n").expect("write");

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
        let err = resolve_target(
            &ctx,
            &CommissionReviewInput {
                brief: "ready".to_string(),
                focus: None,
            },
            Some("main"),
        )
        .await
        .expect_err("dirty working tree is refused");

        assert!(err.contains("refused dirty working tree"));
        assert!(err.contains("only reviews committed changes"));
    }

    #[tokio::test]
    async fn resolve_target_uses_approved_remote_base_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).await;
        git_ok(repo, &["config", "user.email", "t@t.t"]).await;
        git_ok(repo, &["config", "user.name", "t"]).await;
        git_ok(repo, &["commit", "-qm", "base", "--allow-empty"]).await;
        let base = git_capture(repo, &["rev-parse", "HEAD"])
            .await
            .expect("base");
        git_ok(repo, &["update-ref", "refs/remotes/origin/master", &base]).await;
        git_ok(repo, &["update-ref", "refs/remotes/origin/develop", &base]).await;
        git_ok(
            repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/master",
            ],
        )
        .await;

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
            },
            Some("develop"),
        )
        .await
        .expect("target resolves");

        assert_eq!(target.summary.base, "refs/remotes/origin/develop");
    }

    /// A diff whose only changed files all exceed the per-file cap reviews
    /// nothing, but must report the coverage gap (partial status + top-level
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
        git_ok(
            repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )
        .await;
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
            status: ReviewStatus::Partial,
            review_status: ReviewCompletionStatus::CompletedWithWarnings,
            findings_status: FindingsStatus::Unavailable,
            findings_trust: FindingsTrust::Low,
            stage_status: ReviewStageStatus {
                target_collection: StageStatus::Ok,
                diff_collection: StageStatus::Partial,
                llm_review: StageStatus::Skipped,
                json_parse: StageStatus::Skipped,
                finding_extraction: StageStatus::Skipped,
            },
            finding_summary: FindingSummary {
                total: 0,
                critical: 0,
                high: 0,
                medium: 0,
                low: 0,
            },
            warnings_summary: Vec::new(),
            retry_recommendation: RetryRecommendation::DoNotRetry,
            summary: ReviewSummary {
                target: ReviewTargetSummary {
                    kind: ReviewTargetKind::CommittedBranchDiff,
                    repo_root: "/r".to_string(),
                    base: "main".to_string(),
                    head: "HEAD".to_string(),
                    dirty: false,
                },
                files_changed: 2,
                files_reviewed: 0,
                insertions: 0,
                deletions: 0,
                elapsed_ms: 0,
                usage: phoenix_core::domain::llm_types::Usage::default(),
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
            },
            runtime_base_branch: Some("main".to_string()),
            approved_working_dir: "/repo/approved".to_string(),
            approved_worktree_path: None,
            approved_head: None,
            approved_base: None,
        };

        assert!(assert_approved_paths_match("/repo/approved", None, &approved).is_ok());
    }

    fn sample_target() -> ReviewTargetSummary {
        ReviewTargetSummary {
            kind: ReviewTargetKind::CommittedBranchDiff,
            repo_root: "/repo".to_string(),
            base: "main".to_string(),
            head: "task".to_string(),
            dirty: false,
        }
    }

    fn sample_collection() -> DiffCollection {
        DiffCollection {
            files_changed: 1,
            files_reviewed: 1,
            insertions: 3,
            deletions: 1,
            body: "diff".to_string(),
            warnings: Vec::new(),
            unreviewed: Vec::new(),
        }
    }

    fn sample_coverage() -> ReviewCoverage {
        ReviewCoverage::from_collection(sample_collection())
    }

    fn assert_review_output_invariants(output: &ReviewOutput) {
        assert_eq!(review_output_invariant_error(output), None);
    }

    fn sample_finding(severity: &str, file: &str, title: &str) -> ReviewFinding {
        ReviewFinding {
            severity: severity.to_string(),
            confidence: "high".to_string(),
            file: file.to_string(),
            line: Some(7),
            symbol: Some("parse_external_models".to_string()),
            title: title.to_string(),
            rationale: "bad".to_string(),
            suggested_fix: "fix".to_string(),
        }
    }

    #[test]
    fn outcome_cancelled_with_buffered_findings_clears_actionable_output() {
        let output = ReviewRun::InterruptedNoOutput {
            target: sample_target(),
            coverage: sample_coverage(),
            warnings: Vec::new(),
            usage: phoenix_core::domain::llm_types::Usage::default(),
            reason: "cancelled".to_string(),
            interruption: ReviewInterruption::Cancelled,
        }
        .into_output(0);

        assert_review_output_invariants(&output);
        assert_eq!(output.status, ReviewStatus::Failed);
        assert_eq!(output.findings_status, FindingsStatus::Unavailable);
        assert!(output.findings.is_empty());
        assert_eq!(output.finding_summary.total, 0);
        assert_eq!(output.summary.reviewer_summary, None);
    }

    #[test]
    fn outcome_after_output_requires_summary_or_findings() {
        let summaries = vec!["Reviewed chunks before timeout".to_string()];
        let output = ReviewRun::InterruptedAfterOutput {
            target: sample_target(),
            coverage: sample_coverage(),
            parsed: ParsedReviewOutput::from_parts(Vec::new(), &summaries),
            warnings: Vec::new(),
            usage: phoenix_core::domain::llm_types::Usage::default(),
            reason: "timed out".to_string(),
            interruption: ReviewInterruption::Timeout,
        }
        .into_output(0);

        assert_review_output_invariants(&output);
        assert_eq!(output.status, ReviewStatus::Partial);
        assert_eq!(
            output.review_status,
            ReviewCompletionStatus::ModelTimeoutAfterOutput
        );
        assert_eq!(output.finding_summary.total, 0);
        assert_eq!(
            output.summary.reviewer_summary.as_deref(),
            Some("Reviewed chunks before timeout")
        );
    }

    #[test]
    fn outcome_collection_failure_marks_exact_failed_stage() {
        let output = ReviewRun::CollectionFailed {
            target: sample_target(),
            stage: CollectionFailureStage::DiffCollection,
            reason: "git diff failed".to_string(),
        }
        .into_output(0);

        assert_review_output_invariants(&output);
        assert_eq!(output.status, ReviewStatus::Failed);
        assert_eq!(output.review_status, ReviewCompletionStatus::Unavailable);
        assert_eq!(output.stage_status.target_collection, StageStatus::Ok);
        assert_eq!(output.stage_status.diff_collection, StageStatus::Failed);
        assert_eq!(output.stage_status.llm_review, StageStatus::Skipped);
    }

    #[test]
    fn outcome_collection_cancellation_marks_stage_cancelled() {
        let output = ReviewRun::CollectionFailed {
            target: sample_target(),
            stage: CollectionFailureStage::DiffCollection,
            reason: "commission_review cancelled while collecting diffs".to_string(),
        }
        .into_output(0);

        assert_review_output_invariants(&output);
        assert_eq!(output.status, ReviewStatus::Failed);
        assert_eq!(output.review_status, ReviewCompletionStatus::Cancelled);
        assert_eq!(output.stage_status.diff_collection, StageStatus::Cancelled);
        assert_eq!(output.retry_recommendation, RetryRecommendation::DoNotRetry);
        assert!(output
            .warnings_summary
            .contains(&"review request was cancelled".to_string()));
    }

    #[test]
    fn non_json_text_is_not_counted_as_parsed_partial_output() {
        let (findings, summary, warnings) = parse_findings("plain reviewer prose");
        let mut reviewer_summaries = Vec::new();
        if !has_warning(&warnings, "model_output_parse") {
            if let Some(summary) = summary.filter(|s| !s.trim().is_empty()) {
                reviewer_summaries.push(summary);
            }
        }
        let output = interrupted_review_output(
            Instant::now(),
            sample_target(),
            &sample_collection(),
            InterruptedReview {
                findings,
                warnings,
                reviewer_summaries,
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reason: "request timed out".to_string(),
                interruption: ReviewInterruption::Timeout,
            },
        );

        assert_eq!(output.status, ReviewStatus::Failed);
        assert_eq!(output.findings_status, FindingsStatus::Unavailable);
        assert_eq!(
            output.review_status,
            ReviewCompletionStatus::ModelTimeoutNoOutput
        );
        assert_eq!(output.stage_status.json_parse, StageStatus::Failed);
        assert_eq!(output.stage_status.finding_extraction, StageStatus::Failed);
    }

    #[test]
    fn completed_parse_failure_without_findings_is_failed_unavailable() {
        let warnings = vec![warning(
            "model_output_parse",
            "reviewer returned non-JSON output",
            None,
        )];
        let (status, findings_status, findings_trust, retry_recommendation) =
            completed_result_contract(&warnings, false, false);

        assert_eq!(status, ReviewStatus::Failed);
        assert_eq!(findings_status, FindingsStatus::Unavailable);
        assert_eq!(findings_trust, FindingsTrust::Low);
        assert_eq!(retry_recommendation, RetryRecommendation::Retry);
        assert_eq!(json_parse_stage_status(&warnings), StageStatus::Failed);
        assert_eq!(
            finding_extraction_stage_status(&warnings),
            StageStatus::Failed
        );
    }

    #[test]
    fn completed_failed_result_uses_unavailable_review_status() {
        assert_eq!(
            completed_review_status(&ReviewStatus::Failed, false),
            ReviewCompletionStatus::Unavailable
        );
        assert_eq!(
            completed_review_status(&ReviewStatus::Success, true),
            ReviewCompletionStatus::Completed
        );
        assert_eq!(
            completed_review_status(&ReviewStatus::Partial, false),
            ReviewCompletionStatus::CompletedWithWarnings
        );
    }

    #[test]
    fn completed_parse_failure_with_parsed_summary_is_partial() {
        let warnings = vec![warning(
            "model_output_parse",
            "later reviewer chunk returned non-JSON output",
            None,
        )];
        let (status, findings_status, findings_trust, retry_recommendation) =
            completed_result_contract(&warnings, true, false);

        assert_eq!(status, ReviewStatus::Partial);
        assert_eq!(findings_status, FindingsStatus::Partial);
        assert_eq!(findings_trust, FindingsTrust::Low);
        assert_eq!(
            retry_recommendation,
            RetryRecommendation::ReviewFindingsFirst
        );
    }

    #[test]
    fn coverage_warnings_make_completed_result_partial() {
        let warnings = vec![warning(
            "unsupported_file",
            "unsupported file type skipped",
            Some("image.png"),
        )];
        let has_diff_coverage_gap = diff_stage_status(&warnings, false) == StageStatus::Partial;
        let (status, findings_status, findings_trust, retry_recommendation) =
            completed_result_contract(&warnings, true, has_diff_coverage_gap);

        assert_eq!(status, ReviewStatus::Partial);
        assert_eq!(findings_status, FindingsStatus::Complete);
        assert_eq!(findings_trust, FindingsTrust::Complete);
        assert_eq!(retry_recommendation, RetryRecommendation::DoNotRetry);
    }

    #[test]
    fn dropped_findings_make_completed_findings_partial() {
        let warnings = vec![warning(
            "dropped_findings",
            "dropped 1 reviewer finding(s) without a file",
            None,
        )];
        let (status, findings_status, findings_trust, retry_recommendation) =
            completed_result_contract(&warnings, true, false);

        assert_eq!(status, ReviewStatus::Success);
        assert_eq!(findings_status, FindingsStatus::Partial);
        assert_eq!(findings_trust, FindingsTrust::Partial);
        assert_eq!(retry_recommendation, RetryRecommendation::DoNotRetry);
    }

    #[test]
    fn all_dropped_findings_without_summary_is_failed_unavailable() {
        let warnings = vec![warning(
            "dropped_findings",
            "dropped 1 reviewer finding(s) without a file",
            None,
        )];
        let (status, findings_status, findings_trust, retry_recommendation) =
            completed_result_contract(&warnings, false, false);

        assert_eq!(status, ReviewStatus::Failed);
        assert_eq!(findings_status, FindingsStatus::Unavailable);
        assert_eq!(findings_trust, FindingsTrust::Low);
        assert_eq!(retry_recommendation, RetryRecommendation::Retry);
    }

    #[test]
    fn cancellation_after_output_is_not_a_deliverable_partial_result() {
        let (status, review_status, findings_status, findings_trust, retry_recommendation) =
            interrupted_contract(ReviewInterruption::Cancelled, true);

        assert_eq!(status, ReviewStatus::Failed);
        assert_eq!(review_status, ReviewCompletionStatus::Cancelled);
        assert_eq!(findings_status, FindingsStatus::Unavailable);
        assert_eq!(findings_trust, FindingsTrust::Low);
        assert_eq!(retry_recommendation, RetryRecommendation::DoNotRetry);
    }

    #[test]
    fn malformed_symbol_does_not_discard_finding() {
        let (findings, summary, warnings) = parse_findings(
            r#"{"summary":"ok","findings":[{"severity":"high","confidence":"high","file":"src/lib.rs","line":7,"symbol":{"name":"parse_external_models"},"title":"bug","rationale":"bad","suggested_fix":"fix"}]}"#,
        );

        assert_eq!(summary.as_deref(), Some("ok"));
        assert!(warnings.is_empty());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].symbol, None);
    }

    #[test]
    fn interrupted_review_preserves_unreviewed_diff_stage_gap() {
        let mut collection = sample_collection();
        collection.unreviewed.push(UnreviewedFile {
            file: "big.rs".to_string(),
            reason: OversizedReason::PerFileCap,
        });
        let output = interrupted_review_output(
            Instant::now(),
            sample_target(),
            &collection,
            InterruptedReview {
                findings: vec![sample_finding("high", "src/lib.rs", "Bug")],
                warnings: Vec::new(),
                reviewer_summaries: Vec::new(),
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reason: "request timed out".to_string(),
                interruption: ReviewInterruption::Timeout,
            },
        );

        assert_eq!(output.status, ReviewStatus::Partial);
        assert_eq!(output.stage_status.diff_collection, StageStatus::Partial);
        assert_eq!(output.unreviewed.len(), 1);
    }

    #[test]
    fn skipped_unsupported_files_mark_diff_stage_partial() {
        let warnings = vec![warning(
            "unsupported_file",
            "unsupported file type skipped",
            Some("x.png"),
        )];
        assert_eq!(diff_stage_status(&warnings, false), StageStatus::Partial);
    }

    #[test]
    fn untracked_files_mark_diff_stage_partial_and_are_summarized() {
        let warnings = vec![warning(
            "untracked_file",
            "untracked file not included in git diff",
            Some("new.rs"),
        )];
        assert_eq!(diff_stage_status(&warnings, false), StageStatus::Partial);
        assert!(summarize_warnings(&warnings)
            .contains(&"some untracked files were not reviewed".to_string()));
    }

    #[test]
    fn summary_only_interruption_does_not_fail_finding_extraction() {
        let output = interrupted_review_output(
            Instant::now(),
            sample_target(),
            &sample_collection(),
            InterruptedReview {
                findings: Vec::new(),
                warnings: Vec::new(),
                reviewer_summaries: vec!["No issues found in reviewed chunks.".to_string()],
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reason: "request timed out".to_string(),
                interruption: ReviewInterruption::Timeout,
            },
        );

        assert_eq!(output.status, ReviewStatus::Partial);
        assert_eq!(
            output.review_status,
            ReviewCompletionStatus::ModelTimeoutAfterOutput
        );
        assert_eq!(output.stage_status.llm_review, StageStatus::Timeout);
        assert_eq!(output.stage_status.json_parse, StageStatus::Ok);
        assert_eq!(output.stage_status.finding_extraction, StageStatus::Ok);
    }

    #[test]
    fn diff_capture_failures_are_summarized_near_status() {
        let summaries = summarize_warnings(&[warning(
            "diff_capture_failed",
            "failed to capture file diff",
            Some("src/lib.rs"),
        )]);
        assert!(summaries.contains(&"some file diffs could not be captured".to_string()));
    }

    #[test]
    fn timeout_before_findings_returns_failed_unavailable() {
        let output = interrupted_review_output(
            Instant::now(),
            sample_target(),
            &sample_collection(),
            InterruptedReview {
                findings: Vec::new(),
                warnings: Vec::new(),
                reviewer_summaries: Vec::new(),
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reason: "request timed out".to_string(),
                interruption: ReviewInterruption::Timeout,
            },
        );

        assert_eq!(output.status, ReviewStatus::Failed);
        assert_eq!(
            output.review_status,
            ReviewCompletionStatus::ModelTimeoutNoOutput
        );
        assert_eq!(output.findings_status, FindingsStatus::Unavailable);
        assert_eq!(output.findings_trust, FindingsTrust::Low);
        assert_eq!(output.stage_status.llm_review, StageStatus::Timeout);
        assert_eq!(output.stage_status.json_parse, StageStatus::Skipped);
        assert_eq!(output.stage_status.finding_extraction, StageStatus::Skipped);
        assert!(output.findings.is_empty());
        assert!(output
            .warnings_summary
            .contains(&"review request timed out before parsed output".to_string()));
    }

    #[test]
    fn timeout_after_output_returns_partial_actionable_output() {
        let output = interrupted_review_output(
            Instant::now(),
            sample_target(),
            &sample_collection(),
            InterruptedReview {
                findings: vec![sample_finding("high", "src/lib.rs", "Bug")],
                warnings: Vec::new(),
                reviewer_summaries: Vec::new(),
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reason: "request timed out".to_string(),
                interruption: ReviewInterruption::Timeout,
            },
        );

        assert_eq!(output.status, ReviewStatus::Partial);
        assert_eq!(
            output.review_status,
            ReviewCompletionStatus::ModelTimeoutAfterOutput
        );
        assert_eq!(output.findings_status, FindingsStatus::Partial);
        assert_eq!(output.findings_trust, FindingsTrust::Partial);
        assert_eq!(
            output.retry_recommendation,
            RetryRecommendation::ReviewFindingsFirst
        );
        assert_eq!(output.finding_summary.total, 1);
        assert_eq!(output.finding_summary.high, 1);
        assert_eq!(
            output.findings[0].symbol.as_deref(),
            Some("parse_external_models")
        );
        assert_eq!(output.stage_status.finding_extraction, StageStatus::Ok);
        assert!(output
            .warnings_summary
            .contains(&"review request timed out after partial output".to_string()));
    }

    #[test]
    fn parse_repair_sets_trust_and_warning_summary() {
        let (findings, _summary, warnings) = parse_findings(
            "prefix {\"findings\":[{\"file\":\"src/lib.rs\",\"symbol\":\" f \",\"title\":\"bug\"}]} suffix",
        );
        let mut normalized = findings;
        let mut all_warnings = warnings;
        normalize_findings(&mut normalized, &mut all_warnings);
        let output = finalize_review_output(ReviewOutputDraft {
            status: ReviewStatus::Success,
            review_status: ReviewCompletionStatus::CompletedWithWarnings,
            findings_status: FindingsStatus::Complete,
            findings_trust: findings_trust_for_completed(&all_warnings),
            stage_status: ReviewStageStatus {
                target_collection: StageStatus::Ok,
                diff_collection: StageStatus::Ok,
                llm_review: StageStatus::Ok,
                json_parse: json_parse_stage_status(&all_warnings),
                finding_extraction: finding_extraction_stage_status(&all_warnings),
            },
            retry_recommendation: RetryRecommendation::DoNotRetry,
            summary: ReviewSummary {
                target: sample_target(),
                files_changed: 1,
                files_reviewed: 1,
                insertions: 1,
                deletions: 0,
                elapsed_ms: 0,
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reviewer_summary: None,
            },
            unreviewed: Vec::new(),
            findings: normalized,
            warnings: all_warnings,
        });

        assert_eq!(output.findings_trust, FindingsTrust::Repaired);
        assert_eq!(output.stage_status.json_parse, StageStatus::Repaired);
        assert!(output
            .warnings_summary
            .contains(&"model output repaired".to_string()));
        assert_eq!(output.findings[0].symbol.as_deref(), Some("f"));
    }

    #[test]
    fn normalized_findings_are_repaired_trust_and_summarized() {
        let mut findings = vec![sample_finding("unexpected", "src/lib.rs", "Bug")];
        findings[0].confidence = "certain".to_string();
        let mut warnings = Vec::new();
        normalize_findings(&mut findings, &mut warnings);

        assert_eq!(
            findings_trust_for_completed(&warnings),
            FindingsTrust::Repaired
        );
        let summaries = summarize_warnings(&warnings);
        assert!(summaries.contains(&"some finding severities were normalized".to_string()));
        assert!(summaries.contains(&"some finding confidences were normalized".to_string()));
    }

    #[test]
    fn severity_summary_counts_normalized_deduped_findings() {
        let mut findings = vec![
            sample_finding("HIGH", "b.rs", "Dup"),
            sample_finding("high", "b.rs", "dup"),
            sample_finding("critical", "a.rs", "Critical"),
            sample_finding("unknown", "c.rs", "Fallback"),
            sample_finding("low", "d.rs", "Low"),
        ];
        let mut warnings = Vec::new();
        normalize_findings(&mut findings, &mut warnings);
        let summary = summarize_findings(&findings);

        assert_eq!(summary.total, 4);
        assert_eq!(summary.critical, 1);
        assert_eq!(summary.high, 1);
        assert_eq!(summary.medium, 1);
        assert_eq!(summary.low, 1);
    }

    #[test]
    fn display_status_panel_preserves_status_field_order() {
        let output = finalize_review_output(ReviewOutputDraft {
            status: ReviewStatus::Failed,
            review_status: ReviewCompletionStatus::Unavailable,
            findings_status: FindingsStatus::Unavailable,
            findings_trust: FindingsTrust::Low,
            stage_status: ReviewStageStatus {
                target_collection: StageStatus::Ok,
                diff_collection: StageStatus::Ok,
                llm_review: StageStatus::Ok,
                json_parse: StageStatus::Failed,
                finding_extraction: StageStatus::Failed,
            },
            retry_recommendation: RetryRecommendation::Retry,
            summary: ReviewSummary {
                target: sample_target(),
                files_changed: 1,
                files_reviewed: 1,
                insertions: 1,
                deletions: 0,
                elapsed_ms: 0,
                usage: phoenix_core::domain::llm_types::Usage::default(),
                reviewer_summary: None,
            },
            unreviewed: Vec::new(),
            findings: Vec::new(),
            warnings: Vec::new(),
        });
        let display = review_display(&output);
        let names: Vec<_> = display["status_panel"]
            .as_array()
            .expect("status panel is an array")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name is a string"))
            .collect();

        assert_eq!(
            names,
            vec![
                "status",
                "review_status",
                "findings_status",
                "findings_trust",
                "finding_summary",
                "warnings_summary",
                "retry_recommendation",
            ]
        );
    }

    #[test]
    fn user_facing_token_fields_are_absent() {
        let output = finalize_review_output(ReviewOutputDraft {
            status: ReviewStatus::Success,
            review_status: ReviewCompletionStatus::Completed,
            findings_status: FindingsStatus::Complete,
            findings_trust: FindingsTrust::Complete,
            stage_status: ReviewStageStatus {
                target_collection: StageStatus::Ok,
                diff_collection: StageStatus::Ok,
                llm_review: StageStatus::Ok,
                json_parse: StageStatus::Ok,
                finding_extraction: StageStatus::Ok,
            },
            retry_recommendation: RetryRecommendation::DoNotRetry,
            summary: ReviewSummary {
                target: sample_target(),
                files_changed: 1,
                files_reviewed: 1,
                insertions: 1,
                deletions: 0,
                elapsed_ms: 0,
                usage: phoenix_core::domain::llm_types::Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cache_creation_tokens: 30,
                    cache_read_tokens: 40,
                },
                reviewer_summary: None,
            },
            unreviewed: Vec::new(),
            findings: Vec::new(),
            warnings: Vec::new(),
        });
        let value = serde_json::to_value(&output).expect("serializes");
        let serialized = serde_json::to_string(&value).expect("stringifies");

        assert!(value.get("summary").is_some());
        assert!(!serialized.contains("input_tokens"));
        assert_eq!(value["finding_summary"]["total"], 0);
        assert!(!serialized.contains("findings_count"));
        assert!(!serialized.contains("output_tokens"));
        assert!(!serialized.contains("cache_creation_tokens"));
        assert!(!serialized.contains("cost"));
    }
    #[test]
    fn findings_are_normalized_sorted_and_deduped() {
        let mut findings = vec![
            ReviewFinding {
                severity: "LOW".to_string(),
                confidence: "certain".to_string(),
                file: "b.rs".to_string(),
                line: Some(2),
                symbol: None,
                title: "Dup".to_string(),
                rationale: String::new(),
                suggested_fix: String::new(),
            },
            ReviewFinding {
                severity: "critical".to_string(),
                confidence: "high".to_string(),
                file: "a.rs".to_string(),
                line: Some(1),
                symbol: Some("parse_external_models".to_string()),
                title: "Bad".to_string(),
                rationale: String::new(),
                suggested_fix: String::new(),
            },
            ReviewFinding {
                severity: "low".to_string(),
                confidence: "low".to_string(),
                file: "b.rs".to_string(),
                line: Some(2),
                symbol: None,
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
