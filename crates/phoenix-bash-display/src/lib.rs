//! Bash command display simplification.
//!
//! LLMs commonly emit commands like `cd /path && actual_command`. For UI
//! display, the `cd` prefix is boilerplate. These helpers strip it when
//! semantically safe (followed by `&&` or `;`) and only when the `cd` target
//! matches the conversation's working directory.
//!
//! Pure brush-parser leaf: depends only on `brush-parser` + std. No
//! phoenix-core or other-crate types.

use brush_parser::ast::{AndOr, AndOrList, Command, Pipeline, SimpleCommand};
use brush_parser::{Parser, ParserOptions};
use std::io::Cursor;

/// Extract the "interesting" part of a bash command for display purposes.
///
/// LLMs commonly emit commands like `cd /path && actual_command`. For UI display,
/// we want to show `actual_command` since the `cd` prefix is boilerplate.
///
/// This uses brush-parser for semantic awareness of `&&` vs `||` operators:
/// - `&&` means "run next if previous succeeded" - safe to strip cd prefix (if path matches cwd)
/// - `||` means "run next if previous failed" - NOT safe to strip, shows fallback
/// - `;` means "run sequentially" - safe to strip cd prefix (if path matches cwd)
///
/// The `cwd` parameter is the conversation's working directory. We only strip cd prefixes
/// when they match this directory - stripping `cd /other/path` would be misleading.
///
/// Examples (assuming cwd = "/foo"):
/// - `cd /foo && cargo test` -> `cargo test` (matches cwd)
/// - `cd /bar && cargo test` -> `cd /bar && cargo test` (different path, keep full command)
/// - `cd /foo; npm build` -> `npm build` (matches cwd)
/// - `cargo test` -> `cargo test` (unchanged)
/// - `cd /foo` -> `cd /foo` (no follow-up command)
/// - `cat file || echo "not found"` -> `cat file || echo "not found"` (preserve || chains)
#[must_use]
pub fn display_command(script: &str, cwd: &str) -> String {
    // Try brush-parser first for semantic awareness
    if let Some(simplified) = simplify_with_brush(script, cwd) {
        return simplified;
    }
    // Fallback: return original unchanged
    script.to_string()
}

/// Parse script with brush-parser and simplify by stripping cd prefixes
/// only when semantically safe (followed by && or ;) AND the cd target matches cwd.
fn simplify_with_brush(script: &str, cwd: &str) -> Option<String> {
    use brush_parser::ast::SeparatorOperator;

    let cursor = Cursor::new(script);
    let mut parser = Parser::new(cursor, &ParserOptions::default());
    let program = parser.parse_program().ok()?;

    // Process each complete command list
    // A CompoundList contains multiple items separated by ; or &
    // Each item is (AndOrList, SeparatorOperator)
    let mut results: Vec<String> = Vec::new();

    for complete_cmd in &program.complete_commands {
        let items: &Vec<_> = &complete_cmd.0;

        // Process the list of items, where items are separated by ; or &
        // We can strip cd commands when followed by ; (Sequence), because
        // both commands run regardless, so cd success/failure doesn't gate the next
        // BUT only if the cd target matches the conversation's working directory
        let mut i = 0;
        while i < items.len() {
            let item = &items[i];
            let and_or_list = &item.0;
            let separator = &item.1;

            // Check if this entire AndOrList is just a cd command to the cwd
            if is_cd_to_cwd_and_or_list(and_or_list, cwd) {
                // If followed by ; (Sequence), safe to skip
                // If followed by & (Async), also safe to skip (runs in parallel)
                match separator {
                    SeparatorOperator::Sequence | SeparatorOperator::Async => {
                        // Safe to skip - next command runs regardless
                        i += 1;
                        continue;
                    }
                }
            }

            // Process the AndOrList with && and || awareness
            let simplified = simplify_and_or_list(and_or_list, cwd);
            if !simplified.is_empty() {
                results.push(simplified);
            }
            i += 1;
        }
    }

    if results.is_empty() {
        // Everything was cd commands - return original
        return Some(script.to_string());
    }

    Some(results.join("; "))
}

/// Check if an `AndOrList` is just a simple cd command to the cwd (no && or || chains)
fn is_cd_to_cwd_and_or_list(list: &AndOrList, cwd: &str) -> bool {
    // Must have no additional && or || operations
    if !list.additional.is_empty() {
        return false;
    }
    // The first (and only) pipeline must be a cd command to cwd
    is_cd_to_cwd_pipeline(&list.first, cwd)
}

