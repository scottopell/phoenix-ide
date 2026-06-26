//! Keyword search tool - conceptual code search
//!
//! REQ-KWS-001: Conceptual Search
//! REQ-KWS-002: Search Scope
//! REQ-KWS-003: Result Filtering
//! REQ-KWS-004: Tool Schema
//! REQ-KWS-005: LLM Selection

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use phoenix_core::domain::llm_types::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, PromptCacheKey, SystemContent,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const MAX_TERM_RESULTS: usize = 64 * 1024; // 64KB per term
const MAX_COMBINED_RESULTS: usize = 128 * 1024; // 128KB combined

/// Preferred models for filtering (fast and cheap)
const PREFERRED_MODELS: &[&str] = &["claude-haiku-4-5", "claude-sonnet-4-5", "claude-sonnet-4-6"];

use phoenix_core::llm_language::KEYWORD_SEARCH_FILTER_SYSTEM as FILTER_SYSTEM_PROMPT;

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

    /// Run ripgrep with given terms.
    ///
    /// Cooperative cancellation (REQ-BED-005): the spawned `rg` child is raced
    /// against `cancel.cancelled()`. On cancel the child is killed and reaped
    /// promptly (`Child::kill` = `start_kill` + await exit, with `kill_on_drop` as
    /// a backstop) and an error returns, so a search of a huge tree (e.g. an
    /// unbounded scan) cannot block the tool task indefinitely. The executor's
    /// deadline backstop (REQ-BED-005a) is then only a safety net for tools that
    /// are not cooperative.
    async fn ripgrep(
        &self,
        dir: &PathBuf,
        terms: &[String],
        cancel: &CancellationToken,
    ) -> Result<String, String> {
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
            .stderr(Stdio::piped())
            // Kill the child if the JoinHandle::abort drops this future before
            // the select below runs — backstop for the deadline-abort path.
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to run ripgrep: {e}"))?;

        // Take the piped handles so we can drain them concurrently with a
        // borrowing `child.wait()`. Keeping ownership of `child` (rather than
        // consuming it via `wait_with_output`) is what lets the cancel branch
        // deterministically `kill()` (start_kill + reap) the OS process.
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| "ripgrep stdout pipe missing".to_string())?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| "ripgrep stderr pipe missing".to_string())?;

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        let drain = async {
            tokio::try_join!(
                stdout_pipe.read_to_end(&mut stdout_buf),
                stderr_pipe.read_to_end(&mut stderr_buf),
            )
        };

        let status = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // Cancellation requested: explicitly kill AND reap the child so
                // a zombie cannot pile up while tokio's `kill_on_drop` reaper
                // gets around to it. `Child::kill` does `start_kill` + awaits the
                // exit. `kill_on_drop(true)` remains as a belt-and-suspenders
                // backstop for the abort-without-select path.
                let _ = child.kill().await;
                return Err("ripgrep cancelled".to_string());
            }
            res = async { tokio::join!(child.wait(), drain) } => {
                let (wait_res, drain_res) = res;
                drain_res.map_err(|e| format!("Failed to read ripgrep output: {e}"))?;
                wait_res.map_err(|e| format!("Failed to run ripgrep: {e}"))?
            }
        };

        // Exit code 1 = no matches (not an error)
        if status.code() == Some(1) {
            return Ok("No matches found".to_string());
        }

        if !status.success() && status.code() != Some(1) {
            let stderr = String::from_utf8_lossy(&stderr_buf);
            return Err(format!("ripgrep failed: {stderr}"));
        }

        Ok(String::from_utf8_lossy(&stdout_buf).to_string())
    }

    /// Select an LLM for filtering
    fn select_filter_llm(
        ctx: &ToolContext,
    ) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
        // Try preferred models in order
        for model_id in PREFERRED_MODELS {
            if let Some(svc) = ctx.llm_selector().get(model_id) {
                return Some(svc);
            }
        }
        // Fall back to any available model
        ctx.llm_selector().default_service()
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
            // Shared by every keyword-search filter call so FILTER_SYSTEM_PROMPT caches.
            cache_key: PromptCacheKey::stable("keyword-search-filter"),
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
    // clearable: re-queryable read — see specs/stale-tool-results (REQ-STR-002).
    fn clearable(&self) -> bool {
        true
    }

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
            match self
                .ripgrep(&search_root, std::slice::from_ref(term), &ctx.cancel)
                .await
            {
                Ok(result) => {
                    if result.len() <= MAX_TERM_RESULTS {
                        usable_terms.push(term.clone());
                    } else {
                        tracing::debug!(term = %term, size = result.len(), "Skipping broad term");
                    }
                }
                Err(e) => {
                    // Stop prechecking the moment cancellation fires — otherwise
                    // a large term list spawns and kills one `rg` per remaining
                    // term, churning processes and delaying the cooperative
                    // cancel path. ripgrep() returns Err on cancel.
                    if ctx.cancel.is_cancelled() {
                        return ToolOutput::error("keyword_search cancelled");
                    }
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
            match self.ripgrep(&search_root, &usable_terms, &ctx.cancel).await {
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
                    format!(
                        "{}\n\n[results truncated]",
                        results.get(..8000).unwrap_or(&results)
                    )
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
    use crate::browser::BrowserSessionManager;
    use std::time::Duration;

    fn test_context(working_dir: PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
            None,
        )
    }

    #[test]
    fn test_find_search_root() {
        let ctx = test_context(PathBuf::from("/tmp"));
        let root = KeywordSearchTool::find_search_root(&ctx);
        // Should fall back to working dir since /tmp isn't a git repo
        assert_eq!(root, PathBuf::from("/tmp"));
    }

    /// Returns true if an `rg` process whose argv mentions `needle` is alive.
    /// We match on the unique search term, which `ripgrep()` passes verbatim as
    /// `-e <term>` — the scanned directory is set via `current_dir` and so does
    /// not appear in argv, making the term the only reliable argv marker.
    fn rg_child_alive(needle: &str) -> bool {
        let out = std::process::Command::new("pgrep")
            .args(["-f", needle])
            .output();
        match out {
            Ok(out) => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            // pgrep unavailable: treat as "not alive" so the test degrades to a
            // no-op rather than a false failure on platforms without it.
            Err(_) => false,
        }
    }

    /// REQ-BED-005: `ripgrep()` kills and reaps its `rg` child on cancellation
    /// rather than blocking until the scan completes or leaving a zombie.
    ///
    /// Deterministic by construction: the token is cancelled BEFORE the call, so
    /// the biased `select!` in `ripgrep()` always takes the cancel branch — the
    /// outcome cannot depend on whether `rg` outran a timer, which is what made
    /// the earlier "cancel once the child is observed live" version flake under
    /// heavy parallel test load (the scan could finish before it was observed).
    /// The child is still spawned and then killed+reaped via `Child::kill().await`,
    /// so the reap path is exercised.
    #[tokio::test]
    async fn ripgrep_reaps_child_on_cancel() {
        // Small tree — correctness no longer depends on scan duration.
        let dir = tempfile::tempdir().expect("create tempdir");
        for f in 0..8 {
            std::fs::write(
                dir.path().join(format!("f{f}.txt")),
                "lorem ipsum dolor sit amet\n".repeat(64),
            )
            .unwrap();
        }

        let tool = KeywordSearchTool;
        // Unique per-run term so `pgrep -f <term>` cannot collide with another
        // test's `rg` child during the reap check.
        let needle = format!("zzz_nonexistent_term_{}_zzz", std::process::id());
        let terms = vec![needle.clone()];

        // Pre-cancelled: the biased select returns the cancel branch immediately,
        // after the child has been spawned — no race on scan timing.
        let cancel = CancellationToken::new();
        cancel.cancel();

        let start = std::time::Instant::now();
        let result = tool
            .ripgrep(&dir.path().to_path_buf(), &terms, &cancel)
            .await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "ripgrep with a cancelled token must return an error, got {result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "cancelled ripgrep must return promptly, took {elapsed:?}"
        );

        // `Child::kill().await` reaps before `ripgrep()` returns; confirm no `rg`
        // child for our term lingers (brief grace for OS-level reaping).
        let reaped_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while rg_child_alive(&needle) && std::time::Instant::now() < reaped_deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !rg_child_alive(&needle),
            "rg child for term {needle} should have been reaped"
        );
    }
}
