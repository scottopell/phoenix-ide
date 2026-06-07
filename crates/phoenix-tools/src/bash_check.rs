//! Bash command safety checks
//!
//! UX layer to catch common LLM mistakes before execution.
//! NOT a security boundary - just helpful guardrails.

use brush_parser::ast::{
    AndOr, AndOrList, Command, CommandPrefixOrSuffixItem, CompoundCommand, CompoundList, Pipeline,
    SimpleCommand,
};
use brush_parser::{Parser, ParserOptions};
use std::io::Cursor;

/// Display-side simplification of bash scripts (strips boilerplate `cd`
/// prefixes for UI). Re-exported so existing `bash_check::display_command`
/// call sites keep resolving; the implementation lives in the
/// `phoenix-bash-display` leaf crate.
pub use phoenix_bash_display::display_command;

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
///
/// # Errors
/// Returns [`CheckError`] with a human-readable message when the script
/// matches a blocked pattern.
pub fn check(script: &str) -> Result<(), CheckError> {
    let cursor = Cursor::new(script);
    let mut parser = Parser::new(cursor, &ParserOptions::default());
    let program = parser.parse_program().map_err(|_| CheckError {
        message: "Failed to parse script".into(),
    })?;

    for complete_cmd in &program.complete_commands {
        check_compound_list(complete_cmd)?;
    }
    Ok(())
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
        Command::ExtendedTest(_, _) => Ok(()), // [[ ... ]] doesn't execute commands
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
        // Coprocess runs a single command asynchronously in a subshell.
        // Check that command's body the same as a simple subshell.
        CompoundCommand::Coprocess(coproc) => check_command(&coproc.body),
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
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Known-blocked command cores. Each, on its own, makes `check` return Err.
    const BLOCKED_CORES: &[&str] = &[
        "git add -A",
        "git add --all",
        "git add .",
        "git add *",
        "git push --force",
        "git push -f",
        "rm -rf /",
        "rm -rf ~",
        "rm -rf .git",
        "rm -rf *",
    ];

    /// Generate runs of horizontal whitespace (spaces/tabs), 1..=4 chars.
    fn arb_ws() -> impl Strategy<Value = String> {
        "[ \t]{1,4}"
    }

    fn arb_blocked_core() -> impl Strategy<Value = &'static str> {
        prop::sample::select(BLOCKED_CORES)
    }

    proptest! {
        // Property 1: WHITESPACE INVARIANCE — arbitrary horizontal whitespace
        // between tokens of a blocked command keeps it blocked.

        #[test]
        fn prop_git_add_whitespace_invariant(ws1 in arb_ws(), ws2 in arb_ws()) {
            let script = format!("git{ws1}add{ws2}-A");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_git_push_whitespace_invariant(ws1 in arb_ws(), ws2 in arb_ws()) {
            let script = format!("git{ws1}push{ws2}--force");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_rm_whitespace_invariant(ws1 in arb_ws(), ws2 in arb_ws()) {
            let script = format!("rm{ws1}-rf{ws2}/");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        // Property 2: SUDO PREFIX — prepending `sudo ` keeps a blocked command blocked.

        #[test]
        fn prop_sudo_prefix_keeps_blocked(core in arb_blocked_core()) {
            let script = format!("sudo {core}");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        // Property 3: CONTEXT-POSITION INVARIANCE — wrapping a blocked core in
        // benign sequencing / pipeline / subshell / if-body keeps it blocked.

        #[test]
        fn prop_context_and(core in arb_blocked_core()) {
            let script = format!("echo ok && {core}");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_context_seq(core in arb_blocked_core()) {
            let script = format!("echo ok ; {core}");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_context_pipe(core in arb_blocked_core()) {
            let script = format!("{core} | cat");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_context_or(core in arb_blocked_core()) {
            let script = format!("echo ok || {core}");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_context_subshell(core in arb_blocked_core()) {
            let script = format!("( {core} )");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        #[test]
        fn prop_context_if_body(core in arb_blocked_core()) {
            let script = format!("if true; then {core}; fi");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        // Property 4: GIT-ADD BAD-ARG ENUMERATION — each bad arg blocks, even
        // with benign explicit filenames interspersed.

        #[test]
        fn prop_git_add_bad_arg_with_benign(
            bad in prop::sample::select(&["-A", "--all", ".", "*"][..]),
            lead in "[a-z]{1,10}",
            trail in "[a-z]{1,10}",
        ) {
            let script = format!("git add {lead} {bad} {trail}");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        // Property 5: RM FLAG-PERMUTATION INVARIANCE — any spelling of
        // recursive+force against a dangerous path blocks.

        #[test]
        fn prop_rm_flag_permutation(
            flags in prop::sample::select(
                &["-rf", "-fr", "-r -f", "-f -r", "--recursive --force", "-Rf"][..]
            ),
            path in prop::sample::select(&["/", ".git"][..]),
        ) {
            let script = format!("rm {flags} {path}");
            prop_assert!(check(&script).is_err(), "should block: {:?}", script);
        }

        // Property 6: TRUE-NEGATIVE / NO OVER-BLOCK — allowed commands stay
        // allowed, including under benign wrapping.

        #[test]
        fn prop_force_with_lease_allowed(suffix in prop::sample::select(&["", "=origin/main"][..])) {
            let script = format!("git push --force-with-lease{suffix}");
            prop_assert!(check(&script).is_ok(), "should allow: {:?}", script);
            let wrapped = format!("echo ok && {script}");
            prop_assert!(check(&wrapped).is_ok(), "should allow: {:?}", wrapped);
        }

        #[test]
        fn prop_git_add_explicit_file_allowed(name in "[a-z]{1,10}") {
            let script = format!("git add {name}");
            prop_assert!(check(&script).is_ok(), "should allow: {:?}", script);
        }
    }
}
