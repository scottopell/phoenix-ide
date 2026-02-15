---
created: 2026-02-15
priority: p2
status: ready
---

# Rewrite display_command using brush-parser for semantic awareness

## Problem

The current `display_command()` in `src/tools/bash_check.rs` uses tree-sitter to extract the "interesting" part of bash commands for UI display. It has a critical bug:

```
Input:  cat /path/to/file || echo "FILE NOT FOUND"
Actual: echo "FILE NOT FOUND"  (WRONG - shows fallback, not primary command)
Expect: cat /path/to/file || echo "FILE NOT FOUND" (or just cat /path/to/file)
```

The bug occurs because tree-sitter treats `&&` and `||` identically as "list" nodes, and the code iterates right-to-left returning the first non-cd command. This works for `cd /path && actual_cmd` but fails for `primary_cmd || fallback`.

## Root Cause

Tree-sitter is a **syntax-only** tool. It doesn't understand shell semantics:
- `&&` = run right side only if left succeeds
- `||` = run right side only if left fails  
- `;` = run sequentially regardless

To correctly transform commands, we need **semantic awareness** of these operators.

## Solution: Use brush-parser

The `brush-parser` crate (v0.3.0) provides a proper Rust AST for bash. It's already added to Cargo.toml.

### Key AST Types

```rust
use brush_parser::ast::{AndOrList, AndOr, Pipeline, Command, SimpleCommand};

// AndOrList structure:
struct AndOrList {
    first: Pipeline,              // First command
    additional: Vec<AndOr>,       // Subsequent && or || chains
}

enum AndOr {
    And(Pipeline),  // &&
    Or(Pipeline),   // ||
}

// Pipeline contains Vec<Command>, where Command can be:
enum Command {
    Simple(SimpleCommand),  // Regular command like "cd /foo" or "cargo test"
    Compound(...),          // Loops, conditionals, etc.
    ...
}

// SimpleCommand has the command name and args:
struct SimpleCommand {
    word_or_name: Option<Word>,  // Command name (e.g., "cd", "cargo")
    suffix: Option<CommandSuffix>,  // Arguments
}
```

### Parsing Example

```rust
use brush_parser::{Parser, ParserOptions, SourceInfo};
use std::io::Cursor;

fn parse(input: &str) -> brush_parser::ast::Program {
    let cursor = Cursor::new(input);
    let mut parser = Parser::new(cursor, &ParserOptions::default(), &SourceInfo::default());
    parser.parse_program().unwrap()
}

// The AST implements Display for roundtrip:
let prog = parse("cd /foo && cargo test");
assert_eq!(prog.to_string(), "cd /foo && cargo test");
```

## Implementation Requirements

### REQ-1: Preserve semantics for || chains

For `cmd1 || cmd2`, the primary command is `cmd1`. The fallback `cmd2` only runs on failure.

```rust
// WRONG: return "echo FILE NOT FOUND"
// RIGHT: return "cat /path/to/file" or the whole thing
display_command(r#"cat /path || echo "FILE NOT FOUND""#)
```

### REQ-2: Strip cd prefixes from && chains only

For `cd /path && actual_cmd`, stripping `cd` is safe because:
- If cd succeeds → actual_cmd runs (what we want to show)
- If cd fails → actual_cmd doesn't run (nothing shown anyway)

```rust
assert_eq!(display_command("cd /foo && cargo test"), "cargo test");
assert_eq!(display_command("cd /a && cd /b && npm build"), "npm build");
```

### REQ-3: Handle ; (sequence) chains

For `cd /path; cmd`, both run regardless. Strip the cd:

```rust
assert_eq!(display_command("cd /foo; npm test"), "npm test");
```

### REQ-4: Handle mixed chains correctly

Apply transformations based on operator semantics:

```rust
// cd && (cmd || fallback) → cmd || fallback  (strip cd)
assert_eq!(
    display_command(r#"cd /app && cat file || echo "not found""#),
    r#"cat file || echo "not found""#
);
```

### REQ-5: Fallback gracefully on parse errors

If brush-parser fails, return the original string unchanged:

```rust
fn display_command(script: &str) -> &str {
    // Try brush-parser first
    // On error, fallback to returning script unchanged
}
```

### REQ-6: Property-based testing

Add proptest coverage to ensure:
1. Output is always valid bash (roundtrips through parser)
2. Semantic preservation: transformed command executes same logic when pwd matches cd target
3. Never produces empty or dangling-operator output

## Algorithm

```
function simplify(and_or_list):
    result_pipelines = []
    current_op = None  // Start with implicit "first"
    
    for (op, pipeline) in and_or_list:
        if is_cd_command(pipeline):
            if op == And or op == Sequence:
                // Safe to skip - cd success means next runs
                continue
            elif op == Or:
                // NOT safe - cd failure means next runs
                // Keep the cd
                result_pipelines.append((op, pipeline))
        else:
            result_pipelines.append((op, pipeline))
    
    // Reconstruct from result_pipelines
    // If empty, return original
```

## Files to Modify

- `src/tools/bash_check.rs`: Replace tree-sitter implementation with brush-parser
- Keep `check()` function using tree-sitter (safety checks are syntax-only)
- Only change `display_command()` to use brush-parser

## Testing

```bash
cargo test display  # Run display_command tests
```

Existing tests should continue to pass. Add new tests for || chains.

## Debug Logging

The `enrich_message_json` function in `src/api/handlers.rs` already has debug logging:

```rust
tracing::debug!(
    command = %command,
    display = %display_str,
    "bash display_command transformation"
);
```

To see transformations in prod: `./dev.py prod set RUST_LOG debug`
