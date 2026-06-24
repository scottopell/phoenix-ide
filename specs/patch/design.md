# Patch Tool - Design Document

## Overview

The patch tool enables precise file editing without full file rewrites. It supports atomic multi-patch operations, clipboard-based cut/copy/paste, and indentation adjustment for moving code between contexts.

## Tool Interface (REQ-PATCH-006)

### Schema

```json
{
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
            "enum": ["replace", "insert_before", "insert_after", "append_eof", "prepend_bof", "overwrite"],
            "description": "Type of operation to perform"
          },
          "oldText": {
            "type": "string",
            "description": "Text to locate (must be unique in file unless replaceAll; required for replace, insert_before, insert_after)"
          },
          "newText": {
            "type": "string",
            "description": "The new text to use, or the text to insert for the insert operations (empty for deletions, leave empty if fromClipboard is set)"
          },
          "replaceAll": {
            "type": "boolean",
            "description": "Replace only: substitute every exact occurrence of oldText instead of requiring a unique match (exact matches only, no fuzzy recovery)"
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
}
```

### Description

```
File modification tool for precise text edits.

Operations:
- replace: Substitute unique text with new content
- insert_before: Insert newText immediately before a unique oldText anchor, leaving the anchor unchanged
- insert_after: Insert newText immediately after a unique oldText anchor, leaving the anchor unchanged
- append_eof: Append new text at the end of the file
- prepend_bof: Insert new text at the beginning of the file
- overwrite: Replace the entire file with new content (automatically creates the file)

replaceAll (replace only): substitute every exact occurrence of oldText (exact matches only, no fuzzy recovery).

Clipboard:
- toClipboard: Store oldText to a named clipboard before the operation
- fromClipboard: Use clipboard content as newText (ignores provided newText)
- Clipboards persist across patch calls
- Always use clipboards when moving/copying code (within or across files)

Indentation adjustment:
- reindent applies to whatever text is being inserted
- First strips the specified prefix from each line, then adds the new prefix
- Useful when moving code from one indentation to another

Recipes:
- cut: replace with empty newText and toClipboard
- copy: replace with toClipboard and fromClipboard using the same clipboard name
- paste: replace with fromClipboard

Usage notes:
- All inputs are interpreted literally (no automatic newline handling)
- For replace, insert_before, and insert_after, oldText must appear EXACTLY ONCE in the file (unless replaceAll)
- Two patches in one call whose ranges overlap are rejected
- On success the applied unified diff is returned so you can confirm placement without re-reading
```

## Core Data Structures

```rust
struct PatchInput {
    path: String,
    patches: Vec<PatchRequest>,
}

struct PatchRequest {
    operation: Operation,
    old_text: Option<String>,
    new_text: Option<String>,
    replace_all: bool,          // replace only: substitute every exact occurrence
    to_clipboard: Option<String>,
    from_clipboard: Option<String>,
    reindent: Option<Reindent>,
}

enum Operation {
    Replace,
    InsertBefore,
    InsertAfter,
    AppendEof,
    PrependBof,
    Overwrite,
}

struct Reindent {
    strip: Option<String>,
    add: Option<String>,
}

/// Clipboards persist across tool calls within a conversation
struct PatchTool {
    working_dir: PathBuf,
    clipboards: HashMap<String, String>,
}
```

## Execution Flow (REQ-PATCH-001, REQ-PATCH-002)

```rust
impl PatchTool {
    pub fn run(&mut self, input: PatchInput) -> ToolResult {
        let path = self.resolve_path(&input.path);
        
        // Read original content (or empty for new files)
        let original = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == NotFound => {
                // Only certain operations allowed on new files
                self.validate_new_file_operations(&input.patches)?;
                String::new()
            }
            Err(e) => return ToolResult::Error(format!("failed to read: {}", e)),
        };
        
        // Build edit buffer for atomic application
        let mut buffer = EditBuffer::new(&original);
        
        // Process all patches against original content
        for patch in &input.patches {
            self.apply_patch(&original, &mut buffer, patch)?;
        }
        
        // Write atomically
        let patched = buffer.to_string();
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, &patched)?;
        
        // Generate output
        let diff = generate_unified_diff(&path, &original, &patched);
        ToolResult::Success { 
            message: "<patches_applied>all</patches_applied>",
            display: PatchDisplay { path, old: original, new: patched, diff },
        }
    }
}
```

## Patch Application (REQ-PATCH-001)

