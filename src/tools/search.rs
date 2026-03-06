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
use std::io::BufRead;
use std::path::PathBuf;

const DEFAULT_MAX_RESULTS: usize = 50;

/// Directories to always skip (in addition to gitignore rules).
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".next", "__pycache__"];

/// Search for text patterns across files.
pub struct SearchTool;

#[derive(Debug, Deserialize)]
struct SearchInput {
    pattern: String,
    path: Option<String>,
    include: Option<String>,
    max_results: Option<usize>,
}

/// Resolve and validate a search path within `working_dir`.
fn resolve_and_validate(path_str: &str, working_dir: &std::path::Path) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path_str);
    let resolved = if raw.is_absolute() {
        raw
    } else {
        working_dir.join(&raw)
    };

    let canonical = resolved
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path '{}': {e}", resolved.display()))?;
    let canonical_wd = working_dir
        .canonicalize()
        .map_err(|e| format!("Cannot resolve working directory: {e}"))?;

    if !canonical.starts_with(&canonical_wd) {
        return Err(format!(
            "Path '{path_str}' is outside the working directory"
        ));
    }

    Ok(canonical)
}

/// Check if a file appears to be binary by examining the first 8KB for null bytes.
fn is_binary(path: &std::path::Path) -> bool {
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
        let inner = &pattern[1..pattern.len() - 1];
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

        let max_results = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS);

        // Use the `ignore` crate for gitignore-aware walking
        let walker = WalkBuilder::new(&search_path)
            .hidden(true) // respect hidden files (skip by default)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        let canonical_wd = match ctx.working_dir.canonicalize() {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(format!("Cannot resolve working directory: {e}")),
        };

        let mut results = Vec::new();

        for entry in walker {
            if ctx.cancel.is_cancelled() {
                return ToolOutput::error("Cancelled");
            }

            let Ok(entry) = entry else {
                continue;
            };

            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                // Check if this directory should be skipped
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if SKIP_DIRS.contains(&name) {
                        continue;
                    }
                }
                continue;
            }

            // Apply include filter
            if let Some(ref include_pattern) = input.include {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if !matches_glob(name, include_pattern) {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            // Skip binary files
            if is_binary(path) {
                continue;
            }

            // Read and search the file
            let Ok(content) = std::fs::read_to_string(path) else {
                continue; // skip unreadable files
            };

            // Compute display path relative to working directory
            let display_path = path
                .strip_prefix(&canonical_wd)
                .unwrap_or(path)
                .display()
                .to_string();

            for (line_num, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(format!("{}:{}: {}", display_path, line_num + 1, line));
                    if results.len() >= max_results {
                        break;
                    }
                }
            }

            if results.len() >= max_results {
                break;
            }
        }

        if results.is_empty() {
            return ToolOutput::success("No matches found.");
        }

        let total = results.len();
        let mut output = results.join("\n");
        if total >= max_results {
            let _ = write!(
                output,
                "\n\n[Results limited to {max_results} matches. Use a more specific pattern or path to narrow results.]"
            );
        }

        ToolOutput::success(output)
    }
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
        let content: String = (1..=100).map(|i| format!("match line {i}\n")).collect();
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
    async fn test_search_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SearchTool;
        let result = tool
            .run(
                json!({"pattern": "test", "path": "../../etc"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(
            result.output.contains("outside the working directory")
                || result.output.contains("Cannot resolve")
        );
    }

    #[test]
    fn test_matches_glob() {
        assert!(matches_glob("test.rs", "*.rs"));
        assert!(!matches_glob("test.ts", "*.rs"));
        assert!(matches_glob("test.tsx", "*.tsx"));
    }
}
