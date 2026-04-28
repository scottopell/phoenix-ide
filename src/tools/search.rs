//! Search tool - regex/text search across files
//!
//! REQ-PROJ-002, REQ-PROJ-013: Explore mode search without bash

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_RESULTS: usize = 50;
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ENTRIES_VISITED: usize = 100_000;

/// Directories to prune from the walk (applied via `WalkBuilder::filter_entry`,
/// so descent into these subtrees is skipped entirely).
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".next",
    "__pycache__",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    ".tox",
];

/// Search for text patterns across files.
pub struct SearchTool;

#[derive(Debug, Deserialize)]
struct SearchInput {
    pattern: String,
    path: Option<String>,
    include: Option<String>,
    max_results: Option<usize>,
}

/// Resolve a search path relative to `working_dir`. Absolute paths are used
/// as-is; relative paths are joined to `working_dir`. No containment check —
/// see the note on `read_file::resolve_and_validate` for rationale.
fn resolve_and_validate(path_str: &str, working_dir: &Path) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path_str);
    let resolved = if raw.is_absolute() {
        raw
    } else {
        working_dir.join(&raw)
    };

    resolved
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path '{}': {e}", resolved.display()))
}

/// Check if a file appears to be binary by examining the first 8KB for null bytes.
fn is_binary(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::with_capacity(8192, file);
    let Ok(buf) = reader.fill_buf() else {
        return false;
    };
    buf.contains(&0)
}

/// Check if a glob pattern matches a filename.
fn matches_glob(filename: &str, pattern: &str) -> bool {
    // Simple glob matching: support *.ext and *pattern* forms
    if let Some(ext) = pattern.strip_prefix("*.") {
        filename.ends_with(&format!(".{ext}"))
    } else if pattern.starts_with('*') && pattern.ends_with('*') {
        let inner = pattern.get(1..pattern.len() - 1).unwrap_or("");
        filename.contains(inner)
    } else {
        filename == pattern
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &'static str {
        "search"
    }

    fn description(&self) -> String {
        "Search for a text pattern across files. Returns matching lines with file paths and line numbers."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (supports regex)"
                },
                "path": {
                    "type": "string",
                    "description": "Subdirectory or file to search in (relative to working directory). Default: \".\""
                },
                "include": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., \"*.rs\", \"*.ts\")"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return. Default: 50"
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: SearchInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let re = match Regex::new(&input.pattern) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Invalid regex pattern: {e}")),
        };

        let search_path = match &input.path {
            Some(p) => match resolve_and_validate(p, &ctx.working_dir) {
                Ok(resolved) => resolved,
                Err(e) => return ToolOutput::error(e),
            },
            None => match ctx.working_dir.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    return ToolOutput::error(format!("Cannot resolve working directory: {e}"))
                }
            },
        };

        let canonical_wd = match ctx.working_dir.canonicalize() {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(format!("Cannot resolve working directory: {e}")),
        };

        let max_results = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        let include = input.include;
        let pattern_str = input.pattern;
        let cancel = ctx.cancel.clone();
        let conv_id = ctx.conversation_id.clone();

        match tokio::task::spawn_blocking(move || {
            run_search(SearchArgs {
                re,
                search_path,
                canonical_wd,
                include,
                max_results,
                cancel,
                conv_id,
                pattern_str,
            })
        })
        .await
        {
            Ok(out) => out,
            Err(e) => ToolOutput::error(format!("Search task failed: {e}")),
        }
    }
}

struct SearchArgs {
    re: Regex,
    search_path: PathBuf,
    canonical_wd: PathBuf,
    include: Option<String>,
    max_results: usize,
    cancel: CancellationToken,
    conv_id: String,
    pattern_str: String,
}

