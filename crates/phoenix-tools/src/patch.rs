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

pub(crate) mod executor;
pub(crate) mod interpreter;
pub(crate) mod matching;
pub(crate) mod planner;

#[cfg(test)]
mod proptests;

pub use planner::PatchPlanner;
// `types` (PatchInput, …) moved to phoenix-core. Alias back + glob re-export.
pub use phoenix_core::domain::patch_types::{self as types, *};

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use executor::{execute_effects, read_file_content};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_INPUT_SIZE: usize = 60 * 1024; // 60KB limit

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchScope {
    Unrestricted,
    TaskProposalDraft { tasks_dir_name: String },
}

/// Patch tool for file editing
///
/// This is the Tool implementation that wraps the pure `PatchPlanner`
/// with actual filesystem IO.
///
/// REQ-BASH-010: Stateless - uses `ToolContext` for `working_dir`
pub struct PatchTool {
    planner: Mutex<PatchPlanner>,
    scope: PatchScope,
}

impl PatchTool {
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            planner: Mutex::new(PatchPlanner::new()),
            scope: PatchScope::Unrestricted,
        }
    }

    /// Construct the Explore-mode patch tool for drafting task proposal files.
    /// This scope both enforces the task-directory write boundary and emits the
    /// post-success `propose_task` next-step reminder.
    pub fn for_task_proposal_drafts(tasks_dir_name: impl Into<String>) -> Self {
        Self {
            planner: Mutex::new(PatchPlanner::new()),
            scope: PatchScope::TaskProposalDraft {
                tasks_dir_name: tasks_dir_name.into(),
            },
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

    fn enforce_scope(
        &self,
        ctx: &ToolContext,
        raw_path: &str,
        resolved: &std::path::Path,
    ) -> Option<String> {
        let PatchScope::TaskProposalDraft { tasks_dir_name } = &self.scope else {
            return None;
        };

        if std::path::Path::new(raw_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Some(format!(
                "patch is restricted to '{tasks_dir_name}/' task proposal drafts in this mode; \
                 '..' components are not allowed (got '{raw_path}')."
            ));
        }

        let filename = std::path::Path::new(raw_path)
            .file_name()
            .and_then(|name| name.to_str());
        if filename.is_none_or(|name| phoenix_core::task_source::TaskSource::detect(name).is_none())
        {
            return Some(format!(
                "patch is restricted to markdown task proposal drafts under '{tasks_dir_name}/' \
                 in this mode (got '{raw_path}')."
            ));
        }

        let allowed_root = ctx.working_dir.join(tasks_dir_name);
        let canon_allowed = std::fs::canonicalize(&allowed_root).unwrap_or(allowed_root);
        let canon_resolved =
            std::fs::canonicalize(resolved).unwrap_or_else(|_| resolved.to_path_buf());
        if canon_resolved.starts_with(&canon_allowed) {
            None
        } else {
            Some(format!(
                "patch is restricted to '{tasks_dir_name}/' task proposal drafts in this mode; \
                 '{}' is outside the allowed directory.",
                resolved.display()
            ))
        }
    }

    fn proposal_next_step(&self, raw_path: &str) -> Option<String> {
        match &self.scope {
            PatchScope::Unrestricted => None,
            PatchScope::TaskProposalDraft { .. } => Some(format!(
                "\n<next_step>Call propose_task with task_file=\"{}\" if this is the task you want the user to approve.</next_step>",
                escape_xml_attribute(raw_path)
            )),
        }
    }
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl Default for PatchTool {
    fn default() -> Self {
        Self::unrestricted()
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
- All patches in a single call resolve against the original file content simultaneously, not sequentially. Repeating the same oldText across patches cannot disambiguate sites. For sequential edits where each step sees the prior result, use separate patch tool calls.

Size limit: each patch call must be less than 60 KB of input.".to_string()
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
                                "description": "Text to locate. Required for replace. Must be unique in file; when the same text appears at multiple sites, widen with surrounding context."
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

        if let Some(msg) = self.enforce_scope(&ctx, &patch_input.path, &path) {
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

        if let Some(next_step) = self.proposal_next_step(&patch_input.path) {
            output.push_str(&next_step);
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
    use crate::browser::BrowserSessionManager;
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
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
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

        assert!(result.is_success(), "Error: {}", result.output());
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello Rust");
    }

    #[tokio::test]
    async fn task_proposal_patch_rejects_paths_outside_task_dir() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::for_task_proposal_drafts("tasks");

        // Pre-create both the allowed and disallowed targets.
        fs::create_dir_all(dir.path().join("tasks")).unwrap();
        fs::write(dir.path().join("tasks/note.md"), "old\n").unwrap();
        fs::write(dir.path().join("source.md"), "# Source\n").unwrap();

        // Write outside the allowed prefix is rejected.
        let ctx = test_context(dir.path().to_path_buf());
        let blocked = tool
            .run(
                json!({
                    "path": "source.md",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "fn pwned() {}\n"
                    }]
                }),
                ctx,
            )
            .await;
        assert!(
            !blocked.is_success(),
            "expected rejection, got: {}",
            blocked.output()
        );
        assert!(
            blocked
                .output()
                .contains("restricted to 'tasks/' task proposal drafts"),
            "missing task proposal scope hint: {}",
            blocked.output()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("source.md")).unwrap(),
            "# Source\n",
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
            !traversal.is_success(),
            "expected rejection of '..' traversal, got: {}",
            traversal.output()
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
        assert!(
            allowed.is_success(),
            "expected success, got: {}",
            allowed.output()
        );
        assert!(
            allowed
                .output()
                .contains("<next_step>Call propose_task with task_file=\"tasks/note.md\""),
            "missing proposal next-step reminder: {}",
            allowed.output()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("tasks/note.md")).unwrap(),
            "new\n"
        );
    }
    #[tokio::test]
    async fn task_proposal_patch_rejects_non_markdown_files() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::for_task_proposal_drafts("tasks");
        fs::create_dir_all(dir.path().join("tasks")).unwrap();

        let ctx = test_context(dir.path().to_path_buf());
        let result = tool
            .run(
                json!({
                    "path": "tasks/note.txt",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "not markdown\n"
                    }]
                }),
                ctx,
            )
            .await;

        assert!(
            !result.is_success(),
            "expected rejection, got: {}",
            result.output()
        );
        assert!(
            result
                .output()
                .contains("restricted to markdown task proposal drafts"),
            "missing markdown-task-source hint: {}",
            result.output()
        );
        assert!(
            !dir.path().join("tasks/note.txt").exists(),
            "non-markdown task proposal draft must not be created"
        );
        assert!(
            !result.output().contains("propose_task"),
            "failed patches must not include the success next-step reminder: {}",
            result.output()
        );
    }

    #[tokio::test]
    async fn unrestricted_patch_success_does_not_include_proposal_reminder() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::default();
        let ctx = test_context(dir.path().to_path_buf());

        let result = tool
            .run(
                json!({
                    "path": "notes.md",
                    "patches": [{
                        "operation": "overwrite",
                        "newText": "# Notes\n"
                    }]
                }),
                ctx,
            )
            .await;

        assert!(
            result.is_success(),
            "expected success, got: {}",
            result.output()
        );
        assert_eq!(result.output(), "<patches_applied>all</patches_applied>");
    }

    #[tokio::test]
    async fn duplicate_replace_error_includes_locations_and_snippets() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::default();
        let ctx = test_context(dir.path().to_path_buf());

        let test_file = dir.path().join("test.txt");
        fs::write(
            &test_file,
            "first block\nTARGET\nafter first\nsecond block\nTARGET\nafter second\n",
        )
        .unwrap();

        let result = tool
            .run(
                json!({
                    "path": "test.txt",
                    "patches": [{
                        "operation": "replace",
                        "oldText": "TARGET",
                        "newText": "replacement"
                    }]
                }),
                ctx,
            )
            .await;

        assert!(
            !result.is_success(),
            "expected error, got: {}",
            result.output()
        );
        let output = result.output();
        assert!(output.contains("oldText appears 2 times"), "{output}");
        assert!(output.contains("line 2"), "{output}");
        assert!(
            output.contains("first block\n  TARGET\n  after first"),
            "{output}"
        );
        assert!(output.contains("line 5"), "{output}");
        assert!(
            output.contains("second block\n  TARGET\n  after second"),
            "{output}"
        );
        assert!(
            output.contains("Widen oldText with surrounding context"),
            "{output}"
        );
        assert_eq!(
            fs::read_to_string(&test_file).unwrap(),
            "first block\nTARGET\nafter first\nsecond block\nTARGET\nafter second\n"
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

        assert!(result.is_success());
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
