//! System prompt construction with AGENTS.md discovery and skill catalog injection
//!
//! Discovers and loads guidance files (AGENTS.md, AGENT.md) from the working
//! directory up to the filesystem root, combining them into a system prompt.
//! Also scans for skill directories (any directory containing SKILL.md) and
//! injects a metadata catalog so the agent knows which skills are available.

use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// Names of guidance files to look for, in order of preference
const GUIDANCE_FILE_NAMES: &[&str] = &["AGENTS.md", "AGENT.md"];

/// Base system prompt establishing the agent's role
const BASE_PROMPT: &str = r"You are a helpful AI assistant with access to tools for executing code, editing files, and searching codebases. Use tools when appropriate to accomplish tasks.

Be concise in your responses. When using tools, explain what you're doing briefly.

You have access to browser automation tools (browser_navigate, browser_eval, browser_take_screenshot, browser_resize, browser_recent_console_logs, browser_clear_console_logs) for web interaction. Use these tools instead of bash commands when you need to:
- Navigate to web pages and interact with them
- Execute JavaScript in a page context
- Take screenshots of web content
- Monitor browser console output

The browser maintains a persistent session per conversation.";

/// Suffix added for sub-agent conversations
const SUB_AGENT_SUFFIX: &str = r"

You are a sub-agent working on a specific task. When you complete your task, call submit_result with your findings. If you encounter an unrecoverable error, call submit_error. Your conversation will end after calling either tool.";

/// A discovered guidance file with its path and content
#[derive(Debug, Clone)]
pub struct GuidanceFile {
    pub path: PathBuf,
    pub content: String,
}

/// Metadata extracted from a skill's SKILL.md frontmatter
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// Parse `name` and `description` from SKILL.md YAML frontmatter.
///
/// Expects the file to start with `---\n`, followed by `key: value` lines,
/// closed by `\n---\n`. Returns `None` if either field is missing or the
/// frontmatter is malformed.
fn parse_skill_frontmatter(content: &str) -> Option<(String, String)> {
    let body = content.strip_prefix("---\n")?;
    let end = body.find("\n---\n").or_else(|| {
        // Handle frontmatter at end of file with no trailing newline after ---
        body.find("\n---").filter(|&i| i + 4 == body.len())
    })?;
    let frontmatter = &body[..end];

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;

    for line in frontmatter.lines() {
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        }
    }

    Some((name?, description?))
}

