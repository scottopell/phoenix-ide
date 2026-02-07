//! Keyword search tool - conceptual code search
//!
//! REQ-KWS-001: Conceptual Search
//! REQ-KWS-002: Search Scope
//! REQ-KWS-003: Result Filtering
//! REQ-KWS-004: Tool Schema
//! REQ-KWS-005: LLM Selection

use super::{Tool, ToolContext, ToolOutput};
use crate::llm::{ContentBlock, LlmMessage, LlmRequest, MessageRole, SystemContent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

const MAX_TERM_RESULTS: usize = 64 * 1024; // 64KB per term
const MAX_COMBINED_RESULTS: usize = 128 * 1024; // 128KB combined

/// Preferred models for filtering (fast and cheap)
const PREFERRED_MODELS: &[&str] = &["claude-3.5-haiku", "claude-3.5-sonnet", "claude-4-sonnet"];

const FILTER_SYSTEM_PROMPT: &str = r#"You are a code search relevance evaluator. Your task is to analyze ripgrep results and determine which files are most relevant to the user's query.

INPUT FORMAT:
- You will receive ripgrep output containing file matches for keywords with 10 lines of context
- At the end will be the original search query

ANALYSIS INSTRUCTIONS:
1. Examine each file match and its surrounding context
2. Evaluate relevance to the query based on:
   - Direct relevance to concepts in the query
   - Implementation of functionality described in the query
   - Evidence of patterns or systems related to the query
3. Exercise strict judgment - only return files that are genuinely relevant

OUTPUT FORMAT:
Respond with a plain text list of the most relevant files in decreasing order of relevance:

/path/to/most/relevant/file: Concise relevance explanation
/path/to/second/file: Concise relevance explanation
...

IMPORTANT:
- Only include files with meaningful relevance to the query
- Keep it short, don't blather
- Do NOT list all files that had keyword matches
- Focus on quality over quantity
- If no files are truly relevant, return "No relevant files found"
- Use absolute file paths"#;

#[derive(Debug, Deserialize)]
struct KeywordSearchInput {
    query: String,
    search_terms: Vec<String>,
}

/// Keyword search tool
///
/// REQ-BASH-010: Stateless - uses `ToolContext` for `working_dir` and `llm_registry`
pub struct KeywordSearchTool;

impl KeywordSearchTool {
    /// Find git repository root or fall back to working directory
    fn find_search_root(ctx: &ToolContext) -> PathBuf {
        let mut current = ctx.working_dir.clone();
        loop {
            if current.join(".git").exists() {
                return current;
            }
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => return ctx.working_dir.clone(),
            }
        }
    }

    /// Run ripgrep with given terms
    async fn ripgrep(&self, dir: &PathBuf, terms: &[String]) -> Result<String, String> {
        let mut cmd = Command::new("rg");
        cmd.args(["-C", "10"]) // 10 lines context
            .arg("-i") // Case insensitive
            .arg("--line-number")
            .arg("--with-filename");

        for term in terms {
            cmd.args(["-e", term]);
        }

        cmd.current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to run ripgrep: {e}"))?;

        // Exit code 1 = no matches (not an error)
        if output.status.code() == Some(1) {
            return Ok("No matches found".to_string());
        }

        if !output.status.success() && output.status.code() != Some(1) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("ripgrep failed: {stderr}"));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Select an LLM for filtering
    fn select_filter_llm(ctx: &ToolContext) -> Option<Arc<dyn crate::llm::LlmService>> {
        // Try preferred models in order
        for model_id in PREFERRED_MODELS {
            if let Some(svc) = ctx.llm_registry().get(model_id) {
                return Some(svc);
            }
        }
        // Fall back to any available model
        ctx.llm_registry().default()
    }

    /// Filter results using LLM
    async fn filter_with_llm(
        &self,
        ctx: &ToolContext,
        query: &str,
        search_root: &Path,
        results: &str,
    ) -> Result<String, String> {
        let llm = Self::select_filter_llm(ctx).ok_or("No LLM available for filtering")?;

        let user_content = format!(
            "Search root: {}\n\nRipgrep results:\n{}\n\nOriginal query: {}",
            search_root.display(),
            results,
            query
        );

        let request = LlmRequest {
            system: vec![SystemContent::new(FILTER_SYSTEM_PROMPT)],
            messages: vec![LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(user_content)],
            }],
            tools: vec![],
            max_tokens: Some(4096),
        };

        let response = llm
            .complete(&request)
            .await
            .map_err(|e| format!("LLM filtering failed: {e}"))?;

        Ok(response.text())
    }
}

