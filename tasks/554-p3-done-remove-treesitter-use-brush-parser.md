---
created: 2025-03-28
priority: p3
status: done
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
use brush_parser::ast::{
    Command, CompoundCommand, CompoundList, Pipeline, SimpleCommand,
    AndOrList, AndOr, CommandPrefixOrSuffixItem,
};

pub fn check(script: &str) -> Result<(), CheckError> {
    let cursor = Cursor::new(script);
    let mut parser = Parser::new(cursor, &ParserOptions::default(), &SourceInfo::default());
    let program = parser.parse_program().map_err(|_| CheckError {
        message: "Failed to parse script".into(),
    })?;
    
    // Program contains Vec<CompleteCommand>, where CompleteCommand = CompoundList
    for complete_cmd in &program.complete_commands {
        check_compound_list(complete_cmd)?;
    }
    Ok(())
}
```

### AST Structure Overview

The brush-parser AST hierarchy for finding commands:

```
Program
 └── complete_commands: Vec<CompleteCommand>  (CompleteCommand = CompoundList)
      └── CompoundList(Vec<CompoundListItem>)  // semicolon/ampersand separated
           └── CompoundListItem(AndOrList, SeparatorOperator)
                └── AndOrList { first: Pipeline, additional: Vec<AndOr> }  // && and || chains
                     └── AndOr::And(Pipeline) | AndOr::Or(Pipeline)
                          └── Pipeline { seq: Vec<Command> }  // pipe-separated
                               └── Command::Simple(SimpleCommand)  // <-- check these
                               └── Command::Compound(CompoundCommand, _)  // recurse into these
                               └── Command::Function(FunctionDefinition)  // recurse into body
```

### Recursive Traversal Functions

```rust
fn check_compound_list(list: &CompoundList) -> Result<(), CheckError> {
    for item in &list.0 {
        check_and_or_list(&item.0)?;  // item.1 is SeparatorOperator (;/&)
    }
    Ok(())
}

fn check_and_or_list(list: &AndOrList) -> Result<(), CheckError> {
    check_pipeline(&list.first)?;
    for and_or in &list.additional {
        match and_or {
            AndOr::And(pipeline) | AndOr::Or(pipeline) => check_pipeline(pipeline)?,
        }
    }
    Ok(())
}

fn check_pipeline(pipeline: &Pipeline) -> Result<(), CheckError> {
    for cmd in &pipeline.seq {
        check_command(cmd)?;
    }
    Ok(())
}

fn check_command(cmd: &Command) -> Result<(), CheckError> {
    match cmd {
        Command::Simple(simple) => check_simple_command(simple),
        Command::Compound(compound, _redirects) => check_compound_command(compound),
        Command::Function(func) => {
            // FunctionBody contains CompoundCommand
            check_compound_command(&func.body.0)
        }
        Command::ExtendedTest(_) => Ok(()),  // [[ ... ]] doesn't execute commands
    }
}

fn check_compound_command(cmd: &CompoundCommand) -> Result<(), CheckError> {
    match cmd {
        CompoundCommand::BraceGroup(bg) => check_compound_list(&bg.body),
        CompoundCommand::Subshell(sub) => check_compound_list(&sub.body),
        CompoundCommand::ForClause(fc) => check_compound_list(&fc.body.list),  // DoGroupCommand.list
        CompoundCommand::WhileClause(wc) | CompoundCommand::UntilClause(wc) => {
            check_compound_list(&wc.0)?;  // condition
            check_compound_list(&wc.1.list)  // body (DoGroupCommand.list)
        }
        CompoundCommand::IfClause(ic) => {
            check_compound_list(&ic.condition)?;
            check_compound_list(&ic.then)?;
            if let Some(elses) = &ic.elses {
                for else_clause in elses {
                    // ElseClause has condition (Option<CompoundList>) and body (CompoundList)
                    if let Some(cond) = &else_clause.condition {
                        check_compound_list(cond)?;
                    }
                    check_compound_list(&else_clause.body)?;
                }
            }
            Ok(())
        }
        CompoundCommand::CaseClause(cc) => {
            // CaseClauseCommand has items: Vec<CaseItem>
            // CaseItem has body: Option<CompoundList>
            for item in &cc.items {
                if let Some(body) = &item.body {
                    check_compound_list(body)?;
                }
            }
            Ok(())
        }
        CompoundCommand::Arithmetic(_) | CompoundCommand::ArithmeticForClause(_) => Ok(()),
    }
}
```

### Extract Args from SimpleCommand

```rust
fn check_simple_command(cmd: &SimpleCommand) -> Result<(), CheckError> {
    let name = cmd.word_or_name.as_ref().map(|w| w.to_string());
    let Some(name) = name else { return Ok(()) };  // assignment-only commands
    
    let mut args: Vec<String> = vec![name.clone()];
    if let Some(suffix) = &cmd.suffix {
        for item in &suffix.0 {
            if let CommandPrefixOrSuffixItem::Word(word) = item {
                args.push(word.to_string());
            }
            // Skip redirects (CommandPrefixOrSuffixItem::IoRedirect)
        }
    }
    
    // Route to existing checkers
    match name.as_str() {
        "git" => check_git_command(&args),
        "rm" => check_rm_command(&args),
        _ => Ok(()),
    }
}
```

## Acceptance Criteria

- [x] Remove `tree-sitter` and `tree-sitter-bash` from Cargo.toml
- [x] Rewrite `check()` function to use brush-parser AST
- [x] All existing tests pass (there are ~40 tests in the file)
- [x] No tree-sitter imports remain in the codebase

## Notes

- The existing `check_git_command()`, `check_git_add()`, `check_git_push()`, `check_rm_command()` logic can stay mostly unchanged - they work on `&[String]` slices
- Main work is replacing `parse_script()`, `check_node()`, and `collect_command_args()` with brush-parser equivalents
- Need to handle nested commands (pipelines, subshells, etc.) - walk the full AST
- brush-parser's `Command` enum has variants: `Simple`, `Compound`, `Function`, `ExtendedTest`
