//! `ReadFile` tool - read file contents with line numbers
//!
//! REQ-PROJ-002, REQ-PROJ-013: Explore mode file reading without bash

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::PathBuf;

const DEFAULT_LIMIT: usize = 2000;

/// Read a file's contents with line numbers.
pub struct ReadFileTool;

#[derive(Debug, Deserialize)]
struct ReadFileInput {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Resolve a path relative to `working_dir` and verify it stays within bounds.
fn resolve_and_validate(path: &str, working_dir: &std::path::Path) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path);
    let resolved = if raw.is_absolute() {
        raw
    } else {
        working_dir.join(&raw)
    };

    // Canonicalize both to resolve symlinks and `..` components
    let canonical = resolved
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path '{}': {e}", resolved.display()))?;
    let canonical_wd = working_dir
        .canonicalize()
        .map_err(|e| format!("Cannot resolve working directory: {e}"))?;

    if !canonical.starts_with(&canonical_wd) {
        return Err(format!("Path '{path}' is outside the working directory"));
    }

    Ok(canonical)
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> String {
        "Read a file's contents. Returns numbered lines. Use offset and limit for large files."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (absolute or relative to working directory)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start from (1-based). Default: 1"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return. Default: 2000"
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ReadFileInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let resolved = match resolve_and_validate(&input.path, &ctx.working_dir) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };

        // Read file contents
        let contents = match tokio::fs::read(&resolved).await {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolOutput::error(format!("Failed to read '{}': {e}", resolved.display()))
            }
        };

        // Check for binary content (presence of null bytes in first 8KB)
        let check_len = contents.len().min(8192);
        if contents[..check_len].contains(&0) {
            return ToolOutput::error(format!("'{}' appears to be a binary file", input.path));
        }

        let text = String::from_utf8(contents)
            .map_err(|_| format!("'{}' is not valid UTF-8 text", input.path));
        let text = match text {
            Ok(s) => s,
            Err(msg) => return ToolOutput::error(msg),
        };

        let offset = input.offset.unwrap_or(1).max(1); // 1-based, minimum 1
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);

        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();

        // Convert 1-based offset to 0-based index
        let start_idx = (offset - 1).min(total_lines);
        let end_idx = (start_idx + limit).min(total_lines);

        if ctx.cancel.is_cancelled() {
            return ToolOutput::error("Cancelled");
        }

        let mut output = String::new();
        for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
            let line_num = start_idx + i + 1; // 1-based line number
            let _ = writeln!(output, "{line_num:>6}\t{line}");
        }

        let remaining = total_lines.saturating_sub(end_idx);
        if remaining > 0 {
            let _ = write!(
                output,
                "\n[{remaining} more lines not shown (total: {total_lines} lines)]"
            );
        }

        if output.is_empty() {
            output = "(empty file)".to_string();
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
            crate::terminal::ActiveTerminals::new(),
        )
    }

    #[tokio::test]
    async fn test_read_file_basic() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "line1\nline2\nline3\n").unwrap();

        let tool = ReadFileTool;
        let result = tool
            .run(
                json!({"path": "test.txt"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("line1"));
        assert!(result.output.contains("line2"));
        assert!(result.output.contains("line3"));
    }

    #[tokio::test]
    async fn test_read_file_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "a\nb\nc\nd\ne\n").unwrap();

        let tool = ReadFileTool;
        let result = tool
            .run(
                json!({"path": "test.txt", "offset": 2, "limit": 2}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains('b'));
        assert!(result.output.contains('c'));
        assert!(!result.output.contains("\ta\n"));
    }

    #[tokio::test]
    async fn test_read_file_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool;
        let result = tool
            .run(
                json!({"path": "../../etc/passwd"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(
            result.output.contains("outside the working directory")
                || result.output.contains("Cannot resolve")
        );
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool;
        let result = tool
            .run(
                json!({"path": "nonexistent.txt"}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_read_file_truncation_note() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.txt");
        let content: String = (1..=100).fold(String::new(), |mut s, i| {
            use std::fmt::Write;
            let _ = writeln!(s, "line {i}");
            s
        });
        std::fs::write(&file_path, &content).unwrap();

        let tool = ReadFileTool;
        let result = tool
            .run(
                json!({"path": "big.txt", "limit": 10}),
                test_context(dir.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("more lines not shown"));
    }
}