/// Discover skills by walking from `working_dir` up to the filesystem root.
///
/// At each level, scans all immediate subdirectories for a `SKILL.md` file.
/// Any directory containing `SKILL.md` is treated as a skill — no constraint
/// on the containing directory name (e.g. `skills/`, `.claude/skills/`, etc.)
///
/// When the same skill name appears at multiple levels, the one closer to
/// `working_dir` wins (more specific overrides parent).
///
/// Returns skills sorted by name for deterministic output.
pub fn discover_skills(working_dir: &Path) -> Vec<SkillMetadata> {
    let mut skills: Vec<SkillMetadata> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut current = Some(working_dir.to_path_buf());

    while let Some(dir) = current {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let skill_md = entry.path().join("SKILL.md");
                if !skill_md.is_file() {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&skill_md) {
                    if let Some((name, description)) = parse_skill_frontmatter(&content) {
                        // cwd wins: only add if we haven't seen this name yet
                        if seen_names.insert(name.clone()) {
                            skills.push(SkillMetadata {
                                name,
                                description,
                                path: skill_md,
                            });
                        }
                    }
                }
            }
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Discover guidance files from the working directory up to the root.
/// Returns files in order from root to cwd (more specific files last).
pub fn discover_guidance_files(working_dir: &Path) -> Vec<GuidanceFile> {
    let mut files = Vec::new();
    let mut current = Some(working_dir.to_path_buf());

    // Walk up the directory tree
    while let Some(dir) = current {
        for name in GUIDANCE_FILE_NAMES {
            let path = dir.join(name);
            if path.is_file() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    files.push(GuidanceFile {
                        path: path.clone(),
                        content,
                    });
                    // Only use one guidance file per directory (first match wins)
                    break;
                }
            }
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    // Reverse so root files come first, cwd files last (more specific override)
    files.reverse();
    files
}

/// Build the complete system prompt for a conversation.
pub fn build_system_prompt(working_dir: &Path, is_sub_agent: bool) -> String {
    let mut prompt = String::from(BASE_PROMPT);

    // Add guidance from discovered files
    let guidance_files = discover_guidance_files(working_dir);
    if !guidance_files.is_empty() {
        prompt.push_str("\n\n<project_guidance>\n");

        for (i, file) in guidance_files.iter().enumerate() {
            if i > 0 {
                prompt.push_str("\n---\n\n");
            }
            // Include the relative path for context
            let display_path = file.path.display();
            let _ = writeln!(prompt, "<!-- From: {display_path} -->");
            prompt.push_str(&file.content);
            if !file.content.ends_with('\n') {
                prompt.push('\n');
            }
        }

        prompt.push_str("</project_guidance>");
    }

    // Inject skill catalog (metadata only — full instructions loaded on demand via bash)
    let skills = discover_skills(working_dir);
    if !skills.is_empty() {
        prompt.push_str("\n\n<available_skills>\n");
        prompt.push_str("The following skills are available. To use a skill, read its SKILL.md file using bash (e.g. `cat /path/to/SKILL.md`) to load full instructions.\n");
        for skill in &skills {
            let path = skill.path.display();
            let _ = writeln!(
                prompt,
                "\n- **{}** — {} (`{path}`)",
                skill.name, skill.description
            );
        }
        prompt.push_str("</available_skills>");
    }

    // Add sub-agent suffix if applicable
    if is_sub_agent {
        prompt.push_str(SUB_AGENT_SUFFIX);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_discover_no_files() {
        let temp = TempDir::new().unwrap();
        let files = discover_guidance_files(temp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn test_discover_single_file() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        fs::write(&agents_path, "# Test guidance").unwrap();

        let files = discover_guidance_files(temp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "# Test guidance");
    }

    #[test]
    fn test_agents_md_preferred_over_agent_md() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "agents content").unwrap();
        fs::write(temp.path().join("AGENT.md"), "agent content").unwrap();

        let files = discover_guidance_files(temp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "agents content");
    }

    #[test]
    fn test_discover_nested_files() {
        let temp = TempDir::new().unwrap();
        let subdir = temp.path().join("project");
        fs::create_dir(&subdir).unwrap();

        fs::write(temp.path().join("AGENTS.md"), "root guidance").unwrap();
        fs::write(subdir.join("AGENTS.md"), "project guidance").unwrap();

        let files = discover_guidance_files(&subdir);
        assert_eq!(files.len(), 2);
        // Root comes first
        assert_eq!(files[0].content, "root guidance");
        // Project-specific comes last (higher precedence)
        assert_eq!(files[1].content, "project guidance");
    }

    #[test]
    fn test_build_system_prompt_no_guidance() {
        let temp = TempDir::new().unwrap();
        let prompt = build_system_prompt(temp.path(), false);

        assert!(prompt.contains("helpful AI assistant"));
        assert!(!prompt.contains("<project_guidance>"));
        assert!(!prompt.contains("sub-agent"));
    }

    #[test]
    fn test_build_system_prompt_with_guidance() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "# Project Rules\nBe nice.").unwrap();

        let prompt = build_system_prompt(temp.path(), false);

        assert!(prompt.contains("<project_guidance>"));
        assert!(prompt.contains("# Project Rules"));
        assert!(prompt.contains("Be nice."));
        assert!(prompt.contains("</project_guidance>"));
    }

    #[test]
    fn test_build_system_prompt_sub_agent() {
        let temp = TempDir::new().unwrap();
        let prompt = build_system_prompt(temp.path(), true);

        assert!(prompt.contains("sub-agent"));
        assert!(prompt.contains("submit_result"));
    }

    // -------------------------------------------------------------------------
    // Skill discovery tests
    // -------------------------------------------------------------------------

    fn write_skill(dir: &Path, skill_dir_name: &str, name: &str, description: &str) {
        let skill_dir = dir.join(skill_dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .unwrap();
    }

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = "---\nname: my-skill\ndescription: Does something useful\n---\n\n# Body\n";
        let result = parse_skill_frontmatter(content);
        assert_eq!(
            result,
            Some(("my-skill".to_string(), "Does something useful".to_string()))
        );
    }

    #[test]
    fn test_parse_frontmatter_missing_name() {
        let content = "---\ndescription: Does something useful\n---\n\n# Body\n";
        assert_eq!(parse_skill_frontmatter(content), None);
    }

    #[test]
    fn test_parse_frontmatter_missing_description() {
        let content = "---\nname: my-skill\n---\n\n# Body\n";
        assert_eq!(parse_skill_frontmatter(content), None);
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# Just a markdown file\nNo frontmatter here.\n";
        assert_eq!(parse_skill_frontmatter(content), None);
    }

    #[test]
    fn test_discover_skills_none() {
        let temp = TempDir::new().unwrap();
        let skills = discover_skills(temp.path());
        assert!(skills.is_empty());
    }

    #[test]
    fn test_discover_skills_found() {
        let temp = TempDir::new().unwrap();
        write_skill(
            temp.path(),
            "my-skill",
            "my-skill",
            "Does something useful. Use when you need something.",
        );

        let skills = discover_skills(temp.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert!(skills[0].description.contains("Does something useful"));
        assert_eq!(
            skills[0].path,
            temp.path().join("my-skill").join("SKILL.md")
        );
    }

    #[test]
    fn test_discover_skills_sorted_by_name() {
        let temp = TempDir::new().unwrap();
        write_skill(temp.path(), "zzz-skill", "zzz-skill", "Last alphabetically");
        write_skill(
            temp.path(),
            "aaa-skill",
            "aaa-skill",
            "First alphabetically",
        );

        let skills = discover_skills(temp.path());
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "aaa-skill");
        assert_eq!(skills[1].name, "zzz-skill");
    }

    #[test]
    fn test_discover_skills_dedup_cwd_wins() {
        let temp = TempDir::new().unwrap();
        let child = temp.path().join("project");
        fs::create_dir(&child).unwrap();

        // Parent has skill with one description
        write_skill(
            temp.path(),
            "shared-skill",
            "shared-skill",
            "Parent description",
        );
        // Child has same skill name with different description
        write_skill(&child, "shared-skill", "shared-skill", "Child description");

        // Discover from child — child should win
        let skills = discover_skills(&child);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "Child description");
    }

    #[test]
    fn test_discover_skills_no_constraint_on_dir_name() {
        let temp = TempDir::new().unwrap();
        // Skill not under a "skills/" directory — any subdir with SKILL.md counts
        write_skill(
            temp.path(),
            "custom-dir-name",
            "custom-skill",
            "Uses a non-standard directory name",
        );

        let skills = discover_skills(temp.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "custom-skill");
    }

    #[test]
    fn test_build_system_prompt_with_skills() {
        let temp = TempDir::new().unwrap();
        write_skill(
            temp.path(),
            "deploy-skill",
            "deploy-skill",
            "Deploy the app. Use when deploying.",
        );

        let prompt = build_system_prompt(temp.path(), false);

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("</available_skills>"));
        assert!(prompt.contains("**deploy-skill**"));
        assert!(prompt.contains("Deploy the app"));
        assert!(prompt.contains("SKILL.md"));
    }

    #[test]
    fn test_build_system_prompt_no_skills() {
        let temp = TempDir::new().unwrap();
        let prompt = build_system_prompt(temp.path(), false);

        assert!(!prompt.contains("<available_skills>"));
    }
}
