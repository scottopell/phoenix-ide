//! Skill tool — LLM-invoked skill execution
//!
//! Allows the LLM to programmatically invoke project-level or user-level skills
//! discovered from `.claude/skills/` and `.agents/skills/` directories.
//! Unlike the user-facing `/skill` prefix (which expands before sending to the LLM),
//! this tool is called BY the LLM when it decides a skill would help.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool that lets the LLM invoke a discovered skill by name.
pub struct SkillTool;

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "skill"
    }

    fn description(&self) -> String {
        "Invoke a skill by name. Skills are project-specific or user-level \
         capabilities discovered from .claude/skills/ and .agents/skills/ \
         directories. Use this when a skill would help accomplish the current task."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["skill_name"],
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the skill to invoke (e.g., 'build', 'lint', 'deploy')"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments to pass to the skill"
                }
            }
        })
    }

    fn defer_loading(&self) -> bool {
        true
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let skill_name = input
            .get("skill_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let args = input.get("args").and_then(|v| v.as_str()).unwrap_or("");

        if skill_name.is_empty() {
            return ToolOutput::error("skill_name is required");
        }

        // Discover skills from the conversation's working directory
        let skills = crate::system_prompt::discover_skills(&ctx.working_dir);

        let Some(skill) = skills.iter().find(|s| s.name == skill_name) else {
            let available: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
            return ToolOutput::error(format!(
                "Skill '{}' not found. Available skills: {}",
                skill_name,
                if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                }
            ));
        };

        // Read the skill content
        let content = match std::fs::read_to_string(&skill.path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("Failed to read skill: {e}")),
        };

        // Substitute $ARGUMENTS if present
        let expanded = if content.contains("$ARGUMENTS") {
            let mut result = content.clone();

            // Replace positional patterns first ($ARGUMENTS[N] and $N) before
            // the bare $ARGUMENTS, since $ARGUMENTS is a prefix of $ARGUMENTS[N].
            let tokens: Vec<&str> = args.split_whitespace().collect();
            for (i, token) in tokens.iter().enumerate() {
                let n = i + 1;
                result = result
                    .replace(&format!("$ARGUMENTS[{n}]"), token)
                    .replace(&format!("${n}"), token);
            }

            // Now replace bare $ARGUMENTS with the full args string
            result = result.replace("$ARGUMENTS", args);

            result
        } else if !args.is_empty() {
            format!("{content}\nARGUMENTS: {args}")
        } else {
            content
        };

        ToolOutput::success(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::BrowserSessionManager;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: std::path::PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
        )
    }

    fn write_skill(base: &std::path::Path, skill_dir: &str, name: &str, desc: &str, body: &str) {
        let skill_path = base.join(".claude/skills").join(skill_dir);
        std::fs::create_dir_all(&skill_path).unwrap();
        std::fs::write(
            skill_path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n\n{body}"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_skill_not_found() {
        let tmp = TempDir::new().unwrap();
        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": "nonexistent"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("not found"));
        assert!(result.output.contains("Available skills:"));
    }

    #[tokio::test]
    async fn test_skill_not_found_lists_available() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "build", "build", "Build stuff", "Build body.");

        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": "deploy"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("not found"));
        assert!(result.output.contains("build"));
    }

    #[tokio::test]
    async fn test_skill_empty_name() {
        let tmp = TempDir::new().unwrap();
        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": ""}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("skill_name is required"));
    }

    #[tokio::test]
    async fn test_skill_missing_name() {
        let tmp = TempDir::new().unwrap();
        let tool = SkillTool;
        let result = tool
            .run(json!({}), test_context(tmp.path().to_path_buf()))
            .await;
        assert!(!result.success);
        assert!(result.output.contains("skill_name is required"));
    }

    #[tokio::test]
    async fn test_skill_found_no_args() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "build",
            "build",
            "Build the project",
            "Run cargo build.",
        );

        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": "build"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("Run cargo build."));
    }

    #[tokio::test]
    async fn test_skill_with_arguments_placeholder() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "review",
            "review",
            "Review code",
            "Please review $ARGUMENTS carefully.",
        );

        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": "review", "args": "src/main.rs"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result
            .output
            .contains("Please review src/main.rs carefully."));
    }

    #[tokio::test]
    async fn test_skill_with_args_no_placeholder_appends() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "deploy",
            "deploy",
            "Deploy skill",
            "Run the deployment steps.",
        );

        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": "deploy", "args": "staging"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("Run the deployment steps."));
        assert!(result.output.contains("ARGUMENTS: staging"));
    }

    #[tokio::test]
    async fn test_skill_positional_arguments() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "greet",
            "greet",
            "Greet someone",
            "Hello $ARGUMENTS[1], welcome to $ARGUMENTS[2].",
        );

        let tool = SkillTool;
        let result = tool
            .run(
                json!({"skill_name": "greet", "args": "Alice Wonderland"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(result.success);
        assert!(result
            .output
            .contains("Hello Alice, welcome to Wonderland."));
    }
}
