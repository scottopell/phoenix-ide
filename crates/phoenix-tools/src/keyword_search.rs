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
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// A term whose running match total crosses this is rejected as too broad. The
/// probe kills `rg` the instant the total is exceeded, so the cost of rejecting
/// a broad term is O(limit), independent of the size of the tree being scanned.
const BROAD_TERM_MATCH_LIMIT: usize = 400;
/// Always-on ceiling on the combined context output. The scan is killed once
/// stdout crosses this, even inside a legitimate single repo.
const MAX_COMBINED_RESULTS: usize = 128 * 1024; // 128KB combined

use phoenix_core::llm_language::KEYWORD_SEARCH_FILTER_SYSTEM as FILTER_SYSTEM_PROMPT;

#[derive(Debug, Deserialize)]
struct KeywordSearchInput {
    query: String,
    search_terms: Vec<String>,
}

/// Parse the trailing `:count` from one `rg --count-matches` output line.
/// The line is `path:count`; the path may be non-UTF-8, but the count after the
/// last `:` is ASCII, so we slice from the last colon and parse that lossily.
fn parse_trailing_count(line: &[u8]) -> Option<usize> {
    let colon = line.iter().rposition(|&b| b == b':')?;
    let tail = std::str::from_utf8(&line[colon + 1..]).ok()?;
    tail.trim().parse::<usize>().ok()
}