/// Simplify an `AndOrList` by stripping leading cd commands that are followed by `&&`
/// AND that cd to the conversation's working directory.
/// Returns the simplified command string.
fn simplify_and_or_list(list: &AndOrList, cwd: &str) -> String {
    // Collect (operator, pipeline) pairs
    // The first pipeline has an implicit "start" operator
    // Additional pipelines have And or Or operators

    #[derive(Debug, Clone, Copy)]
    enum Op {
        Start, // First command in the list
        And,   // &&
        Or,    // ||
    }

    let mut items: Vec<(Op, &Pipeline)> = Vec::new();
    items.push((Op::Start, &list.first));

    for and_or in &list.additional {
        match and_or {
            AndOr::And(pipeline) => items.push((Op::And, pipeline)),
            AndOr::Or(pipeline) => items.push((Op::Or, pipeline)),
        }
    }

    // Now filter: strip cd commands that are followed by && or at Start
    // BUT only if the cd target matches the cwd
    // NEVER strip cd commands that are followed by ||
    let mut result_items: Vec<(Op, &Pipeline)> = Vec::new();

    for (i, (op, pipeline)) in items.iter().enumerate() {
        let is_cd_to_cwd = is_cd_to_cwd_pipeline(pipeline, cwd);

        // Check what operator connects this to the NEXT command
        let next_op = items.get(i + 1).map(|(o, _)| *o);

        // cd to cwd followed by && (or at start before &&): safe to skip
        // The cd succeeds → next runs (what we want to show)
        // The cd fails → nothing runs anyway
        // Only strip if cd target matches conversation's working directory
        if is_cd_to_cwd && matches!(next_op, Some(Op::And | Op::Start)) {
            continue;
        }

        // Keep non-cd commands, cd to different dirs, cd followed by ||, and cd at the end
        result_items.push((*op, pipeline));
    }

    if result_items.is_empty() {
        return String::new();
    }

    // Reconstruct the command string
    let mut output = String::new();
    for (i, (op, pipeline)) in result_items.iter().enumerate() {
        if i > 0 {
            match op {
                Op::Start => {} // Shouldn't happen after first
                Op::And => output.push_str(" && "),
                Op::Or => output.push_str(" || "),
            }
        }
        output.push_str(&pipeline.to_string());
    }

    output
}

/// Check if a pipeline is a simple `cd` command to the given cwd
fn is_cd_to_cwd_pipeline(pipeline: &Pipeline, cwd: &str) -> bool {
    // A cd pipeline should have exactly one command and it should be a simple `cd` command
    if pipeline.seq.len() != 1 {
        return false;
    }

    match &pipeline.seq[0] {
        Command::Simple(simple) => is_cd_to_cwd_simple_command(simple, cwd),
        Command::Compound(..) | Command::Function(_) | Command::ExtendedTest(..) => false,
    }
}

/// Check if a `SimpleCommand` is a `cd` command to the given cwd
fn is_cd_to_cwd_simple_command(command: &SimpleCommand, cwd: &str) -> bool {
    // Check if command name is "cd"
    let is_cd = command
        .word_or_name
        .as_ref()
        .is_some_and(|w| w.to_string() == "cd");

    if !is_cd {
        return false;
    }

    // Extract the cd target path from the suffix
    let target = command
        .suffix
        .as_ref()
        .and_then(|s| s.0.first())
        .map(std::string::ToString::to_string);

    let Some(target) = target else {
        // `cd` with no argument - goes to home, not cwd
        return false;
    };

    // Normalize and compare paths
    paths_match(&target, cwd)
}

/// Check if two paths refer to the same directory
/// Handles cases like "/foo" vs "/foo/", "./foo" vs "/abs/foo", etc.
fn paths_match(target: &str, cwd: &str) -> bool {
    use std::path::Path;

    // Handle ~ expansion (common in cd commands)
    let target = if target.starts_with('~') {
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy();
            if target == "~" {
                home.to_string()
            } else if let Some(rest) = target.strip_prefix("~/") {
                format!("{home}/{rest}")
            } else {
                // ~user syntax - don't try to expand
                return false;
            }
        } else {
            return false;
        }
    } else {
        target.to_string()
    };

    let target_path = Path::new(&target);
    let cwd_path = Path::new(cwd);

    // If target is relative, resolve it against cwd
    let target_abs = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        cwd_path.join(target_path)
    };

    // Canonicalize both paths for comparison (handles .., symlinks, etc.)
    // If canonicalization fails (path doesn't exist), fall back to string comparison
    let target_canonical = target_abs
        .canonicalize()
        .unwrap_or_else(|_| target_abs.clone());
    let cwd_canonical = cwd_path
        .canonicalize()
        .unwrap_or_else(|_| cwd_path.to_path_buf());

    target_canonical == cwd_canonical
}

