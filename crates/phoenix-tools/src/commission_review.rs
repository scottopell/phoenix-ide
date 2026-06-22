//! Phoenix-native commission review tool.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use phoenix_core::domain::llm_types::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, PromptCacheKey, SystemContent,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

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

#[derive(Debug, Serialize)]
struct ReviewOutput {
    status: ReviewStatus,
    summary: ReviewSummary,
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
                    "findings": &out.findings,
                    "warnings": &out.warnings,
                });
                ToolOutput::success(pretty_json(&out)).with_display(display)
            }
            Err(err) => ToolOutput::error(err),
        }
    }
}

async fn run_review(input: Value, ctx: ToolContext) -> Result<ReviewOutput, String> {
    let started = Instant::now();
    let approved: ApprovedCommissionReviewInput = serde_json::from_value(input)
        .map_err(|e| format!("Invalid approved commission_review input: {e}"))?;
    assert_approved_context_has_not_drifted(&ctx, &approved)?;
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
        return Ok(ReviewOutput {
            status: ReviewStatus::Skipped,
            summary: ReviewSummary {
                target: target.summary,
                files_changed: collection.files_changed,
                files_reviewed: 0,
                insertions: collection.insertions,
                deletions: collection.deletions,
                findings_count: 0,
                elapsed_ms: started.elapsed().as_millis(),
                input_tokens: None,
                output_tokens: None,
                reviewer_summary: Some("No reviewable text diff was found".to_string()),
            },
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

    for (index, chunk) in chunks.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err("commission_review cancelled during LLM review".to_string());
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
            () = ctx.cancel.cancelled() => return Err("commission_review cancelled during LLM review".to_string()),
            response = service.complete(&request) => response.map_err(|e| format!("commission_review LLM review failed: {e}"))?,
        };
        input_tokens += response.usage.input_tokens;
        output_tokens += response.usage.output_tokens;
        let (mut chunk_findings, chunk_summary, chunk_warnings) = parse_findings(&response.text());
        findings.append(&mut chunk_findings);
        if let Some(summary) = chunk_summary.filter(|s| !s.trim().is_empty()) {
            reviewer_summaries.push(summary);
        }
        warnings.extend(chunk_warnings);
    }

    normalize_findings(&mut findings, &mut warnings);
    warnings.extend(collection.warnings);
    let status = if warnings.is_empty() {
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
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            reviewer_summary: if reviewer_summaries.is_empty() {
                None
            } else {
                Some(reviewer_summaries.join("\n\n"))
            },
        },
        findings,
        warnings,
    })
}

fn assert_approved_context_has_not_drifted(
    ctx: &ToolContext,
    approved: &ApprovedCommissionReviewInput,
) -> Result<(), String> {
    let current_cwd = ctx.working_dir.display().to_string();
    if current_cwd != approved.approved_working_dir {
        return Err(format!(
            "commission_review target changed after approval: working directory was `{}` at approval time but is now `{current_cwd}`. Request review again.",
            approved.approved_working_dir
        ));
    }

    let current_worktree = ctx
        .worktree_path
        .as_ref()
        .map(|path| path.display().to_string());
    if current_worktree != approved.approved_worktree_path {
        return Err(format!(
            "commission_review target changed after approval: worktree was `{:?}` at approval time but is now `{:?}`. Request review again.",
            approved.approved_worktree_path,
            current_worktree
        ));
    }
    Ok(())
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
        let base = runtime_base_branch.unwrap_or("main").to_string();
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

async fn collect_diff(target: &ReviewTarget, ctx: &ToolContext) -> Result<DiffCollection, String> {
    let repo = Path::new(&target.summary.repo_root);
    let mut warnings = Vec::new();
    let numstat = match &target.diff_spec {
        DiffSpec::Range { base, head } => {
            git_capture(repo, &["diff", "--numstat", base, head]).await?
        }
        DiffSpec::Workspace => git_capture(repo, &["diff", "--numstat", "HEAD", "--"]).await?,
    };
    let mut insertions = 0;
    let mut deletions = 0;
    let mut files = Vec::new();
    for line in numstat.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() >= 3 {
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
            DiffSpec::Range { base, head } => git_capture(repo, &["diff", base, head, "--", file])
                .await
                .unwrap_or_default(),
            DiffSpec::Workspace => git_capture(repo, &["diff", "HEAD", "--", file])
                .await
                .unwrap_or_default(),
        };
        if diff.len() > MAX_FILE_BYTES {
            warnings.push(warning(
                "file_too_large",
                "file diff exceeded per-file review cap",
                Some(file),
            ));
            continue;
        }
        if body.len() + diff.len() > MAX_REVIEW_BYTES {
            warnings.push(warning(
                "review_truncated",
                "review diff exceeded total review cap",
                Some(file),
            ));
            break;
        }
        if !diff.trim().is_empty() {
            files_reviewed += 1;
            body.push_str(&format!("\n\n--- FILE: {file} ---\n{diff}"));
        }
    }

    Ok(DiffCollection {
        files_changed: files.len(),
        files_reviewed,
        insertions,
        deletions,
        body,
        warnings,
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
        let start = cleaned.find('{').unwrap_or(0);
        let end = cleaned
            .rfind('}')
            .map(|idx| idx + 1)
            .unwrap_or(cleaned.len());
        serde_json::from_str::<ModelReviewResponse>(&cleaned[start..end])
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

async fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
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

fn pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{\"status\":\"failed\"}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
