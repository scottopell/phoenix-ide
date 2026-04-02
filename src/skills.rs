//! Shared skill invocation logic (REQ-SK-001 through REQ-SK-005)
//!
//! Both the user `/skill` path (`message_expander`) and the LLM Skill tool
//! (`tools/skill.rs`) call `invoke_skill()` to produce identical output.

use crate::system_prompt::discover_skills;
use std::path::Path;

/// The result of invoking a skill.
#[derive(Debug, Clone)]
pub struct SkillInvocation {
    /// The skill name (e.g., "build")
    pub name: String,
    /// The fully expanded skill body: frontmatter stripped, base directory
    /// prepended, arguments substituted (REQ-SK-001, REQ-SK-003, REQ-SK-004)
    pub body: String,
    /// Absolute path to the skill's directory
    pub skill_dir: String,
}

/// Invoke a skill by name: discover, read SKILL.md, strip frontmatter,
/// prepend base directory, substitute arguments.
///
/// # Errors
///
/// Returns `Err` if the skill is not found or cannot be read from disk.
pub fn invoke_skill(
    skill_name: &str,
    arguments: &str,
    working_dir: &Path,
) -> Result<SkillInvocation, String> {
    let skills = discover_skills(working_dir);

    let skill = skills
        .iter()
        .find(|s| s.name == skill_name)
        .ok_or_else(|| {
            let available: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
            format!(
                "Skill '{}' not found. Available: {}",
                skill_name,
                if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;

    let raw_content = std::fs::read_to_string(&skill.path)
        .map_err(|e| format!("Failed to read skill '{skill_name}': {e}"))?;

    // REQ-SK-001: Strip YAML frontmatter
    let body = strip_frontmatter(&raw_content);

    // REQ-SK-003: Prepend base directory
    let skill_dir = skill
        .path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let body_with_dir = format!("Base directory for this skill: {skill_dir}\n\n{body}");

    // REQ-SK-004: Argument substitution
    let final_body = substitute_arguments(&body_with_dir, arguments);

    Ok(SkillInvocation {
        name: skill_name.to_string(),
        body: final_body,
        skill_dir,
    })
}

/// Strip YAML frontmatter (--- delimited block at the top of the file).
/// Returns the body content after the closing ---.
fn strip_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    // Safety: we checked that `trimmed` starts with "---" (3 bytes), so
    // slicing at byte offset 3 is a valid UTF-8 boundary.
    #[allow(clippy::string_slice)]
    let after_open = &trimmed[3..];
    if let Some(end_pos) = after_open.find("\n---") {
        // Skip past the closing "---" and any trailing newline
        // Safety: `end_pos` is from `find()` on `after_open`; adding 4
        // accounts for "\n---" (4 bytes). The result is a valid UTF-8 boundary.
        #[allow(clippy::string_slice)]
        let body_start_str = &after_open[end_pos + 4..];
        body_start_str.trim_start_matches('\n').to_string()
    } else {
        content.to_string()
    }
}

/// Substitute argument placeholders in the skill body.
/// Order: `$ARGUMENTS[N]` and `$N` first (to prevent `$ARGUMENTS` from
/// corrupting them), then `$ARGUMENTS`. If no placeholder exists, append
/// arguments.
fn substitute_arguments(body: &str, arguments: &str) -> String {
    if arguments.is_empty() {
        return body.to_string();
    }

    if body.contains("$ARGUMENTS") {
        let tokens: Vec<&str> = arguments.split_whitespace().collect();
        let mut result = body.to_string();

        // Positional first (prevents $ARGUMENTS from corrupting $ARGUMENTS[N])
        for (i, token) in tokens.iter().enumerate() {
            let n = i + 1;
            result = result
                .replace(&format!("$ARGUMENTS[{n}]"), token)
                .replace(&format!("${n}"), token);
        }

        // Then the full $ARGUMENTS
        result = result.replace("$ARGUMENTS", arguments);

        result
    } else {
        format!("{body}\nARGUMENTS: {arguments}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -------------------------------------------------------------------------
    // strip_frontmatter
    // -------------------------------------------------------------------------

    #[test]
    fn test_strip_frontmatter_valid() {
        let content = "---\nname: build\ndescription: Build it\n---\n\n# Build\nRun cargo build.";
        let result = strip_frontmatter(content);
        assert_eq!(result, "# Build\nRun cargo build.");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let content = "# Just markdown\nNo frontmatter here.";
        let result = strip_frontmatter(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_strip_frontmatter_incomplete() {
        // Opening --- but no closing ---
        let content = "---\nname: build\ndescription: Build it\n\n# Body";
        let result = strip_frontmatter(content);
        // Should return original content since frontmatter is incomplete
        assert_eq!(result, content);
    }

    #[test]
    fn test_strip_frontmatter_empty_body() {
        let content = "---\nname: build\ndescription: Build it\n---\n";
        let result = strip_frontmatter(content);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_frontmatter_with_leading_whitespace() {
        let content = "  ---\nname: build\ndescription: Build it\n---\n\nBody here.";
        let result = strip_frontmatter(content);
        assert_eq!(result, "Body here.");
    }

    // -------------------------------------------------------------------------
    // substitute_arguments
    // -------------------------------------------------------------------------

    #[test]
    fn test_substitute_arguments_full_replacement() {
        let body = "Review $ARGUMENTS carefully.";
        let result = substitute_arguments(body, "src/main.rs");
        assert_eq!(result, "Review src/main.rs carefully.");
    }

    #[test]
    fn test_substitute_arguments_positional() {
        let body = "Build $ARGUMENTS[1] in $ARGUMENTS[2] mode. Full: $ARGUMENTS";
        let result = substitute_arguments(body, "myapp release");
        assert_eq!(result, "Build myapp in release mode. Full: myapp release");
    }

    #[test]
    fn test_substitute_arguments_dollar_n_shorthand() {
        let body = "First: $1, second: $2";
        // $N shorthand requires $ARGUMENTS to be present somewhere for the
        // substitution branch to trigger. Test the full path:
        let body_with_args = "Full: $ARGUMENTS. First: $1, second: $2";
        let result = substitute_arguments(body_with_args, "foo bar");
        assert_eq!(result, "Full: foo bar. First: foo, second: bar");
        // Without $ARGUMENTS, falls through to append mode
        let result2 = substitute_arguments(body, "foo bar");
        assert_eq!(result2, "First: $1, second: $2\nARGUMENTS: foo bar");
    }

    #[test]
    fn test_substitute_arguments_no_placeholder() {
        let body = "Run the build steps.";
        let result = substitute_arguments(body, "staging");
        assert_eq!(result, "Run the build steps.\nARGUMENTS: staging");
    }

    #[test]
    fn test_substitute_arguments_no_args() {
        let body = "Run $ARGUMENTS if provided.";
        let result = substitute_arguments(body, "");
        assert_eq!(result, body);
    }

    // -------------------------------------------------------------------------
    // invoke_skill (integration with temp dir)
    // -------------------------------------------------------------------------

    fn write_skill(dir: &std::path::Path, skill_dir: &str, name: &str, description: &str, body: &str) {
        let skill_path = dir.join(".claude/skills").join(skill_dir);
        fs::create_dir_all(&skill_path).unwrap();
        fs::write(
            skill_path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn test_invoke_skill_found() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "build", "build", "Build the project", "Run cargo build.");

        let result = invoke_skill("build", "", tmp.path()).unwrap();
        assert_eq!(result.name, "build");
        assert!(result.body.contains("Base directory for this skill:"));
        assert!(result.body.contains("Run cargo build."));
        assert!(!result.body.contains("---"));
        assert!(!result.skill_dir.is_empty());
    }

    #[test]
    fn test_invoke_skill_with_arguments() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "deploy",
            "deploy",
            "Deploy the app",
            "Deploy to $ARGUMENTS environment.",
        );

        let result = invoke_skill("deploy", "staging", tmp.path()).unwrap();
        assert!(result.body.contains("Deploy to staging environment."));
    }

    #[test]
    fn test_invoke_skill_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = invoke_skill("nonexistent", "", tmp.path()).unwrap_err();
        assert!(err.contains("not found"));
        assert!(err.contains("nonexistent"));
    }

    #[test]
    fn test_invoke_skill_not_found_lists_available() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "build", "build", "Build it", "body");
        write_skill(tmp.path(), "lint", "lint", "Lint it", "body");

        let err = invoke_skill("deploy", "", tmp.path()).unwrap_err();
        assert!(err.contains("deploy"));
        assert!(err.contains("build"));
        assert!(err.contains("lint"));
    }

    #[test]
    fn test_invoke_skill_no_args_appended_when_no_placeholder() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "simple",
            "simple",
            "Simple skill",
            "Do the thing.",
        );

        let result = invoke_skill("simple", "extra args", tmp.path()).unwrap();
        assert!(result.body.contains("ARGUMENTS: extra args"));
    }
}