fn run_search(args: SearchArgs) -> ToolOutput {
    let SearchArgs {
        re,
        search_path,
        canonical_wd,
        include,
        max_results,
        cancel,
        conv_id,
        pattern_str,
    } = args;

    let walker = WalkBuilder::new(&search_path)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if SKIP_DIRS.contains(&name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    let mut results = Vec::new();
    let mut entries_visited: usize = 0;
    let mut walk_truncated = false;

    for entry in walker {
        if cancel.is_cancelled() {
            return ToolOutput::error("Cancelled");
        }

        entries_visited += 1;
        if entries_visited > MAX_ENTRIES_VISITED {
            walk_truncated = true;
            tracing::warn!(
                conv_id = %conv_id,
                cap = MAX_ENTRIES_VISITED,
                path = %search_path.display(),
                pattern = %pattern_str,
                "search walk truncated at entry cap; agent should narrow path or include"
            );
            break;
        }

        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        if let Some(ref include_pattern) = include {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !matches_glob(name, include_pattern) {
                continue;
            }
        }

        if scan_file(path, &re, &canonical_wd, &mut results, max_results) {
            break;
        }
    }

    format_output(&results, max_results, walk_truncated)
}

/// Scan one file for `re`, appending `display_path:lineno: line` to `results`.
/// Returns `true` if `max_results` was hit and the outer walk should stop.
fn scan_file(
    path: &Path,
    re: &Regex,
    canonical_wd: &Path,
    results: &mut Vec<String>,
    max_results: usize,
) -> bool {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_FILE_BYTES {
            return false;
        }
    }
    if is_binary(path) {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let reader = std::io::BufReader::new(file.take(MAX_FILE_BYTES));
    let display_path = path
        .strip_prefix(canonical_wd)
        .unwrap_or(path)
        .display()
        .to_string();

    for (line_num, line_res) in reader.lines().enumerate() {
        let Ok(line) = line_res else {
            break;
        };
        if re.is_match(&line) {
            results.push(format!("{}:{}: {}", display_path, line_num + 1, line));
            if results.len() >= max_results {
                return true;
            }
        }
    }
    false
}

fn format_output(results: &[String], max_results: usize, walk_truncated: bool) -> ToolOutput {
    if results.is_empty() && !walk_truncated {
        return ToolOutput::success("No matches found.");
    }

    let mut output = results.join("\n");
    if results.len() >= max_results {
        let _ = write!(
            output,
            "\n\n[Results limited to {max_results} matches. Use a more specific pattern or path to narrow results.]"
        );
    }
    if walk_truncated {
        let _ = write!(
            output,
            "\n\n[Walk truncated at {MAX_ENTRIES_VISITED} entries. Narrow `path` or `include`.]"
        );
    }

    ToolOutput::success(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::BrowserSessionManager;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
        )
    }

    #[tokio::test]
    async fn test_search_basic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("hello.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("other.txt"), "nothing here\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "println"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("hello.rs"));
        assert!(result.output.contains("println"));
    }

    #[tokio::test]
    async fn test_search_with_include() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn hello()\n").unwrap();
        std::fs::write(dir.path().join("code.txt"), "fn hello()\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "hello", "include": "*.rs"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("code.rs"));
        assert!(!result.output.contains("code.txt"));
    }

    #[tokio::test]
    async fn test_search_max_results() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (1..=100).fold(String::new(), |mut s, i| {
            use std::fmt::Write;
            let _ = writeln!(s, "match line {i}");
            s
        });
        std::fs::write(dir.path().join("big.txt"), &content).unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "match", "max_results": 5}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("Results limited to 5"));
    }

    #[tokio::test]
    async fn test_search_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "zzzznotfound"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_search_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "[invalid"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_search_allows_paths_outside_working_dir() {
        // Consistency with bash/read_file: search can walk any directory the
        // process has permission to read. Guards against re-introducing the
        // old working-directory containment check.
        let outer = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir_in(outer.path()).unwrap();
        let outside_dir = tempfile::tempdir_in(outer.path()).unwrap();
        std::fs::write(outside_dir.path().join("hit.txt"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme", "path": outside_dir.path().to_str().unwrap()}),
                test_context(inner.path().to_path_buf()),
            )
            .await;
        assert!(
            result.success,
            "search should resolve paths outside the working dir: {}",
            result.output
        );
        assert!(result.output.contains("findme"));
    }

    #[test]
    fn test_matches_glob() {
        assert!(matches_glob("test.rs", "*.rs"));
        assert!(!matches_glob("test.ts", "*.rs"));
        assert!(matches_glob("test.tsx", "*.tsx"));
    }

    #[tokio::test]
    async fn test_search_prunes_skip_dirs() {
        let dir = tempfile::tempdir().unwrap();
        for skipped in &["node_modules", "target", "vendor", "dist", ".git"] {
            let sub = dir.path().join(skipped);
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("inside.txt"), "findme\n").unwrap();
        }
        std::fs::write(dir.path().join("real.rs"), "findme\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(
            result.output.contains("real.rs"),
            "should find real source: {}",
            result.output
        );
        for skipped in &["node_modules", "target", "vendor", "dist", ".git"] {
            assert!(
                !result.output.contains(skipped),
                "should not descend into {skipped}: {}",
                result.output
            );
        }
    }

    #[tokio::test]
    async fn test_search_skips_files_over_size_cap() {
        let dir = tempfile::tempdir().unwrap();

        let big_path = dir.path().join("big.log");
        let cap = usize::try_from(MAX_FILE_BYTES).unwrap();
        let mut big = vec![b'x'; cap + 1024];
        big.extend_from_slice(b"\nfindme matchhere\n");
        std::fs::write(&big_path, &big).unwrap();

        std::fs::write(dir.path().join("small.txt"), "findme matchhere\n").unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("small.txt"), "{}", result.output);
        assert!(
            !result.output.contains("big.log"),
            "oversize file should be skipped: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_search_streams_lines_without_full_load() {
        // A file just under the size cap should still be searched. This exercises
        // the streamed-read path (no read_to_string of the full content).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("medium.txt");
        let filler_line = "x".repeat(1024);
        let cap = usize::try_from(MAX_FILE_BYTES).unwrap();
        let mut content = String::with_capacity(cap);
        while content.len() + filler_line.len() + 1 < cap - 256 {
            content.push_str(&filler_line);
            content.push('\n');
        }
        content.push_str("findme matchhere\n");
        std::fs::write(&path, &content).unwrap();

        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "findme"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("medium.txt"), "{}", result.output);
    }
}
