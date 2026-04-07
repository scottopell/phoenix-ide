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
///
/// TODO(REQ-SK-005): For full convergence with the user `/skill` path, this
/// tool should produce a `MessageContent::Skill` (injected via state machine
/// interception, like `ask_user_question`) rather than a plain `ToolOutput`.
/// Currently the *content* is identical thanks to shared `invoke_skill()`, but
/// the delivery mechanism differs (tool result vs. user message).
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

        let skills = crate::system_prompt::discover_skills(&ctx.working_dir);
        match crate::skills::invoke_skill(skill_name, args, &skills) {
            Ok(invocation) => ToolOutput::success(invocation.body),
            Err(e) => ToolOutput::error(e),
        }
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

    // -- Input validation (tool-level concerns) --

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

    // -- Delegation to invoke_skill (smoke tests through the tool interface) --

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
        assert!(result.output.contains("build"));
    }

    #[tokio::test]
    async fn test_skill_found_returns_body() {
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
        // invoke_skill strips frontmatter and prepends base directory
        assert!(result.output.contains("Run cargo build."));
        assert!(result.output.contains("Base directory for this skill:"));
        assert!(!result.output.contains("---"));
    }

    #[tokio::test]
    async fn test_skill_with_args_substituted() {
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
}
