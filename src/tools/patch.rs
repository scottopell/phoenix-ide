//! Patch tool - precise file editing
//!
//! REQ-PATCH-001: File Operations
//! REQ-PATCH-002: Multiple Patches Per Call
//! REQ-PATCH-003: Clipboard Operations
//! REQ-PATCH-004: Indentation Adjustment
//! REQ-PATCH-005: Fuzzy Matching Recovery
//! REQ-PATCH-006: Tool Schema
//! REQ-PATCH-007: Output and Display
//! REQ-PATCH-008: Size Limits

use super::{Tool, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_INPUT_SIZE: usize = 60 * 1024; // 60KB limit

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Replace,
    AppendEof,
    PrependBof,
    Overwrite,
}

#[derive(Debug, Clone, Deserialize)]
struct Reindent {
    strip: Option<String>,
    add: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchRequest {
    operation: Operation,
    old_text: Option<String>,
    new_text: Option<String>,
    to_clipboard: Option<String>,
    from_clipboard: Option<String>,
    reindent: Option<Reindent>,
}

#[derive(Debug, Deserialize)]
struct PatchInput {
    path: String,
    patches: Vec<PatchRequest>,
}

/// Edit specification after matching
struct EditSpec {
    offset: usize,
    length: usize,
}

/// Edit to apply
struct Edit {
    offset: usize,
    length: usize,
    replacement: String,
}

/// Patch tool for file editing
pub struct PatchTool {
    working_dir: PathBuf,
    clipboards: Mutex<HashMap<String, String>>,
}

impl PatchTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            clipboards: Mutex::new(HashMap::new()),
        }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.working_dir.join(p)
        }
    }

    fn apply_patches(&self, input: &PatchInput) -> Result<(String, String, String), String> {
        let path = self.resolve_path(&input.path);
        
        // Read original content
        let original = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File doesn't exist - only certain operations allowed
                for patch in &input.patches {
                    match patch.operation {
                        Operation::Replace => {
                            return Err("Cannot use replace operation on non-existent file".to_string());
                        }
                        _ => {}
                    }
                }
                String::new()
            }
            Err(e) => return Err(format!("Failed to read file: {}", e)),
        };

        // Build edit list
        let mut edits: Vec<Edit> = Vec::new();
        let mut clipboards = self.clipboards.lock().unwrap();

        for patch in &input.patches {
            // Get new text (from clipboard if specified)
            let mut new_text = match &patch.from_clipboard {
                Some(name) => {
                    clipboards.get(name)
                        .ok_or_else(|| format!("Clipboard '{}' not found", name))?
                        .clone()
                }
                None => patch.new_text.clone().unwrap_or_default(),
            };

            // Apply reindentation to new text
            if let Some(reindent) = &patch.reindent {
                new_text = self.apply_reindent(&new_text, reindent)?;
            }

            // Store to clipboard if requested
            if let Some(name) = &patch.to_clipboard {
                if let Some(old_text) = &patch.old_text {
                    clipboards.insert(name.clone(), old_text.clone());
                }
            }

            // Determine edit based on operation
            let edit = match patch.operation {
                Operation::PrependBof => Edit {
                    offset: 0,
                    length: 0,
                    replacement: new_text,
                },
                Operation::AppendEof => Edit {
                    offset: original.len(),
                    length: 0,
                    replacement: new_text,
                },
                Operation::Overwrite => Edit {
                    offset: 0,
                    length: original.len(),
                    replacement: new_text,
                },
                Operation::Replace => {
                    let old_text = patch.old_text.as_ref()
                        .ok_or("Replace operation requires oldText")?;
                    
                    let spec = self.find_unique_match(&original, old_text)?;
                    
                    // Update clipboard with actual matched text if it differed
                    if let Some(name) = &patch.to_clipboard {
                        let matched = &original[spec.offset..spec.offset + spec.length];
                        clipboards.insert(name.clone(), matched.to_string());
                    }

                    Edit {
                        offset: spec.offset,
                        length: spec.length,
                        replacement: new_text,
                    }
                }
            };

            edits.push(edit);
        }

        // Apply edits (in reverse order to maintain offsets)
        let mut result = original.clone();
        edits.sort_by(|a, b| b.offset.cmp(&a.offset));
        
        for edit in &edits {
            if edit.offset + edit.length > result.len() {
                return Err("Edit extends beyond file content".to_string());
            }
            result.replace_range(edit.offset..edit.offset + edit.length, &edit.replacement);
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directories: {}", e))?;
            }
        }

        // Write the file
        fs::write(&path, &result)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        // Generate diff for display
        let diff = self.generate_diff(&input.path, &original, &result);

        Ok((path.display().to_string(), result, diff))
    }

    fn find_unique_match(&self, content: &str, old_text: &str) -> Result<EditSpec, String> {
        // 1. Try exact match
        if let Some(spec) = self.find_exact_unique(content, old_text) {
            return Ok(spec);
        }

        // 2. Try dedent matching
        if let Some(spec) = self.find_dedent_match(content, old_text) {
            return Ok(spec);
        }

        // 3. Try trimmed line match
        if let Some(spec) = self.find_trimmed_match(content, old_text) {
            return Ok(spec);
        }

        // Determine error type
        let count = content.matches(old_text).count();
        if count > 1 {
            Err(format!("oldText appears {} times in file (must be unique)", count))
        } else {
            Err("oldText not found in file".to_string())
        }
    }

    fn find_exact_unique(&self, content: &str, old_text: &str) -> Option<EditSpec> {
        let matches: Vec<_> = content.match_indices(old_text).collect();
        if matches.len() == 1 {
            Some(EditSpec {
                offset: matches[0].0,
                length: old_text.len(),
            })
        } else {
            None
        }
    }

    fn find_dedent_match(&self, content: &str, old_text: &str) -> Option<EditSpec> {
        // Extract the common leading whitespace from old_text
        let old_indent = self.common_leading_whitespace(old_text);
        
        // Try different indent levels found in the content
        for line in content.lines() {
            let line_indent = self.leading_whitespace(line);
            if line_indent != old_indent && !line_indent.is_empty() {
                // Try reindenting old_text to this level
                let adjusted = self.reindent_text(old_text, &old_indent, line_indent);
                if let Some(spec) = self.find_exact_unique(content, &adjusted) {
                    return Some(spec);
                }
            }
        }
        None
    }

    fn find_trimmed_match(&self, content: &str, old_text: &str) -> Option<EditSpec> {
        // Try trimming first and/or last lines
        let lines: Vec<&str> = old_text.lines().collect();
        if lines.len() <= 2 {
            return None;
        }

        // Try without first line
        let without_first = lines[1..].join("\n");
        if let Some(mut spec) = self.find_exact_unique(content, &without_first) {
            // Adjust offset to include the first line if it exists at that position
            if spec.offset > 0 {
                let before = &content[..spec.offset];
                if before.ends_with(lines[0]) || before.ends_with(&format!("{}\n", lines[0])) {
                    // First line is right before - extend the match
                    let first_line_with_newline = format!("{}\n", lines[0]);
                    if before.ends_with(&first_line_with_newline) {
                        spec.offset -= first_line_with_newline.len();
                        spec.length += first_line_with_newline.len();
                        return Some(spec);
                    }
                }
            }
            return Some(spec);
        }

        // Try without last line
        let without_last = lines[..lines.len()-1].join("\n");
        if let Some(spec) = self.find_exact_unique(content, &without_last) {
            return Some(spec);
        }

        None
    }

    fn leading_whitespace<'a>(&self, s: &'a str) -> &'a str {
        let trimmed = s.trim_start();
        &s[..s.len() - trimmed.len()]
    }

    fn common_leading_whitespace(&self, text: &str) -> String {
        let mut common: Option<String> = None;
        
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let ws = self.leading_whitespace(line).to_string();
            common = match common {
                None => Some(ws),
                Some(c) => {
                    // Find common prefix
                    let prefix: String = c.chars()
                        .zip(ws.chars())
                        .take_while(|(a, b)| a == b)
                        .map(|(a, _)| a)
                        .collect();
                    Some(prefix)
                }
            };
        }
        
        common.unwrap_or_default()
    }

    fn reindent_text(&self, text: &str, old_indent: &str, new_indent: &str) -> String {
        text.lines()
            .map(|line| {
                if line.trim().is_empty() {
                    line.to_string()
                } else if let Some(rest) = line.strip_prefix(old_indent) {
                    format!("{}{}", new_indent, rest)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn apply_reindent(&self, text: &str, reindent: &Reindent) -> Result<String, String> {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut result = Vec::with_capacity(lines.len());

        for line in lines {
            if line.is_empty() {
                result.push(String::new());
                continue;
            }

            // Strip prefix
            let stripped = if let Some(prefix) = &reindent.strip {
                line.strip_prefix(prefix.as_str()).ok_or_else(|| {
                    format!(
                        "Line does not start with expected prefix '{}':\n{}",
                        prefix, line
                    )
                })?
            } else {
                line
            };

            // Add prefix
            let final_line = if let Some(prefix) = &reindent.add {
                format!("{}{}", prefix, stripped)
            } else {
                stripped.to_string()
            };

            result.push(final_line);
        }

        Ok(result.join("\n"))
    }

    fn generate_diff(&self, path: &str, old: &str, new: &str) -> String {
        let diff = TextDiff::from_lines(old, new);
        let mut output = String::new();
        
        output.push_str(&format!("--- a/{}\n", path));
        output.push_str(&format!("--- b/{}\n", path));
        
        for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
            if idx > 0 {
                output.push_str("\n");
            }
            for op in group {
                for change in diff.iter_changes(op) {
                    let tag = match change.tag() {
                        ChangeTag::Delete => "-",
                        ChangeTag::Insert => "+",
                        ChangeTag::Equal => " ",
                    };
                    output.push_str(&format!("{}{}", tag, change));
                }
            }
        }
        
        output
    }

    fn is_autogenerated(&self, _path: &PathBuf, content: &str) -> bool {
        // Check for common autogenerated markers
        let markers = [
            "Code generated",
            "DO NOT EDIT",
            "generated by",
            "auto-generated",
            "@generated",
        ];

        // Only check early in file
        let header = &content[..content.len().min(2000)];
        let header_lower = header.to_lowercase();
        
        markers.iter().any(|m| header_lower.contains(&m.to_lowercase()))
    }
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> String {
        r#"File modification tool for precise text edits.

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
large overwrite. Prefer incremental replace operations over full file overwrites."#.to_string()
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

    async fn run(&self, input: Value) -> ToolOutput {
        // Check input size
        let input_str = input.to_string();
        if input_str.len() > MAX_INPUT_SIZE {
            return ToolOutput::error(format!(
                "Input too large ({} bytes). Maximum is {} bytes. Break into smaller patches.",
                input_str.len(),
                MAX_INPUT_SIZE
            ));
        }

        let input: PatchInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {}", e)),
        };

        if input.patches.is_empty() {
            return ToolOutput::error("No patches provided");
        }

        match self.apply_patches(&input) {
            Ok((path, content, diff)) => {
                let mut output = "<patches_applied>all</patches_applied>".to_string();
                
                // Check for autogenerated file
                let full_path = self.resolve_path(&input.path);
                if self.is_autogenerated(&full_path, &content) {
                    output.push_str("\n<warning>This file appears to be auto-generated. Edits may be overwritten.</warning>");
                }
                
                let display_data = json!({
                    "path": path,
                    "diff": diff
                });
                
                ToolOutput::success(output).with_display(display_data)
            }
            Err(e) => ToolOutput::error(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_replace_operation() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::new(dir.path().to_path_buf());
        
        // Create test file
        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "Hello World").unwrap();
        
        let result = tool.run(json!({
            "path": "test.txt",
            "patches": [{
                "operation": "replace",
                "oldText": "World",
                "newText": "Rust"
            }]
        })).await;
        
        assert!(result.success, "Error: {}", result.output);
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello Rust");
    }

    #[tokio::test]
    async fn test_append_operation() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::new(dir.path().to_path_buf());
        
        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "Hello").unwrap();
        
        let result = tool.run(json!({
            "path": "test.txt",
            "patches": [{
                "operation": "append_eof",
                "newText": " World"
            }]
        })).await;
        
        assert!(result.success);
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello World");
    }

    #[tokio::test]
    async fn test_overwrite_creates_file() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::new(dir.path().to_path_buf());
        
        let result = tool.run(json!({
            "path": "new_file.txt",
            "patches": [{
                "operation": "overwrite",
                "newText": "New content"
            }]
        })).await;
        
        assert!(result.success);
        let test_file = dir.path().join("new_file.txt");
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "New content");
    }

    #[tokio::test]
    async fn test_clipboard_operations() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::new(dir.path().to_path_buf());
        
        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "AAA BBB CCC").unwrap();
        
        // Cut BBB to clipboard
        tool.run(json!({
            "path": "test.txt",
            "patches": [{
                "operation": "replace",
                "oldText": "BBB",
                "newText": "",
                "toClipboard": "clip1"
            }]
        })).await;
        
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "AAA  CCC");
        
        // Paste from clipboard
        tool.run(json!({
            "path": "test.txt",
            "patches": [{
                "operation": "replace",
                "oldText": "CCC",
                "fromClipboard": "clip1"
            }]
        })).await;
        
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "AAA  BBB");
    }

    #[tokio::test]
    async fn test_reindent() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::new(dir.path().to_path_buf());
        
        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "MARKER").unwrap();
        
        let result = tool.run(json!({
            "path": "test.txt",
            "patches": [{
                "operation": "replace",
                "oldText": "MARKER",
                "newText": "  line1\n  line2",
                "reindent": {
                    "strip": "  ",
                    "add": "    "
                }
            }]
        })).await;
        
        assert!(result.success, "Error: {}", result.output);
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "    line1\n    line2");
    }

    #[tokio::test]
    async fn test_unique_match_required() {
        let dir = tempdir().unwrap();
        let tool = PatchTool::new(dir.path().to_path_buf());
        
        let test_file = dir.path().join("test.txt");
        fs::write(&test_file, "AAA BBB AAA").unwrap();
        
        let result = tool.run(json!({
            "path": "test.txt",
            "patches": [{
                "operation": "replace",
                "oldText": "AAA",
                "newText": "CCC"
            }]
        })).await;
        
        assert!(!result.success);
        assert!(result.output.contains("2 times"));
    }
}
