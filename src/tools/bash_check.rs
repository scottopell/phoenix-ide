//! Bash command safety checks
//!
//! UX layer to catch common LLM mistakes before execution.
//! NOT a security boundary - just helpful guardrails.

use brush_parser::ast::{
    AndOr, AndOrList, Command, CommandPrefixOrSuffixItem, CompoundCommand, CompoundList, Pipeline,
    SimpleCommand,
};
use brush_parser::{Parser, ParserOptions, SourceInfo};
use std::io::Cursor;

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

/// Check a bash script for potentially dangerous patterns.
/// Returns Ok(()) if safe to run, Err with helpful message if blocked.
pub fn check(script: &str) -> Result<(), CheckError> {
    let cursor = Cursor::new(script);
    let mut parser = Parser::new(cursor, &ParserOptions::default(), &SourceInfo::default());
    let program = parser.parse_program().map_err(|_| CheckError {
        message: "Failed to parse script".into(),
    })?;

    for complete_cmd in &program.complete_commands {
        check_compound_list(complete_cmd)?;
    }
    Ok(())
}

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
        _ => false,
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

/// Recursively check all nodes in the AST
/// Check a `CompoundList` (sequence of commands separated by ; or &)
fn check_compound_list(list: &CompoundList) -> Result<(), CheckError> {
    for item in &list.0 {
        check_and_or_list(&item.0)?;
    }
    Ok(())
}

/// Check an `AndOrList` (commands connected by && or ||)
fn check_and_or_list(list: &AndOrList) -> Result<(), CheckError> {
    check_pipeline(&list.first)?;
    for and_or in &list.additional {
        match and_or {
            AndOr::And(pipeline) | AndOr::Or(pipeline) => check_pipeline(pipeline)?,
        }
    }
    Ok(())
}

/// Check a Pipeline (commands connected by |)
fn check_pipeline(pipeline: &Pipeline) -> Result<(), CheckError> {
    for cmd in &pipeline.seq {
        check_command(cmd)?;
    }
    Ok(())
}

/// Check a single Command node
fn check_command(cmd: &Command) -> Result<(), CheckError> {
    match cmd {
        Command::Simple(simple) => check_simple_command(simple),
        Command::Compound(compound, _redirects) => check_compound_command(compound),
        Command::Function(func) => check_compound_command(&func.body.0),
        Command::ExtendedTest(_) => Ok(()), // [[ ... ]] doesn't execute commands
    }
}

/// Check a `CompoundCommand` (loops, conditionals, subshells, brace groups)
fn check_compound_command(cmd: &CompoundCommand) -> Result<(), CheckError> {
    match cmd {
        CompoundCommand::BraceGroup(bg) => check_compound_list(&bg.list),
        CompoundCommand::Subshell(sub) => check_compound_list(&sub.list),
        CompoundCommand::ForClause(fc) => check_compound_list(&fc.body.list),
        CompoundCommand::WhileClause(wc) | CompoundCommand::UntilClause(wc) => {
            check_compound_list(&wc.0)?; // condition
            check_compound_list(&wc.1.list) // body
        }
        CompoundCommand::IfClause(ic) => {
            check_compound_list(&ic.condition)?;
            check_compound_list(&ic.then)?;
            if let Some(elses) = &ic.elses {
                for else_clause in elses {
                    if let Some(cond) = &else_clause.condition {
                        check_compound_list(cond)?;
                    }
                    check_compound_list(&else_clause.body)?;
                }
            }
            Ok(())
        }
        CompoundCommand::CaseClause(cc) => {
            for item in &cc.cases {
                if let Some(cmd) = &item.cmd {
                    check_compound_list(cmd)?;
                }
            }
            Ok(())
        }
        CompoundCommand::Arithmetic(_) | CompoundCommand::ArithmeticForClause(_) => Ok(()),
    }
}

/// Check a `SimpleCommand` for dangerous patterns
fn check_simple_command(cmd: &SimpleCommand) -> Result<(), CheckError> {
    let args = collect_simple_command_args(cmd);
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

/// Collect all argument strings from a `SimpleCommand`
fn collect_simple_command_args(cmd: &SimpleCommand) -> Vec<String> {
    let mut args = Vec::new();

    // Command name
    if let Some(word) = &cmd.word_or_name {
        args.push(word.to_string());
    }

    // Command suffix (arguments)
    if let Some(suffix) = &cmd.suffix {
        for item in &suffix.0 {
            if let CommandPrefixOrSuffixItem::Word(word) = item {
                args.push(word.to_string());
            }
            // Skip redirects (CommandPrefixOrSuffixItem::IoRedirect)
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
