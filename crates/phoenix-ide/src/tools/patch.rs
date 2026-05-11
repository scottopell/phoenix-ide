//! Patch tool - precise file editing
//!
//! This module implements a patch tool with a pure core that can be
//! property-tested. The architecture follows an effect pattern:
//!
//! 1. `PatchPlanner` (pure) - Plans patches and produces effects
//! 2. `executor` - Executes effects against real filesystem
//! 3. `interpreter` - Interprets effects in memory for testing
//!
//! # Example
//!
//! ```ignore
//! let mut planner = PatchPlanner::new();
//! let plan = planner.plan(path, Some(content), &patches)?;
//! executor::execute_effects(&plan.effects)?;
//! ```

pub mod executor;
pub mod interpreter;
pub mod matching;
pub mod planner;
pub mod types;

#[cfg(test)]
mod proptests;

pub use planner::PatchPlanner;
pub use types::*;

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use executor::{execute_effects, read_file_content};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_INPUT_SIZE: usize = 60 * 1024; // 60KB limit

/// Patch tool for file editing
///
/// This is the Tool implementation that wraps the pure `PatchPlanner`
/// with actual filesystem IO.
///
/// REQ-BASH-010: Stateless - uses `ToolContext` for `working_dir`
pub struct PatchTool {
    planner: Mutex<PatchPlanner>,
    /// When set, edits are rejected unless the resolved path is inside this
    /// directory (relative to the conversation cwd). Used by Explore mode
    /// to permit drafting task files under `tasks/` without unlocking the
    /// rest of the worktree.
    allowed_path_prefix: Option<&'static str>,
}

impl PatchTool {
    /// Construct a `PatchTool` that only accepts paths inside the named
    /// relative directory (e.g. `"tasks"`). Paths outside that directory
    /// are rejected at runtime.
    pub fn restricted_to(prefix: &'static str) -> Self {
        Self {
            planner: Mutex::new(PatchPlanner::new()),
            allowed_path_prefix: Some(prefix),
        }
    }

    fn resolve_path(ctx: &ToolContext, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            ctx.working_dir.join(p)
        }
    }

    /// Reject paths that fall outside the configured allowlist. Returns
    /// `None` if no restriction is configured.
    ///
    /// `raw_path` is the unresolved path string from the LLM. We reject `..`
    /// components on it directly: when the patch target does not yet exist,
    /// `canonicalize(resolved)` returns `Err` and the function falls back to
    /// the lexical path — which would let `tasks/../src/foo.rs` pass the
    /// prefix check on a brand-new file.
    fn enforce_allowlist(
        &self,
        ctx: &ToolContext,
        raw_path: &str,
        resolved: &std::path::Path,
    ) -> Option<String> {
        let prefix = self.allowed_path_prefix?;

        if std::path::Path::new(raw_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Some(format!(
                "patch is restricted to '{prefix}/' in this mode; \
                 '..' components are not allowed (got '{raw_path}')."
            ));
        }

        let allowed_root = ctx.working_dir.join(prefix);
        let canon_allowed = std::fs::canonicalize(&allowed_root).unwrap_or(allowed_root);
        let canon_resolved =
            std::fs::canonicalize(resolved).unwrap_or_else(|_| resolved.to_path_buf());
        if canon_resolved.starts_with(&canon_allowed) {
            None
        } else {
            Some(format!(
                "patch is restricted to '{prefix}/' in this mode; \
                 '{}' is outside the allowed directory.",
                resolved.display()
            ))
        }
    }
}

