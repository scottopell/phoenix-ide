//! Bash command safety checks
//!
//! UX layer to catch common LLM mistakes before execution.
//! NOT a security boundary - just helpful guardrails.

use tree_sitter::{Node, Parser};

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
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|e| CheckError {
            message: format!("Failed to set parser language: {e}"),
        })?;

    let tree = parser.parse(script, None).ok_or_else(|| CheckError {
        message: "Failed to parse script".into(),
    })?;

    check_node(tree.root_node(), script.as_bytes())
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
}
