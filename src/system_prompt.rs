//! System prompt construction with AGENTS.md discovery
//!
//! Discovers and loads guidance files (AGENTS.md, AGENT.md) from the working
//! directory up to the filesystem root, combining them into a system prompt.

use std::fmt::Write;
use std::path::{Path, PathBuf};

/// Names of guidance files to look for, in order of preference
const GUIDANCE_FILE_NAMES: &[&str] = &["AGENTS.md", "AGENT.md"];

/// Base system prompt establishing the agent's role
const BASE_PROMPT: &str = r"You are a helpful AI assistant with access to tools for executing code, editing files, and searching codebases. Use tools when appropriate to accomplish tasks.

Be concise in your responses. When using tools, explain what you're doing briefly.";

/// Suffix added for sub-agent conversations
const SUB_AGENT_SUFFIX: &str = r"

You are a sub-agent working on a specific task. When you complete your task, call submit_result with your findings. If you encounter an unrecoverable error, call submit_error. Your conversation will end after calling either tool.";

/// A discovered guidance file with its path and content
#[derive(Debug, Clone)]
pub struct GuidanceFile {
    pub path: PathBuf,
    pub content: String,
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
}