impl Default for PatchTool {
    fn default() -> Self {
        Self {
            planner: Mutex::new(PatchPlanner::new()),
            allowed_path_prefix: None,
        }
    }
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &'static str {
        "patch"
    }

    fn description(&self) -> String {
        r"File modification tool for precise text edits.

Operations:
- replace: Substitute unique text with new content
- append_eof: Append new text at the end of the file
- prepend_bof: Insert new text at the beginning of the file
- overwrite: Replace the entire file with new content (automatically creates the file)

Clipboard:
- toClipboard: Store oldText to a named clipboard before the operation
- fromClipboard: Use clipboard content as newText (ignores provided newText)
- Clipboards persist across patch calls
- Always use clipboards when moving/copying code (within or across files), even when the moved/copied code will also have edits.
  This prevents transcription errors and distinguishes intentional changes from unintentional changes.

Indentation adjustment:
- reindent applies to whatever text is being inserted
- First strips the specified prefix from each line, then adds the new prefix
- Useful when moving code from one indentation to another

Recipes:
- cut: replace with empty newText and toClipboard
- copy: replace with toClipboard and fromClipboard using the same clipboard name
- paste: replace with fromClipboard
- in-place indentation change: same as copy, but add indentation adjustment

Usage notes:
- All inputs are interpreted literally (no automatic newline or whitespace handling)
- For replace operations, oldText must appear EXACTLY ONCE in the file

IMPORTANT: Each patch call must be less than 60k tokens total. For large file
changes, break them into multiple smaller patch operations rather than one
large overwrite. Prefer incremental replace operations over full file overwrites.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "patches"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to patch"
                },
                "patches": {
                    "type": "array",
                    "description": "List of patch requests to apply",
                    "items": {
                        "type": "object",
                        "required": ["operation"],
                        "properties": {
                            "operation": {
                                "type": "string",
                                "enum": ["replace", "append_eof", "prepend_bof", "overwrite"],
                                "description": "Type of operation to perform"
                            },
                            "oldText": {
                                "type": "string",
                                "description": "Text to locate (must be unique in file, required for replace)"
                            },
                            "newText": {
                                "type": "string",
                                "description": "The new text to use (empty for deletions, leave empty if fromClipboard is set)"
                            },
                            "toClipboard": {
                                "type": "string",
                                "description": "Save oldText to this named clipboard before the operation"
                            },
                            "fromClipboard": {
                                "type": "string",
                                "description": "Use content from this clipboard as newText (overrides newText field)"
                            },
                            "reindent": {
                                "type": "object",
                                "description": "Modify indentation of inserted text before insertion",
                                "properties": {
                                    "strip": {
                                        "type": "string",
                                        "description": "Remove this prefix from each non-empty line"
                                    },
                                    "add": {
                                        "type": "string",
                                        "description": "Add this prefix to each non-empty line after stripping"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        // Check input size
        let input_str = input.to_string();
        if input_str.len() > MAX_INPUT_SIZE {
            return ToolOutput::error(format!(
                "Input too large ({} bytes). Maximum is {} bytes. Break into smaller patches.",
                input_str.len(),
                MAX_INPUT_SIZE
            ));
        }

        // Parse input
        let patch_input: PatchInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        if patch_input.patches.is_empty() {
            return ToolOutput::error("No patches provided");
        }

        // Resolve path
        let path = Self::resolve_path(&ctx, &patch_input.path);

        if let Some(msg) = self.enforce_allowlist(&ctx, &patch_input.path, &path) {
            return ToolOutput::error(msg);
        }

        // Read current content
        let current_content = match read_file_content(&path) {
            Ok(content) => content,
            Err(e) => return ToolOutput::error(format!("Failed to read file: {e}")),
        };

        // Plan patches
        let plan = {
            let mut planner = self.planner.lock().unwrap();
            match planner.plan(&path, current_content.as_deref(), &patch_input.patches) {
                Ok(plan) => plan,
                Err(e) => return ToolOutput::error(e.to_string()),
            }
        };

        // Execute effects
        if let Err(e) = execute_effects(&plan.effects) {
            return ToolOutput::error(format!("Failed to write file: {e}"));
        }

        // Build output
        let mut output = "<patches_applied>all</patches_applied>".to_string();
        if plan.autogenerated_warning {
            output.push_str(
                "\n<warning>This file appears to be auto-generated. Edits may be overwritten.</warning>",
            );
        }

        let display_data = json!({
            "path": path.display().to_string(),
            "diff": plan.diff
        });

        ToolOutput::success(output).with_display(display_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browser::BrowserSessionManager;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            Arc::new(crate::tools::TmuxRegistry::new()),
            None,
        )
    }

    #[tokio::test]
    async fn test_replace_operation() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::default();
        let ctx = test_context(dir.path().to_path_buf());

        // Create test file
        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "Hello World").unwrap();

        let result = tool
            .run(
                json!({
                    "path": "test.txt",
                    "patches": [{
                        "operation": "replace",
                        "oldText": "World",
                        "newText": "Rust"
                    }]
                }),
                ctx,
            )
            .await;

        assert!(result.success, "Error: {}", result.output);
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello Rust");
    }

    #[tokio::test]
    async fn restricted_patch_rejects_paths_outside_allowlist() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::restricted_to("tasks");

        // Pre-create both the allowed and disallowed targets.
        fs::create_dir_all(dir.path().join("tasks")).unwrap();
        fs::write(dir.path().join("tasks/note.md"), "old\n").unwrap();
        fs::write(dir.path().join("source.rs"), "fn main() {}\n").unwrap();

        // Write outside the allowed prefix is rejected.
        let ctx = test_context(dir.path().to_path_buf());
        let blocked = tool
            .run(
                json!({
                    "path": "source.rs",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "fn pwned() {}\n"
                    }]
                }),
                ctx,
            )
            .await;
        assert!(
            !blocked.success,
            "expected rejection, got: {}",
            blocked.output
        );
        assert!(
            blocked.output.contains("restricted to 'tasks/'"),
            "missing allowlist hint: {}",
            blocked.output
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("source.rs")).unwrap(),
            "fn main() {}\n",
            "source must be untouched"
        );

        // `..` traversal to a *new* file outside the prefix is rejected
        // (canonicalize() returns Err for missing targets and would
        // otherwise fall back to the lexical path that starts_with "tasks").
        let ctx = test_context(dir.path().to_path_buf());
        let traversal = tool
            .run(
                json!({
                    "path": "tasks/../escape.rs",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "fn pwned() {}\n"
                    }]
                }),
                ctx,
            )
            .await;
        assert!(
            !traversal.success,
            "expected rejection of '..' traversal, got: {}",
            traversal.output
        );
        assert!(
            !dir.path().join("escape.rs").exists(),
            "escape file must not have been created"
        );

        // Write inside the allowed prefix succeeds.
        let ctx = test_context(dir.path().to_path_buf());
        let allowed = tool
            .run(
                json!({
                    "path": "tasks/note.md",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "new\n"
                    }]
                }),
                ctx,
            )
            .await;
        assert!(allowed.success, "expected success, got: {}", allowed.output);
        assert_eq!(
            fs::read_to_string(dir.path().join("tasks/note.md")).unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn test_overwrite_creates_file() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::default();
        let ctx = test_context(dir.path().to_path_buf());

        let result = tool
            .run(
                json!({
                    "path": "new_file.txt",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "New content"
                    }]
                }),
                ctx,
            )
            .await;

        assert!(result.success);
        let test_file = dir.path().join("new_file.txt");
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "New content");
    }

    #[tokio::test]
    async fn test_clipboard_operations() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::default();

        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "AAA BBB CCC").unwrap();

        // Cut BBB to clipboard
        let ctx = test_context(dir.path().to_path_buf());
        tool.run(
            json!({
                "path": "test.txt",
                "patches": [{
                    "operation": "replace",
                    "oldText": "BBB",
                    "newText": "",
                    "toClipboard": "clip1"
                }]
            }),
            ctx,
        )
        .await;

        assert_eq!(fs::read_to_string(&test_file).unwrap(), "AAA  CCC");

        // Paste from clipboard
        let ctx = test_context(dir.path().to_path_buf());
        tool.run(
            json!({
                "path": "test.txt",
                "patches": [{
                    "operation": "replace",
                    "oldText": "CCC",
                    "fromClipboard": "clip1"
                }]
            }),
            ctx,
        )
        .await;

        assert_eq!(fs::read_to_string(&test_file).unwrap(), "AAA  BBB");
    }
}