```rust
fn apply_patch(
    &mut self,
    original: &str,
    buffer: &mut EditBuffer,
    patch: &PatchRequest,
) -> Result<(), PatchError> {
    // Handle clipboard operations first
    if let Some(name) = &patch.to_clipboard {
        if patch.operation != Operation::Replace {
            return Err(PatchError::ClipboardRequiresReplace);
        }
        let old_text = patch.old_text.as_ref().ok_or(PatchError::MissingOldText)?;
        self.clipboards.insert(name.clone(), old_text.clone());
    }
    
    // Determine new text (fromClipboard overrides newText)
    let mut new_text = match &patch.from_clipboard {
        Some(name) => self.clipboards.get(name)
            .ok_or_else(|| PatchError::ClipboardNotFound(name.clone()))?
            .clone(),
        None => patch.new_text.clone().unwrap_or_default(),
    };
    
    // Apply reindentation
    if let Some(reindent) = &patch.reindent {
        new_text = apply_reindent(&new_text, reindent)?;
    }
    
    // Apply operation
    match patch.operation {
        Operation::PrependBof => buffer.insert(0, &new_text),
        Operation::AppendEof => buffer.insert(original.len(), &new_text),
        Operation::Overwrite => buffer.replace(0, original.len(), &new_text),
        // insert_before / insert_after locate a unique anchor exactly as replace
        // does, then emit a zero-length edit at the anchor's start / end. The
        // anchor bytes are never part of the edit, so they are preserved.
        Operation::InsertBefore => {
            let old_text = patch.old_text.as_ref().ok_or(PatchError::MissingOldText)?;
            let spec = self.find_unique_match(original, old_text, &new_text)?;
            buffer.insert(spec.offset, &new_text);
        }
        Operation::InsertAfter => {
            let old_text = patch.old_text.as_ref().ok_or(PatchError::MissingOldText)?;
            let spec = self.find_unique_match(original, old_text, &new_text)?;
            buffer.insert(spec.offset + spec.length, &new_text);
        }
        // replaceAll substitutes every exact occurrence (no fuzzy cascade). Zero
        // exact matches => OldTextNotFound, unless the text is present only as a
        // fuzzy/near match, in which case => ReplaceAllInexact. It must never
        // succeed as a no-op.
        Operation::Replace if patch.replace_all => {
            let old_text = patch.old_text.as_ref().ok_or(PatchError::MissingOldText)?;
            let specs = find_all_exact(original, old_text);
            if specs.is_empty() {
                return Err(match find_unique_match(original, old_text, &new_text) {
                    Err(PatchError::OldTextNotFound(_)) => PatchError::OldTextNotFound(old_text.clone()),
                    _ => PatchError::ReplaceAllInexact,
                });
            }
            for spec in specs {
                buffer.replace(spec.offset, spec.length, &new_text);
            }
        }
        Operation::Replace => {
            let old_text = patch.old_text.as_ref().ok_or(PatchError::MissingOldText)?;
            let spec = self.find_unique_match(original, old_text, &new_text)?;
            buffer.replace(spec.offset, spec.length, &new_text);
            
            // Update clipboard with actual matched text if fuzzy matched
            if let Some(name) = &patch.to_clipboard {
                let matched = &original[spec.offset..spec.offset + spec.length];
                self.clipboards.insert(name.clone(), matched.to_string());
            }
        }
    }
    Ok(())
}
```

## Fuzzy Matching (REQ-PATCH-005)

When exact match fails, try recovery strategies in order:

```rust
fn find_unique_match(
    &self,
    original: &str,
    old_text: &str,
    new_text: &str,
) -> Result<MatchSpec, PatchError> {
    // 1. Exact unique match
    if let Some(spec) = find_exact_unique(original, old_text) {
        return Ok(spec);
    }
    
    // 2. Dedent matching - adjust leading whitespace
    if let Some(spec) = find_unique_dedent(original, old_text) {
        return Ok(spec);
    }
    
    // 3. Trim first/last lines if safe
    if let Some(spec) = find_unique_trimmed(original, old_text) {
        return Ok(spec);
    }

    // 4. Unicode confusable-skeleton match (lookalikes: curly quotes, em dash,
    //    ellipsis, fullwidth). Both content and old_text are skeletonised and
    //    matched in skeleton space, then mapped back to original bytes. A match
    //    is accepted ONLY if its mapped original slice itself skeletonises back
    //    to old_text's skeleton — this rejects a needle landing inside a single
    //    character's multi-char skeleton expansion (e.g. ".." inside "…"), which
    //    would otherwise map to a misaligned, file-corrupting edit.
    if let Some(spec) = find_unique_skeleton(original, old_text) {
        return Ok(spec);
    }
    
    // Check if multiple matches (different error)
    let count = count_occurrences(original, old_text);
    if count > 1 {
        Err(PatchError::OldTextNotUnique(old_text.to_string()))
    } else {
        Err(PatchError::OldTextNotFound(old_text.to_string()))
    }
}
```

### Dedent Matching

```rust
/// Try matching with adjusted indentation prefix
fn find_unique_dedent(original: &str, old_text: &str) -> Option<MatchSpec> {
    // Extract common leading whitespace from old_text
    let old_indent = common_leading_whitespace(old_text);
    
    // Try matching with each possible indent level in original
    for indent in possible_indents(original) {
        let adjusted = reindent_text(old_text, &old_indent, &indent);
        if let Some(spec) = find_exact_unique(original, &adjusted) {
            return Some(spec);
        }
    }
    None
}
```

## Reindentation (REQ-PATCH-004)

