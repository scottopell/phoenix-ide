---
created: 2025-03-28
priority: p3
status: ready
---

# Remove tree-sitter, use brush-parser exclusively

## Summary

Replace tree-sitter with brush-parser for bash command safety checks in `src/tools/bash_check.rs`. The file currently uses both parsers - brush-parser for `display_command()` and tree-sitter for `check()`.

## Context

We have two bash parsers in our dependencies:
- `tree-sitter` + `tree-sitter-bash` - used only for `check()` function (safety checks)
- `brush-parser` - used for `display_command()` (cd-stripping logic)

brush-parser is a pure-Rust bash parser with a proper AST. tree-sitter is a C-based parser with FFI overhead. Since brush-parser can do everything tree-sitter does here, we should consolidate.

## Current State

**Tree-sitter usage** (lines ~7, 23-40, 241-280):
- `parse_script()` - creates tree-sitter parser, returns `Tree`
- `check()` - entry point, parses script and calls `check_node()`
- `check_node()` - recursively walks tree-sitter nodes looking for `command` nodes
- `collect_command_args()` - extracts command name and arguments from tree-sitter nodes
- `check_command()` - routes to specific checkers based on command name

**What needs checking:**
- `git add -A|--all|.|*` → block
- `git push --force|-f` → block (but allow `--force-with-lease`)
- `rm -rf` with dangerous paths (`/`, `~`, `$HOME`, `.git`, `*`) → block

## Implementation Approach

Reuse the brush-parser infrastructure already in the file:

```rust
use brush_parser::{Parser, ParserOptions, SourceInfo};
use brush_parser::ast::{Command, SimpleCommand, ...};

pub fn check(script: &str) -> Result<(), CheckError> {
    let cursor = Cursor::new(script);
    let mut parser = Parser::new(cursor, &ParserOptions::default(), &SourceInfo::default());
    let program = parser.parse_program().map_err(|_| CheckError {
        message: "Failed to parse script".into(),
    })?;
    
    // Walk the AST and check each SimpleCommand
    for complete_cmd in &program.complete_commands {
        check_compound_list(complete_cmd)?;
    }
    Ok(())
}
```

Extract command args from `SimpleCommand`:
- `cmd.word_or_name` → command name (e.g., "git", "rm")
- `cmd.suffix` → `CommandSuffix(Vec<CommandPrefixOrSuffixItem>)`
  - Match `CommandPrefixOrSuffixItem::Word(word)` to get arguments
  - Use `.to_string()` on `Word` to get the string value

## Acceptance Criteria

- [ ] Remove `tree-sitter` and `tree-sitter-bash` from Cargo.toml
- [ ] Rewrite `check()` function to use brush-parser AST
- [ ] All existing tests pass (there are ~40 tests in the file)
- [ ] No tree-sitter imports remain in the codebase

## Notes

- The existing `check_git_command()`, `check_git_add()`, `check_git_push()`, `check_rm_command()` logic can stay mostly unchanged - they work on `&[String]` slices
- Main work is replacing `parse_script()`, `check_node()`, and `collect_command_args()` with brush-parser equivalents
- Need to handle nested commands (pipelines, subshells, etc.) - walk the full AST
- brush-parser's `Command` enum has variants: `Simple`, `Compound`, `Function`, `ExtendedTest`
