//! Bash command safety checks
//!
//! UX layer to catch common LLM mistakes before execution.
//! NOT a security boundary - just helpful guardrails.

use brush_parser::ast::{AndOr, AndOrList, Command, Pipeline, SimpleCommand};
use tree_sitter::{Node, Parser, Tree};

/// Error returned when a command is blocked
#[derive(Debug)]
pub struct CheckError {
    pub message: String,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CheckError {}

/// Parse a bash script into a tree-sitter AST.
fn parse_script(script: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()?;
    parser.parse(script, None)
}

/// Check a bash script for potentially dangerous patterns.
/// Returns Ok(()) if safe to run, Err with helpful message if blocked.
pub fn check(script: &str) -> Result<(), CheckError> {
    let tree = parse_script(script).ok_or_else(|| CheckError {
        message: "Failed to parse script".into(),
    })?;

    check_node(tree.root_node(), script.as_bytes())
}

/// Extract the "interesting" part of a bash command for display purposes.
///
/// LLMs commonly emit commands like `cd /path && actual_command`. For UI display,
/// we want to show `actual_command` since the `cd` prefix is boilerplate.
///
/// This uses brush-parser for semantic awareness of `&&` vs `||` operators:
/// - `&&` means "run next if previous succeeded" - safe to strip cd prefix
/// - `||` means "run next if previous failed" - NOT safe to strip, shows fallback
/// - `;` means "run sequentially" - safe to strip cd prefix
///
/// Examples:
/// - `cd /foo && cargo test` -> `cargo test`
/// - `cd /foo; npm build` -> `npm build`
/// - `cd /a; cd /b && cmd` -> `cmd`
/// - `cargo test` -> `cargo test` (unchanged)
/// - `cd /foo` -> `cd /foo` (no follow-up command)
/// - `cat file || echo "not found"` -> `cat file || echo "not found"` (preserve || chains)
/// - `cd /app && cat file || echo "not found"` -> `cat file || echo "not found"` (strip cd only)
pub fn display_command(script: &str) -> String {
    // Try brush-parser first for semantic awareness
    if let Some(simplified) = simplify_with_brush(script) {
        return simplified;
    }
    // Fallback: return original unchanged
    script.to_string()
}

/// Parse script with brush-parser and simplify by stripping cd prefixes
/// only when semantically safe (followed by && or ;).
fn simplify_with_brush(script: &str) -> Option<String> {
    use brush_parser::ast::SeparatorOperator;
    use brush_parser::{Parser, ParserOptions, SourceInfo};
    use std::io::Cursor;

    let cursor = Cursor::new(script);
    let mut parser = Parser::new(cursor, &ParserOptions::default(), &SourceInfo::default());
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
        let mut i = 0;
        while i < items.len() {
            let item = &items[i];
            let and_or_list = &item.0;
            let separator = &item.1;

            // Check if this entire AndOrList is just a cd command
            if is_cd_and_or_list(and_or_list) {
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
            let simplified = simplify_and_or_list(and_or_list);
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

/// Check if an `AndOrList` is just a simple cd command (no && or || chains)
fn is_cd_and_or_list(list: &AndOrList) -> bool {
    // Must have no additional && or || operations
    if !list.additional.is_empty() {
        return false;
    }
    // The first (and only) pipeline must be a cd command
    is_cd_pipeline(&list.first)
}

/// Simplify an `AndOrList` by stripping leading cd commands that are followed by `&&`.
/// Returns the simplified command string.
fn simplify_and_or_list(list: &AndOrList) -> String {
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
    // BUT keep cd commands that are followed by ||
    let mut result_items: Vec<(Op, &Pipeline)> = Vec::new();

    for (i, (op, pipeline)) in items.iter().enumerate() {
        let is_cd = is_cd_pipeline(pipeline);

        // Check what operator connects this to the NEXT command
        let next_op = items.get(i + 1).map(|(o, _)| *o);

        // cd followed by && (or at start before &&): safe to skip
        // The cd succeeds → next runs (what we want to show)
        // The cd fails → nothing runs anyway
        if is_cd && matches!(next_op, Some(Op::And | Op::Start)) {
            continue;
        }

        // Keep non-cd commands, cd followed by ||, and cd at the end
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

/// Check if a pipeline is a simple `cd` command
fn is_cd_pipeline(pipeline: &Pipeline) -> bool {
    // A cd pipeline should have exactly one command and it should be a simple `cd` command
    if pipeline.seq.len() != 1 {
        return false;
    }

    match &pipeline.seq[0] {
        Command::Simple(simple) => is_cd_simple_command(simple),
        _ => false,
    }
}

/// Check if a `SimpleCommand` is a `cd` command
fn is_cd_simple_command(cmd: &SimpleCommand) -> bool {
    if let Some(word) = &cmd.word_or_name {
        return word.to_string() == "cd";
    }
    false
}

/// Recursively check all nodes in the AST
fn check_node(node: Node, source: &[u8]) -> Result<(), CheckError> {
    // Check if this is a command node
    if node.kind() == "command" {
        check_command(node, source)?;
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        check_node(child, source)?;
    }

    Ok(())
}

/// Check a single command node for dangerous patterns
fn check_command(node: Node, source: &[u8]) -> Result<(), CheckError> {
    let args = collect_command_args(node, source);
    if args.is_empty() {
        return Ok(());
    }

    // Skip 'sudo' prefix if present
    let args = if args.first().is_some_and(|a| a == "sudo") {
        &args[1..]
    } else {
        &args[..]
    };

    if args.is_empty() {
        return Ok(());
    }

    // Run checks based on command name
    match args.first().map(String::as_str) {
        Some("git") => check_git_command(args),
        Some("rm") => check_rm_command(args),
        _ => Ok(()),
    }
}

/// Collect all argument strings from a command node
fn collect_command_args(node: Node, source: &[u8]) -> Vec<String> {
    let mut args = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "command_name" | "word" | "string" | "raw_string" | "concatenation"
            | "simple_expansion" | "expansion" => {
                if let Ok(text) = child.utf8_text(source) {
                    // Strip quotes from strings
                    let text = text.trim_matches('"').trim_matches('\'');
                    args.push(text.to_string());
                }
            }
            _ => {}
        }
    }

    args
}

/// Check git commands for dangerous patterns
fn check_git_command(args: &[String]) -> Result<(), CheckError> {
    if args.len() < 2 {
        return Ok(());
    }

    let subcommand = &args[1];

    match subcommand.as_str() {
        "add" => check_git_add(&args[2..]),
        "push" => check_git_push(&args[2..]),
        _ => Ok(()),
    }
}

/// Block blind git add commands
fn check_git_add(args: &[String]) -> Result<(), CheckError> {
    for arg in args {
        match arg.as_str() {
            "-A" | "--all" | "." | "*" => {
                return Err(CheckError {
                    message: "permission denied: blind git add commands (git add -A, git add ., git add --all, git add *) are not allowed, specify files explicitly".into(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Block git push --force (but allow --force-with-lease)
fn check_git_push(args: &[String]) -> Result<(), CheckError> {
    for arg in args {
        // --force-with-lease is fine, check for it first
        if arg.starts_with("--force-with-lease") {
            continue;
        }
        // Block --force and -f
        if arg == "--force" || arg == "-f" {
            return Err(CheckError {
                message: "permission denied: git push --force is not allowed. Use --force-with-lease for safer force pushes, or push without force".into(),
            });
        }
    }
    Ok(())
}

/// Check rm commands for dangerous patterns
fn check_rm_command(args: &[String]) -> Result<(), CheckError> {
    // Check if -r/-R and -f are both present
    let has_recursive = args.iter().any(|a| {
        a == "-r"
            || a == "-R"
            || a == "--recursive"
            || (a.starts_with('-') && !a.starts_with("--") && (a.contains('r') || a.contains('R')))
    });

    let has_force = args.iter().any(|a| {
        a == "-f"
            || a == "--force"
            || (a.starts_with('-') && !a.starts_with("--") && a.contains('f'))
    });

    // Only check paths if it's rm -rf
    if !has_recursive || !has_force {
        return Ok(());
    }

    // Check each non-flag argument for dangerous patterns
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }

        // Dangerous patterns
        if is_dangerous_rm_path(arg) {
            return Err(CheckError {
                message: "permission denied: this rm command could delete critical data (.git, home directory, or root). Specify the full path explicitly (no wildcards, ~, or $HOME)".into(),
            });
        }
    }

    Ok(())
}

/// Check if a path is dangerous for rm -rf
fn is_dangerous_rm_path(path: &str) -> bool {
    // Root directory
    if path == "/" {
        return true;
    }

    // Home directory patterns
    if path == "~" || path == "~/" || path.starts_with("~/") {
        return true;
    }

    // $HOME variable
    if path == "$HOME" || path.starts_with("$HOME/") || path.starts_with("${HOME}") {
        return true;
    }

    // .git directory
    if path == ".git" || path.ends_with("/.git") {
        return true;
    }

    // Wildcards that could match dangerous things
    if path == "*" || path == "/*" || path == ".*" || path.ends_with("/.*") {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Git Add Tests ====================

    #[test]
    fn test_git_add_specific_file_allowed() {
        assert!(check("git add main.rs").is_ok());
    }

    #[test]
    fn test_git_add_multiple_files_allowed() {
        assert!(check("git add main.rs lib.rs").is_ok());
    }

    #[test]
    fn test_git_add_path_allowed() {
        assert!(check("git add src/main.rs").is_ok());
    }

    #[test]
    fn test_git_add_with_flags_allowed() {
        assert!(check("git add -v main.rs").is_ok());
    }

    #[test]
    fn test_git_add_dash_a_blocked() {
        let err = check("git add -A").unwrap_err();
        assert!(err.message.contains("blind git add"));
    }

    #[test]
    fn test_git_add_all_blocked() {
        let err = check("git add --all").unwrap_err();
        assert!(err.message.contains("blind git add"));
    }

    #[test]
    fn test_git_add_dot_blocked() {
        let err = check("git add .").unwrap_err();
        assert!(err.message.contains("blind git add"));
    }

    #[test]
    fn test_git_add_star_blocked() {
        let err = check("git add *").unwrap_err();
        assert!(err.message.contains("blind git add"));
    }

    #[test]
    fn test_sudo_git_add_blocked() {
        let err = check("sudo git add -A").unwrap_err();
        assert!(err.message.contains("blind git add"));
    }

    #[test]
    fn test_git_add_in_pipeline_blocked() {
        let err = check("echo 'adding' && git add -A && git commit -m 'test'").unwrap_err();
        assert!(err.message.contains("blind git add"));
    }

    // ==================== Git Push Tests ====================

    #[test]
    fn test_git_push_allowed() {
        assert!(check("git push").is_ok());
    }

    #[test]
    fn test_git_push_origin_main_allowed() {
        assert!(check("git push origin main").is_ok());
    }

    #[test]
    fn test_git_push_force_with_lease_allowed() {
        assert!(check("git push --force-with-lease").is_ok());
    }

    #[test]
    fn test_git_push_force_with_lease_origin_allowed() {
        assert!(check("git push --force-with-lease origin main").is_ok());
    }

    #[test]
    fn test_git_push_force_blocked() {
        let err = check("git push --force").unwrap_err();
        assert!(err.message.contains("--force is not allowed"));
    }

    #[test]
    fn test_git_push_f_blocked() {
        let err = check("git push -f").unwrap_err();
        assert!(err.message.contains("--force is not allowed"));
    }

    #[test]
    fn test_git_push_force_origin_blocked() {
        let err = check("git push --force origin main").unwrap_err();
        assert!(err.message.contains("--force is not allowed"));
    }

    #[test]
    fn test_sudo_git_push_force_blocked() {
        let err = check("sudo git push --force").unwrap_err();
        assert!(err.message.contains("--force is not allowed"));
    }

    // ==================== Rm Tests ====================

    #[test]
    fn test_rm_file_allowed() {
        assert!(check("rm file.txt").is_ok());
    }

    #[test]
    fn test_rm_rf_specific_dir_allowed() {
        assert!(check("rm -rf /tmp/build").is_ok());
    }

    #[test]
    fn test_rm_rf_node_modules_allowed() {
        assert!(check("rm -rf node_modules").is_ok());
    }

    #[test]
    fn test_rm_r_without_f_allowed() {
        // rm -r without -f is allowed (will prompt)
        assert!(check("rm -r .git").is_ok());
    }

    #[test]
    fn test_rm_f_without_r_allowed() {
        // rm -f without -r on .git is allowed (can't delete dir)
        assert!(check("rm -f .git").is_ok());
    }

    #[test]
    fn test_rm_rf_root_blocked() {
        let err = check("rm -rf /").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_home_blocked() {
        let err = check("rm -rf ~").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_home_slash_blocked() {
        let err = check("rm -rf ~/").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_home_subdir_blocked() {
        let err = check("rm -rf ~/Documents").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_home_var_blocked() {
        let err = check("rm -rf $HOME").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_git_blocked() {
        let err = check("rm -rf .git").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_path_git_blocked() {
        let err = check("rm -rf /path/to/.git").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_star_blocked() {
        let err = check("rm -rf *").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_dotstar_blocked() {
        let err = check("rm -rf .*").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_combined_flags_rf_blocked() {
        let err = check("rm -rf .git").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_combined_flags_fr_blocked() {
        let err = check("rm -fr .git").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_separate_flags_blocked() {
        let err = check("rm -r -f .git").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_sudo_rm_rf_root_blocked() {
        let err = check("sudo rm -rf /").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    #[test]
    fn test_rm_rf_in_pipeline_blocked() {
        let err = check("echo 'cleaning' && rm -rf .git").unwrap_err();
        assert!(err.message.contains("critical data"));
    }

    // ==================== Other Commands ====================

    #[test]
    fn test_other_commands_allowed() {
        assert!(check("ls -la").is_ok());
        assert!(check("cat file.txt").is_ok());
        assert!(check("echo hello").is_ok());
        assert!(check("ps aux | grep python").is_ok());
    }

    #[test]
    fn test_git_other_commands_allowed() {
        assert!(check("git status").is_ok());
        assert!(check("git commit -m 'test'").is_ok());
        assert!(check("git log --oneline").is_ok());
        assert!(check("git diff").is_ok());
    }

    #[test]
    fn test_complex_script_allowed() {
        assert!(check("cd /tmp && ls -la && echo done").is_ok());
    }

    #[test]
    fn test_empty_script() {
        assert!(check("").is_ok());
    }

    #[test]
    fn test_comment_only() {
        assert!(check("# this is a comment").is_ok());
    }

    // ==================== Display Command Tests ====================

    #[test]
    fn test_display_strips_cd_and() {
        assert_eq!(display_command("cd /foo && cargo test"), "cargo test");
    }

    #[test]
    fn test_display_strips_cd_semicolon() {
        assert_eq!(display_command("cd /foo; npm build"), "npm build");
    }

    #[test]
    fn test_display_strips_chained_cds() {
        assert_eq!(display_command("cd /a; cd /b && cargo test"), "cargo test");
    }

    #[test]
    fn test_display_no_cd_unchanged() {
        assert_eq!(
            display_command("cargo test --release"),
            "cargo test --release"
        );
    }

    #[test]
    fn test_display_cd_only_unchanged() {
        // If there's only a cd with no follow-up, show the whole thing
        assert_eq!(display_command("cd /foo"), "cd /foo");
    }

    #[test]
    fn test_display_complex_chain() {
        // cd /path && actual && more -> strips cd prefix, shows the rest
        assert_eq!(
            display_command("cd /path && echo hello && ls -la"),
            "echo hello && ls -la"
        );
    }

    #[test]
    fn test_display_with_pipes() {
        // Note: brush-parser's Display impl doesn't add space around pipe
        assert_eq!(
            display_command("cd /foo && cat file.txt | grep pattern"),
            "cat file.txt |grep pattern"
        );
    }

    #[test]
    fn test_display_empty() {
        assert_eq!(display_command(""), "");
    }

    #[test]
    fn test_display_multiple_semicolon_cds() {
        assert_eq!(
            display_command("cd /a; cd /b; cd /c; actual_command arg"),
            "actual_command arg"
        );
    }

    // ==================== || Chain Tests (REQ-1) ====================

    #[test]
    fn test_display_or_chain_preserves_primary() {
        // For `cmd1 || cmd2`, the primary command is cmd1
        // We should show the whole thing, not just the fallback
        let result = display_command(r#"cat /path/to/file || echo "FILE NOT FOUND""#);
        assert!(
            result.contains("cat"),
            "Should preserve primary command 'cat', got: {}",
            result
        );
        assert!(
            result.contains("||"),
            "Should preserve || operator, got: {}",
            result
        );
    }

    #[test]
    fn test_display_cd_and_or_chain() {
        // cd && (cmd || fallback) → cmd || fallback (strip cd only)
        let result = display_command(r#"cd /app && cat file || echo "not found""#);
        assert_eq!(result, r#"cat file || echo "not found""#);
    }

    #[test]
    fn test_display_or_only_unchanged() {
        // Pure || chain without cd should be unchanged
        let result = display_command("command1 || command2 || command3");
        assert_eq!(result, "command1 || command2 || command3");
    }
}