```rust
fn apply_reindent(text: &str, reindent: &Reindent) -> Result<String, PatchError> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result = Vec::with_capacity(lines.len());
    
    for line in lines {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }
        
        // Strip prefix
        let stripped = if let Some(prefix) = &reindent.strip {
            line.strip_prefix(prefix).ok_or_else(|| {
                PatchError::StripPreconditionFailed {
                    line: line.to_string(),
                    prefix: prefix.clone(),
                }
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
```

## Edit Buffer (REQ-PATCH-002)

Atomic multi-patch application using offset tracking:

```rust
struct EditBuffer {
    original: String,
    edits: Vec<Edit>,
}

struct Edit {
    offset: usize,      // Offset in original
    length: usize,      // Length to remove from original
    replacement: String, // Text to insert
}

impl EditBuffer {
    fn insert(&mut self, offset: usize, text: &str) {
        self.edits.push(Edit { offset, length: 0, replacement: text.to_string() });
    }
    
    fn replace(&mut self, offset: usize, length: usize, text: &str) {
        self.edits.push(Edit { offset, length, replacement: text.to_string() });
    }
    
    fn to_string(&self) -> Result<String, PatchError> {
        // Sort by offset descending; ties break by length descending then by
        // request index descending, so a replace at O precedes a zero-length
        // insert at O, and two inserts at one offset keep request order.
        let mut edits = self.edits.clone();
        edits.sort_by(/* (offset desc, length desc, request index desc) */);
        
        // Edits resolve against the original, so two edits with overlapping byte
        // ranges have no well-defined combined result: reject rather than corrupt.
        // A zero-length insert sharing only a boundary does not overlap.
        let mut result = self.original.clone();
        let mut prev_start = None;
        for edit in edits {
            if let Some(prev_start) = prev_start {
                if edit.offset + edit.length > prev_start {
                    return Err(PatchError::OverlappingEdits);
                }
            }
            prev_start = Some(edit.offset);
            result.replace_range(edit.offset..edit.offset + edit.length, &edit.replacement);
        }
        Ok(result)
    }
}
```

## Clipboard Recipes

### Cut
```json
{
  "operation": "replace",
  "oldText": "text to cut",
  "newText": "",
  "toClipboard": "myClip"
}
```

### Copy
```json
{
  "operation": "replace",
  "oldText": "text to copy",
  "toClipboard": "myClip",
  "fromClipboard": "myClip"
}
```

### Paste
```json
{
  "operation": "replace",
  "oldText": "replace this",
  "fromClipboard": "myClip"
}
```

### Move with Reindent
```json
[
  {
    "operation": "replace",
    "oldText": "    fn helper() {...}",
    "newText": "",
    "toClipboard": "fn"
  }
]
```
Then in another file/location:
```json
[
  {
    "operation": "replace",
    "oldText": "// INSERT HERE",
    "fromClipboard": "fn",
    "reindent": { "strip": "    ", "add": "        " }
  }
]
```

## Autogenerated File Detection (REQ-PATCH-007)

```rust
fn is_autogenerated(path: &Path, content: &str) -> bool {
    // Check file extension
    if !path.extension().map_or(false, |e| e == "go") {
        return false;
    }
    
    // Check for common autogenerated markers
    let markers = [
        "Code generated",
        "DO NOT EDIT",
        "generated by",
        "auto-generated",
    ];
    
    // Only check early in file (before imports end)
    let header = &content[..content.len().min(2000)];
    markers.iter().any(|m| header.to_lowercase().contains(&m.to_lowercase()))
}
```

## Error Types

```rust
enum PatchError {
    OldTextNotFound(String),
    OldTextNotUnique(String),
    ReplaceAllInexact,            // replaceAll: no exact match, but a fuzzy/near match exists
    MissingOldText,
    ClipboardNotFound(String),
    ClipboardRequiresReplace,
    OverlappingEdits,             // two edits in one call target overlapping ranges
    StripPreconditionFailed { line: String, prefix: String },
    FileNotFound(PathBuf),
    IoError(std::io::Error),
    InputTooLarge,
}
```

## Testing Strategy

### Unit Tests
- Each operation type (replace, append, prepend, overwrite)
- Clipboard operations (cut, copy, paste)
- Reindentation edge cases
- Fuzzy matching strategies
- Error conditions (not found, not unique, missing clipboard)

### Integration Tests
- Multi-patch atomic application
- File creation with parent directories
- Autogenerated file warning
- Large file handling

### Property Tests
```rust
#[proptest]
fn replace_is_reversible(original: String, old: String, new: String) {
    // If replace(old, new) succeeds, replace(new, old) should restore original
    // (assuming old and new are both unique)
}

#[proptest]
fn clipboard_round_trip(text: String, name: String) {
    // toClipboard then fromClipboard preserves text exactly
}
```

## File Organization

```
src/tools/
├── patch/
│   ├── mod.rs
│   ├── operations.rs    # Core patch operations
│   ├── clipboard.rs     # Clipboard management
│   ├── reindent.rs      # Indentation adjustment
│   ├── fuzzy.rs         # Fuzzy matching strategies
│   ├── editbuf.rs       # Atomic edit buffer
│   └── tests.rs
```