/// Build the incompleteness notes for a result, one bracketed `[...]` line per
/// applicable cause. Bracketed so the UI renderer surfaces them as notes rather
/// than parsing them as file hits (see `parseKeywordSearchOutput`). Every result
/// path funnels through this so no signal is dropped on any branch.
fn incompleteness_notes(skipped_broad: usize, dropped: usize, capped: bool) -> String {
    let mut s = String::new();
    if skipped_broad > 0 {
        let _ = write!(
            s,
            "\n\n[note: {skipped_broad} search term(s) skipped as too broad — narrow them to include their matches.]"
        );
    }
    if dropped > 0 {
        let _ = write!(
            s,
            "\n\n[note: {dropped} lower-priority term(s) dropped to fit the result budget — narrow your terms for full coverage.]"
        );
    }
    if capped {
        s.push_str(
            "\n\n[results truncated: search scope is large — use more specific search terms for complete results.]",
        );
    }
    s
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

    /// Spawn `rg` with the common flags plus `extra_args` and per-term `-e`
    /// switches, wiring stdin to null and the requested stdio for stdout/stderr.
    ///
    /// `kill_on_drop(true)` is the backstop for the deadline-abort path
    /// (REQ-BED-005a): if this future is aborted before a `select!` gets to
    /// `kill()`, dropping the child still reaps it.
    fn spawn_rg(
        dir: &Path,
        extra_args: &[&str],
        terms: &[String],
        stderr: Stdio,
    ) -> Result<tokio::process::Child, String> {
        let mut cmd = Command::new("rg");
        cmd.args(extra_args).arg("-i"); // case insensitive
        for term in terms {
            cmd.args(["-e", term]);
        }
        cmd.current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .kill_on_drop(true);
        cmd.spawn()
            .map_err(|e| format!("Failed to run ripgrep: {e}"))
    }

    /// Count matches for a single term, aborting the scan the instant the
    /// running total crosses `BROAD_TERM_MATCH_LIMIT`.
    ///
    /// This is the breadth probe. `rg --count-matches` emits `path:count` lazily
    /// as it walks; we accumulate the total and early-exit (kill `rg`) once it
    /// crosses the limit, so a broad term is rejected after finding ~limit
    /// matches rather than after walking the entire tree — an intentionally
    /// broad search root (a multi-repo container) stays cheap. `--count-matches`
    /// (not `--count`) counts individual matches, so a term repeated many times
    /// on one long line still reads as broad. `--max-count = limit + 1` makes a
    /// single file that alone exceeds the limit trip the early exit and bounds
    /// scanning of a many-line file (rg stops after limit+1 matching lines). A
    /// term dense on one enormous single line is read in full — bounded by that
    /// file's size, and fast in practice; files are deliberately not size-capped
    /// so the search never silently omits a file it was asked to cover.
    ///
    /// Returns the accumulated match count (just past the limit when the early
    /// exit fires). A non-`{matches, no-matches}` exit status — e.g. an invalid
    /// regex term, which yields no stdout — is surfaced as `Err` so the caller
    /// skips that one term instead of feeding a bad pattern into the combined
    /// scan. `stderr` is discarded: per-file IO errors do not affect breadth.
    ///
    /// Cooperative cancellation (REQ-BED-005): the child is raced against
    /// `cancel` and killed+reaped on cancel.
    async fn count_matches(
        &self,
        dir: &Path,
        term: &str,
        cancel: &CancellationToken,
    ) -> Result<usize, String> {
        // Cap per-file matches at the breadth limit + 1. This bounds each
        // file's scan (rg stops once a file hits the cap) *and* makes a single
        // file that alone exceeds the limit trip the early exit — a lower cap
        // would let a term dense in one generated file/log report a small count
        // and be wrongly accepted as narrow.
        let max_count = (BROAD_TERM_MATCH_LIMIT + 1).to_string();
        let mut child = Self::spawn_rg(
            dir,
            // `--count-matches`, not `--count`: the latter counts matching
            // *lines*, so a broad term repeated thousands of times on one long
            // line (a minified bundle) would report `1` and be wrongly accepted.
            &["--count-matches", "--max-count", &max_count],
            std::slice::from_ref(&term.to_string()),
            Stdio::null(),
        )?;
        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| "ripgrep stdout pipe missing".to_string())?;
        // Read byte-delimited lines rather than `lines()`: a non-UTF-8 file path
        // in `rg`'s output would make a UTF-8 line decoder error and abort the
        // whole probe. We only need the trailing `:count`, which is ASCII, so we
        // parse it out of the raw bytes and ignore the (possibly non-UTF-8) path.
        let mut reader = BufReader::new(stdout_pipe);

        let mut total = 0usize;
        let mut line = Vec::new();
        loop {
            line.clear();
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return Err("ripgrep cancelled".to_string());
                }
                read = reader.read_until(b'\n', &mut line) => {
                    let n_read = read.map_err(|e| format!("Failed to read ripgrep output: {e}"))?;
                    if n_read == 0 {
                        break; // EOF
                    }
                    // `rg --count-matches` prints `path:count`; the count is the
                    // trailing field after the last colon.
                    if let Some(n) = parse_trailing_count(&line) {
                        total += n;
                        if total > BROAD_TERM_MATCH_LIMIT {
                            let _ = child.kill().await;
                            return Ok(total);
                        }
                    }
                }
            }
        }
        // Natural EOF: rg's exit status distinguishes an error (e.g. an invalid
        // regex term, which produces no stdout) from 0 = matches / 1 = no
        // matches. Surface the error so the caller skips just this term rather
        // than feeding an invalid pattern into the combined scan and failing the
        // whole search.
        let status = child
            .wait()
            .await
            .map_err(|e| format!("Failed to run ripgrep: {e}"))?;
        match status.code() {
            Some(0 | 1) => Ok(total),
            other => Err(format!(
                "ripgrep exited with status {other:?} (invalid term?)"
            )),
        }
    }

    /// Run ripgrep with 10 lines of context for `terms`, streaming stdout into a
    /// buffer capped at `max_bytes`. Returns `(output, truncated)`.
    ///
    /// The cap is enforced *during* the read: the child is killed the moment the
    /// buffer crosses `max_bytes`, so a term that slips past the breadth probe
    /// still cannot buffer an unbounded tree. This is the always-on output
    /// ceiling that fires even inside a legitimate single repo.
    ///
    /// Cooperative cancellation (REQ-BED-005): the child is raced against
    /// `cancel` and killed+reaped on cancel.
    async fn ripgrep_capped(
        &self,
        dir: &Path,
        terms: &[String],
        cancel: &CancellationToken,
        max_bytes: usize,
    ) -> Result<(String, bool), String> {
        let mut child = Self::spawn_rg(
            dir,
            &["-C", "10", "--line-number", "--with-filename"],
            terms,
            Stdio::piped(),
        )?;
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| "ripgrep stdout pipe missing".to_string())?;
        // Drain stderr concurrently so a full stderr pipe can't block `rg`.
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| "ripgrep stderr pipe missing".to_string())?;
        let stderr_task = tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut b).await;
            b
        });

        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut truncated = false;
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let _ = child.kill().await;
                    stderr_task.abort();
                    return Err("ripgrep cancelled".to_string());
                }
                read = stdout_pipe.read(&mut chunk) => {
                    let n = read.map_err(|e| format!("Failed to read ripgrep output: {e}"))?;
                    if n == 0 {
                        break; // EOF
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() >= max_bytes {
                        buf.truncate(max_bytes);
                        truncated = true;
                        let _ = child.kill().await;
                        break;
                    }
                }
            }
        }

        // When we hit the cap the child was killed; its exit status is a signal,
        // not a search result, so only interpret status on a natural EOF.
        if truncated {
            stderr_task.abort();
        } else {
            let status = child
                .wait()
                .await
                .map_err(|e| format!("Failed to run ripgrep: {e}"))?;
            if status.code() == Some(1) {
                return Ok(("No matches found".to_string(), false)); // 1 = no matches
            }
            if !status.success() {
                let stderr =
                    String::from_utf8_lossy(&stderr_task.await.unwrap_or_default()).into_owned();
                return Err(format!("ripgrep failed: {stderr}"));
            }
        }

        Ok((String::from_utf8_lossy(&buf).to_string(), truncated))
    }

    /// Probe every term and return those usable for the combined scan (positive
    /// match count, not over-broad). Zero-count terms are dropped — they add
    /// nothing to the combined OR-scan, so keeping them would force a second
    /// full-tree scan only to rediscover they match nothing.
    ///
    /// On a terminal condition returns `Err(ToolOutput)` — the response the
    /// caller returns directly: every probe failed (surface an error rather than
    /// mask infra failure such as ripgrep missing as an empty result); broadness
    /// was the sole reason nothing is usable (ask for narrower terms); the
    /// probes ran and found nothing, possibly with some broad terms skipped (no
    /// matches); or cancellation fired mid-probe.
    /// Returns `(usable_terms, broad_count)` on success — `broad_count` is the
    /// number of terms skipped as too broad, which the caller surfaces as an
    /// incompleteness note even when usable terms remain.
    async fn probe_usable_terms(
        &self,
        search_root: &Path,
        terms: &[String],
        cancel: &CancellationToken,
    ) -> Result<(Vec<String>, usize), ToolOutput> {
        let mut usable_terms = Vec::new();
        let mut broad_count = 0usize;
        let mut any_zero = false;
        let mut any_ok = false;
        for term in terms {
            match self.count_matches(search_root, term, cancel).await {
                Ok(0) => {
                    any_ok = true;
                    any_zero = true;
                    tracing::debug!(term = %term, "No matches for term");
                }
                Ok(count) if count <= BROAD_TERM_MATCH_LIMIT => {
                    any_ok = true;
                    usable_terms.push(term.clone());
                }
                Ok(count) => {
                    any_ok = true;
                    broad_count += 1;
                    tracing::debug!(term = %term, matches = count, "Skipping broad term");
                }
                Err(e) => {
                    // Stop probing the moment cancellation fires — otherwise a
                    // large term list churns one killed `rg` per remaining term
                    // and delays the cooperative cancel path.
                    if cancel.is_cancelled() {
                        return Err(ToolOutput::error("keyword_search cancelled"));
                    }
                    tracing::warn!(term = %term, error = %e, "Error checking term");
                }
            }
        }

        if usable_terms.is_empty() {
            if !any_ok {
                return Err(ToolOutput::error(
                    "keyword_search could not run any search term (is ripgrep installed and on PATH?)",
                ));
            }
            // Broad terms are skipped, not fatal (REQ-KWS-001): the "too broad"
            // error is only right when broadness is the *sole* reason nothing is
            // usable — i.e. no term also matched zero. With a zero-match term
            // present, the honest outcome is no matches, noting any skipped broad
            // terms so the caller knows coverage was reduced.
            if broad_count > 0 && !any_zero {
                return Err(ToolOutput::error(
                    "Each of those search terms yielded too many results. Try more specific terms.",
                ));
            }
            if broad_count > 0 {
                // Keep the canonical empty sentinel so the UI still renders this
                // as "no matches"; carry the skipped-term coverage warning as a
                // bracketed note the renderer surfaces.
                return Err(ToolOutput::success(format!(
                    "No matches found for the given search terms.{}",
                    incompleteness_notes(broad_count, 0, false)
                )));
            }
            return Err(ToolOutput::success(
                "No matches found for the given search terms.",
            ));
        }
        Ok((usable_terms, broad_count))
    }

    /// Combined context scan over `usable_terms`, capped at `MAX_COMBINED_RESULTS`.
    /// If the output overruns the cap, drop the lowest-priority term and retry:
    /// `search_terms` is ordered by importance (REQ-KWS-004), so peeling from the
    /// tail preserves the most important terms rather than letting a low-priority
    /// term crowd them out of the byte budget in filesystem-traversal order. Each
    /// retry is byte-bounded by `ripgrep_capped`, so this cannot regress to an
    /// unbounded rescan. Returns `(results, capped, dropped_term_count)`.
    async fn combined_scan(
        &self,
        search_root: &Path,
        mut usable_terms: Vec<String>,
        cancel: &CancellationToken,
    ) -> Result<(String, bool, usize), String> {
        let full_term_count = usable_terms.len();
        loop {
            let (out, capped) = self
                .ripgrep_capped(search_root, &usable_terms, cancel, MAX_COMBINED_RESULTS)
                .await?;
            if !capped || usable_terms.len() == 1 {
                return Ok((out, capped, full_term_count - usable_terms.len()));
            }
            usable_terms.pop(); // drop lowest-priority term, retry
        }
    }

    /// Select an LLM for filtering: the shared cheap/fast model (spans the
    /// supported providers, falls back to the default service), so the tool
    /// has no model list of its own to drift from the registry's.
    fn select_filter_llm(
        ctx: &ToolContext,
    ) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
        ctx.llm_selector().get_cheap_model()
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
            telemetry: Some(phoenix_core::domain::llm_types::LlmRequestTelemetry {
                conversation_id: ctx.conversation_id.clone(),
                root_conversation_id: ctx.root_conversation_id.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                retry_attempt: 1,
            }),
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

        // Floor: the filesystem root is never a valid search scope (it would
        // scan every mounted volume). Conversation cwd is already floored at
        // creation (REQ-PROJ-000); this is defense-in-depth for the resolution
        // path. An intentionally broad multi-repo root is *not* refused here —
        // the breadth probe and output cap below make it affordable to scan.
        if search_root.parent().is_none() {
            return ToolOutput::error(
                "keyword_search scope resolved to the filesystem root; run from within a project directory.",
            );
        }
        tracing::info!(root = %search_root.display(), terms = input.search_terms.len(), "keyword_search scope resolved");

        let (usable_terms, skipped_broad) = match self
            .probe_usable_terms(&search_root, &input.search_terms, &ctx.cancel)
            .await
        {
            Ok(t) => t,
            Err(terminal) => return terminal,
        };

        let (results, capped, dropped) = match self
            .combined_scan(&search_root, usable_terms, &ctx.cancel)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                if ctx.cancel.is_cancelled() {
                    return ToolOutput::error("keyword_search cancelled");
                }
                return ToolOutput::error(e);
            }
        };

        if results.is_empty() || results == "No matches found" {
            // Retained terms matched nothing. Keep the canonical empty sentinel
            // (so the UI renders it as empty) and carry any incompleteness as
            // bracketed notes — a broad term skipped or a matching lower-priority
            // term dropped means "no matches" is not the whole story.
            return ToolOutput::success(format!(
                "No matches found for the given search terms.{}",
                incompleteness_notes(skipped_broad, dropped, capped)
            ));
        }

        // Filter with LLM. Incompleteness notes are appended to the FINAL output
        // below, not folded into `results` here: folding them in would put them
        // in the filter prompt, which returns a plain file list and can drop
        // them — losing the signal that results are incomplete.
        let filtered = match self
            .filter_with_llm(&ctx, &input.query, &search_root, &results)
            .await
        {
            Ok(filtered) => filtered,
            Err(e) => {
                // If LLM fails, return raw results (size-truncated).
                tracing::warn!(error = %e, "LLM filtering failed, returning raw results");
                if results.len() > 8000 {
                    // Distinct from the byte-cap note in `incompleteness_notes`:
                    // this is a display-length cut of the *raw* fallback text,
                    // not the search-scope truncation.
                    format!(
                        "{}\n\n[raw output shortened for display]",
                        results.get(..8000).unwrap_or(&results)
                    )
                } else {
                    results
                }
            }
        };

        ToolOutput::success(format!(
            "{filtered}{}",
            incompleteness_notes(skipped_broad, dropped, capped)
        ))
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

    /// `rg` may be absent from a minimal CI image. Tests that assert on real
    /// ripgrep output skip themselves rather than fail when it isn't installed,
    /// matching how `rg_child_alive` degrades when `pgrep` is missing.
    fn rg_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Returns true if an `rg` process whose argv mentions `needle` is alive.
    /// We match on the unique search term, which `spawn_rg()` passes verbatim as
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

    /// REQ-BED-005: `ripgrep_capped()` kills and reaps its `rg` child on
    /// cancellation rather than blocking until the scan completes or leaving a
    /// zombie.
    ///
    /// Deterministic by construction: the token is cancelled BEFORE the call, so
    /// the biased `select!` always takes the cancel branch — the outcome cannot
    /// depend on whether `rg` outran a timer, which is what made the earlier
    /// "cancel once the child is observed live" version flake under heavy
    /// parallel test load (the scan could finish before it was observed). The
    /// child is still spawned and then killed+reaped via `Child::kill().await`,
    /// so the reap path is exercised.
    #[tokio::test]
    async fn ripgrep_reaps_child_on_cancel() {
        if !rg_available() {
            return; // no real child to spawn/reap without ripgrep
        }
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
            .ripgrep_capped(dir.path(), &terms, &cancel, MAX_COMBINED_RESULTS)
            .await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "ripgrep_capped with a cancelled token must return an error, got {result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "cancelled ripgrep_capped must return promptly, took {elapsed:?}"
        );

        // `Child::kill().await` reaps before the call returns; confirm no `rg`
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

    /// A broad term is rejected via early exit: the running total crosses
    /// `BROAD_TERM_MATCH_LIMIT` and `rg` is killed before walking the whole tree.
    #[tokio::test]
    async fn count_matches_early_exits_on_broad_term() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        for f in 0..50 {
            std::fs::write(
                dir.path().join(format!("f{f}.rs")),
                "controller\n".repeat(20),
            )
            .unwrap();
        }
        let tool = KeywordSearchTool;
        let start = std::time::Instant::now();
        let count = tool
            .count_matches(dir.path(), "controller", &CancellationToken::new())
            .await
            .expect("count");
        assert!(
            count > BROAD_TERM_MATCH_LIMIT,
            "broad term should exceed the limit, got {count}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "early exit should be prompt, took {:?}",
            start.elapsed()
        );
    }

    /// A narrow term completes and reports its exact match-line count.
    #[tokio::test]
    async fn count_matches_returns_exact_for_narrow_term() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            "uniqueneedle\nnope\nUniqueNeedle\n",
        )
        .unwrap();
        let tool = KeywordSearchTool;
        let count = tool
            .count_matches(dir.path(), "uniqueneedle", &CancellationToken::new())
            .await
            .expect("count");
        assert_eq!(count, 2, "case-insensitive match should count both lines");
    }

    /// A term concentrated in a single file that alone exceeds the limit must
    /// still be flagged broad — the per-file `--max-count` is `limit + 1`, so
    /// one hot file trips the early exit rather than reporting a small count.
    #[tokio::test]
    async fn count_matches_flags_single_hot_file_as_broad() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        // One file, many matches, no others — mimics a generated bundle/log.
        std::fs::write(
            dir.path().join("bundle.js"),
            "hotmatch\n".repeat(BROAD_TERM_MATCH_LIMIT * 3),
        )
        .unwrap();
        let tool = KeywordSearchTool;
        let count = tool
            .count_matches(dir.path(), "hotmatch", &CancellationToken::new())
            .await
            .expect("count");
        assert!(
            count > BROAD_TERM_MATCH_LIMIT,
            "a single file exceeding the limit must be flagged broad, got {count}"
        );
    }

    /// An absent term counts to zero — the caller uses this to exclude it from
    /// the combined scan rather than re-walking the tree to rediscover no hits.
    #[tokio::test]
    async fn count_matches_returns_zero_for_absent_term() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        std::fs::write(dir.path().join("a.rs"), "hello world\n").unwrap();
        let tool = KeywordSearchTool;
        let count = tool
            .count_matches(dir.path(), "absent_needle_xyz", &CancellationToken::new())
            .await
            .expect("count");
        assert_eq!(count, 0, "absent term must count to zero");
    }

    /// `incompleteness_notes` must produce one single-line, fully-bracketed
    /// `[...]` note per active cause (nothing when all are inactive), joined by
    /// blank lines — the exact shape the UI note extractor relies on.
    #[test]
    fn incompleteness_notes_are_bracketed_per_cause() {
        assert_eq!(incompleteness_notes(0, 0, false), "");

        let all = incompleteness_notes(2, 3, true);
        let notes: Vec<&str> = all.split("\n\n").filter(|s| !s.is_empty()).collect();
        assert_eq!(notes.len(), 3, "one note per active cause");
        for n in &notes {
            assert!(!n.contains('\n'), "note must be single-line: {n}");
            let inner = n
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .unwrap_or_else(|| panic!("not bracketed: {n}"));
            assert!(!inner.contains(']'), "stray ] in note: {n}");
        }
        assert!(notes[0].contains("skipped as too broad"));
        assert!(notes[1].contains("dropped"));
        assert!(notes[2].contains("truncated"));
    }

    /// A non-UTF-8 file path in `rg`'s output must not abort the probe: the
    /// trailing `:count` is parsed from raw bytes, ignoring the path encoding.
    #[cfg(unix)]
    #[tokio::test]
    async fn count_matches_tolerates_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt;
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        // Filename containing an invalid UTF-8 byte (0xFF).
        let mut name = std::ffi::OsString::from("bad_");
        name.push(std::ffi::OsStr::from_bytes(b"\xff"));
        name.push(".txt");
        // Some filesystems (macOS/APFS) reject non-UTF-8 names outright; if the
        // name can't be created here there's nothing to exercise, so skip.
        if std::fs::write(dir.path().join(&name), "needle\nneedle\n").is_err() {
            return;
        }
        let tool = KeywordSearchTool;
        let count = tool
            .count_matches(dir.path(), "needle", &CancellationToken::new())
            .await
            .expect("count");
        assert_eq!(count, 2, "a non-UTF-8 path must not abort the count");
    }

    /// Many matches on a *single line* (a minified bundle) must read as broad —
    /// guards against `--count` (matching-line count), which would report 1.
    #[tokio::test]
    async fn count_matches_flags_single_long_line_as_broad() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        // One line, no newline, term repeated well past the limit.
        let line = "needle ".repeat(BROAD_TERM_MATCH_LIMIT * 2);
        std::fs::write(dir.path().join("bundle.min.js"), line).unwrap();
        let tool = KeywordSearchTool;
        let count = tool
            .count_matches(dir.path(), "needle", &CancellationToken::new())
            .await
            .expect("count");
        assert!(
            count > BROAD_TERM_MATCH_LIMIT,
            "many matches on one line must be flagged broad, got {count}"
        );
    }

    /// An invalid regex term makes `rg` exit with an error status and no stdout;
    /// the probe must surface that as `Err` so the caller skips only that term
    /// rather than feeding the bad pattern into the combined scan.
    #[tokio::test]
    async fn count_matches_errors_on_invalid_regex() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        std::fs::write(dir.path().join("a.rs"), "hello world\n").unwrap();
        let tool = KeywordSearchTool;
        // Unbalanced parenthesis: not a valid regex, so `rg` exits with status 2.
        let res = tool
            .count_matches(dir.path(), "(", &CancellationToken::new())
            .await;
        assert!(
            res.is_err(),
            "invalid regex term must surface an error, got {res:?}"
        );
    }

    /// The combined scan is capped mid-stream: output never exceeds `max_bytes`
    /// and the truncation flag is set.
    #[tokio::test]
    async fn ripgrep_capped_truncates_at_cap() {
        if !rg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("create tempdir");
        let filler = "filler line\n".repeat(20);
        for f in 0..40 {
            let body = format!("{filler}findme here\n{filler}");
            std::fs::write(dir.path().join(format!("f{f}.txt")), body).unwrap();
        }
        let tool = KeywordSearchTool;
        let terms = vec!["findme".to_string()];
        let (out, truncated) = tool
            .ripgrep_capped(dir.path(), &terms, &CancellationToken::new(), 4096)
            .await
            .expect("scan");
        assert!(
            truncated,
            "output larger than the cap must be flagged truncated"
        );
        assert!(
            out.len() <= 4096,
            "capped output must not exceed max_bytes, got {}",
            out.len()
        );
    }
}