#[cfg(test)]
mod proptests;

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Display Command Tests ====================
    //
    // REQ-BASH-011: Only strip cd prefixes when target matches cwd

    #[test]
    fn test_display_strips_cd_when_matches_cwd() {
        // cd target matches cwd - strip the cd prefix
        assert_eq!(
            display_command("cd /foo && cargo test", "/foo"),
            "cargo test"
        );
    }

    #[test]
    fn test_display_keeps_cd_when_different_cwd() {
        // cd target differs from cwd - keep full command
        assert_eq!(
            display_command("cd /foo && cargo test", "/bar"),
            "cd /foo && cargo test"
        );
    }

    #[test]
    fn test_display_strips_cd_semicolon_when_matches() {
        // Semicolon separator, cwd matches
        assert_eq!(display_command("cd /foo; npm build", "/foo"), "npm build");
    }

    #[test]
    fn test_display_keeps_cd_semicolon_when_different() {
        // Semicolon separator, cwd differs
        assert_eq!(
            display_command("cd /foo; npm build", "/bar"),
            "cd /foo; npm build"
        );
    }

    #[test]
    fn test_display_strips_chained_cds_when_final_matches() {
        // Multiple cd's with && - strip cd /b when it matches cwd
        // The cd /a; remains because it's semicolon-separated (different semantics)
        // We strip individual cd commands, not cumulative paths
        assert_eq!(
            display_command("cd /a; cd /b && cargo test", "/b"),
            "cd /a; cargo test"
        );
    }

    #[test]
    fn test_display_no_cd_unchanged() {
        // No cd in command - unchanged regardless of cwd
        assert_eq!(
            display_command("cargo test --release", "/any"),
            "cargo test --release"
        );
    }

    #[test]
    fn test_display_cd_only_unchanged() {
        // cd with no follow-up command - show the whole thing
        assert_eq!(display_command("cd /foo", "/foo"), "cd /foo");
    }

    #[test]
    fn test_display_complex_chain_matches() {
        // cd /path && actual && more -> strips cd prefix when cwd matches
        assert_eq!(
            display_command("cd /path && echo hello && ls -la", "/path"),
            "echo hello && ls -la"
        );
    }

    #[test]
    fn test_display_complex_chain_different() {
        // cd /path && actual && more -> keeps full when cwd differs
        assert_eq!(
            display_command("cd /path && echo hello && ls -la", "/other"),
            "cd /path && echo hello && ls -la"
        );
    }

    #[test]
    fn test_display_with_pipes_matches() {
        // Note: brush-parser's Display impl doesn't add space around pipe
        assert_eq!(
            display_command("cd /foo && cat file.txt | grep pattern", "/foo"),
            "cat file.txt |grep pattern"
        );
    }

    #[test]
    fn test_display_empty() {
        assert_eq!(display_command("", "/any"), "");
    }

    #[test]
    fn test_display_multiple_semicolon_cds_final_matches() {
        // With semicolons, each cd is independent - only cd /c matches cwd /c
        // The earlier cd's don't match so they're preserved
        // Note: in practice, LLM typically uses && not ; for cd chains
        assert_eq!(
            display_command("cd /a; cd /b; cd /c; actual_command arg", "/c"),
            "cd /a; cd /b; actual_command arg"
        );
    }

    #[test]
    fn test_display_multiple_semicolon_cds_final_different() {
        // Keep command when final cd differs from cwd
        assert_eq!(
            display_command("cd /a; cd /b; cd /c; actual_command arg", "/d"),
            "cd /a; cd /b; cd /c; actual_command arg"
        );
    }

    // ==================== || Chain Tests ====================

    #[test]
    fn test_display_or_chain_preserves_full_command() {
        // || chains should never be stripped - the fallback is semantically important
        let result = display_command(r#"cat /path/to/file || echo "FILE NOT FOUND""#, "/any");
        assert!(
            result.contains("cat"),
            "Should preserve primary command 'cat', got: {result}"
        );
        assert!(
            result.contains("||"),
            "Should preserve || operator, got: {result}"
        );
    }

    #[test]
    fn test_display_cd_and_or_chain_strips_cd_when_matches() {
        // cd && (cmd || fallback) → cmd || fallback (strip cd only when cwd matches)
        let result = display_command(r#"cd /app && cat file || echo "not found""#, "/app");
        assert_eq!(result, r#"cat file || echo "not found""#);
    }

    #[test]
    fn test_display_cd_and_or_chain_keeps_when_different() {
        // cd && (cmd || fallback) → keep full command when cwd differs
        let result = display_command(r#"cd /app && cat file || echo "not found""#, "/other");
        assert_eq!(result, r#"cd /app && cat file || echo "not found""#);
    }

    #[test]
    fn test_display_or_only_unchanged() {
        // Pure || chain without cd should be unchanged
        let result = display_command("command1 || command2 || command3", "/any");
        assert_eq!(result, "command1 || command2 || command3");
    }

    // ==================== Path Matching Tests ====================

    #[test]
    fn test_display_trailing_slash_matches() {
        // Trailing slashes should be normalized
        assert_eq!(
            display_command("cd /foo/ && cargo test", "/foo"),
            "cargo test"
        );
    }

    #[test]
    fn test_display_relative_path_matches() {
        // Relative cd path resolved against cwd should match
        // cd . && command with cwd /foo should strip the cd
        assert_eq!(display_command("cd . && cargo test", "/tmp"), "cargo test");
    }
}