#[async_trait]
impl Tool for KeywordSearchTool {
    fn name(&self) -> &'static str {
        "keyword_search"
    }

    fn description(&self) -> String {
        r"keyword_search locates files with a search-and-filter approach.
Use when navigating unfamiliar codebases with only conceptual understanding or vague user questions.

Effective use:
- Provide a detailed query for accurate relevance ranking
- Prefer MANY SPECIFIC terms over FEW GENERAL ones (high precision beats high recall)
- Order search terms by importance (most important first)
- Supports regex search terms for flexible matching

IMPORTANT: Do NOT use this tool if you have precise information like log lines, error messages, stack traces, filenames, or symbols. Use direct approaches (rg, cat, etc.) instead.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query", "search_terms"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "A detailed statement of what you're trying to find or learn."
                },
                "search_terms": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of search terms in descending order of importance."
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: KeywordSearchInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        if input.search_terms.is_empty() {
            return ToolOutput::error("At least one search term is required");
        }

        let search_root = Self::find_search_root(&ctx);

        // Filter out overly broad terms
        let mut usable_terms = Vec::new();
        for term in &input.search_terms {
            match self.ripgrep(&search_root, std::slice::from_ref(term)).await {
                Ok(result) => {
                    if result.len() <= MAX_TERM_RESULTS {
                        usable_terms.push(term.clone());
                    } else {
                        tracing::debug!(term = %term, size = result.len(), "Skipping broad term");
                    }
                }
                Err(e) => {
                    tracing::warn!(term = %term, error = %e, "Error checking term");
                }
            }
        }

        if usable_terms.is_empty() {
            return ToolOutput::error(
                "Each of those search terms yielded too many results. Try more specific terms.",
            );
        }

        // Search with usable terms, peeling off until results fit
        let mut results = String::new();
        while !usable_terms.is_empty() {
            match self.ripgrep(&search_root, &usable_terms).await {
                Ok(r) => {
                    if r.len() <= MAX_COMBINED_RESULTS {
                        results = r;
                        break;
                    }
                    // Too large, remove lowest priority term
                    usable_terms.pop();
                }
                Err(e) => return ToolOutput::error(e),
            }
        }

        if results.is_empty() || results == "No matches found" {
            return ToolOutput::success("No matches found for the given search terms.");
        }

        // Filter with LLM
        match self
            .filter_with_llm(&ctx, &input.query, &search_root, &results)
            .await
        {
            Ok(filtered) => ToolOutput::success(filtered),
            Err(e) => {
                // If LLM fails, return raw results (truncated)
                tracing::warn!(error = %e, "LLM filtering failed, returning raw results");
                let truncated = if results.len() > 8000 {
                    format!("{}\n\n[results truncated]", &results[..8000])
                } else {
                    results
                };
                ToolOutput::success(truncated)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browser::BrowserSessionManager;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
        )
    }

    #[test]
    fn test_find_search_root() {
        let tool = KeywordSearchTool;
        let ctx = test_context(PathBuf::from("/tmp"));
        let root = KeywordSearchTool::find_search_root(&ctx);
        // Should fall back to working dir since /tmp isn't a git repo
        assert_eq!(root, PathBuf::from("/tmp"));
    }
}
