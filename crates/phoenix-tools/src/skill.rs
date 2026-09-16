//! Skill tool — LLM-invoked skill execution
//!
//! Allows the LLM to programmatically invoke project-level or user-level skills
//! discovered from `.claude/skills/` and `.agents/skills/` directories.
//! Unlike the user-facing `/skill` prefix (which expands before sending to the LLM),
//! this tool is called BY the LLM when it decides a skill would help.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub(super) struct AuthenticatedBuiltin;

fn trusted_builtin_instructions(output: String) -> ToolOutput {
    ToolOutput::TrustedInstructions {
        output,
        _authority: AuthenticatedBuiltin,
    }
}

/// Tool that lets the LLM invoke a discovered skill by name.
///
/// Delivers the skill body as a tool result (not a user-role message).
/// This is intentional: the LLM calling this tool mid-task is fetching
/// instructions autonomously; tool result is the correct delivery weight.
/// The user `/skill` slash path delivers as a user-role message because
/// the user is issuing a directive. See REQ-SK-002 in specs/skills/.
pub struct SkillTool {
    audience: phoenix_skills::SkillAudience,
    builtin_dir: Option<std::path::PathBuf>,
}

impl Default for SkillTool {
    fn default() -> Self {
        Self {
            audience: phoenix_skills::SkillAudience::Conversation,
            builtin_dir: None,
        }
    }
}

impl SkillTool {
    #[must_use]
    pub const fn for_global_coordinator() -> Self {
        Self {
            audience: phoenix_skills::SkillAudience::GlobalCoordinator,
            builtin_dir: None,
        }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "skill"
    }

    fn description(&self) -> String {
        match self.audience {
            phoenix_skills::SkillAudience::Conversation => "Invoke a project or user skill by name. Use this when a skill would help accomplish the current task.".to_string(),
            phoenix_skills::SkillAudience::GlobalCoordinator => "Invoke an authenticated built-in Coordinator skill by name. Available skills are listed in the system prompt.".to_string(),
        }
    }

    fn input_schema(&self) -> Value {
        let skill_name = json!({
            "type": "string",
            "description": "Name of an available skill"
        });
        match self.audience {
            phoenix_skills::SkillAudience::Conversation => json!({
                "type": "object", "required": ["skill_name"],
                "properties": {"skill_name": skill_name, "args": {"type": "string", "description": "Optional arguments to pass to the skill"}}
            }),
            phoenix_skills::SkillAudience::GlobalCoordinator => json!({
                "type": "object", "required": ["skill_name"],
                "properties": {"skill_name": skill_name}
            }),
        }
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
        if self.audience == phoenix_skills::SkillAudience::GlobalCoordinator && !args.is_empty() {
            return ToolOutput::error("authenticated built-in skills do not accept arguments");
        }

        let skills = match self.audience {
            phoenix_skills::SkillAudience::Conversation => {
                phoenix_skills::discover_skills_for_audience(
                    ctx.working_dir(),
                    phoenix_skills::SkillAudience::Conversation,
                )
            }
            phoenix_skills::SkillAudience::GlobalCoordinator => {
                let default_dir = phoenix_skills::builtin::default_extract_dir();
                let builtin_dir = self.builtin_dir.as_deref().or(default_dir.as_deref());
                phoenix_skills::discover_builtin_skills_for_audience(
                    builtin_dir,
                    phoenix_skills::SkillAudience::GlobalCoordinator,
                )
            }
        };
        let result = match self.audience {
            phoenix_skills::SkillAudience::Conversation => {
                phoenix_skills::invoke_skill(skill_name, args, self.audience, &skills)
                    .map(|invocation| invocation.body)
            }
            phoenix_skills::SkillAudience::GlobalCoordinator => {
                phoenix_skills::invoke_trusted_coordinator_builtin(skill_name, &skills)
            }
        };
        match (self.audience, result) {
            (phoenix_skills::SkillAudience::GlobalCoordinator, Ok(body)) => {
                trusted_builtin_instructions(body)
            }
            (phoenix_skills::SkillAudience::Conversation, Ok(body)) => ToolOutput::success(body),
            (_, Err(e)) => ToolOutput::error(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserSessionManager;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn test_context(working_dir: std::path::PathBuf) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
            None,
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
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
    async fn coordinator_skill_returns_authenticated_embedded_instructions() {
        let temp = TempDir::new().unwrap();
        phoenix_skills::builtin::extract_to(temp.path()).unwrap();
        let tool = SkillTool {
            audience: phoenix_skills::SkillAudience::GlobalCoordinator,
            builtin_dir: Some(temp.path().to_path_buf()),
        };
        let result = tool
            .run(
                json!({"skill_name": "phoenix-api"}),
                test_context(temp.path().to_path_buf()),
            )
            .await;

        assert!(result.is_success());
        assert!(result.output().contains("<trusted_builtin_skill"));
        assert!(result.output().contains("Embedded reference"));
        assert!(result.output().contains("ContextExhausted"));
    }

    #[tokio::test]
    async fn coordinator_skill_rejects_arguments_before_authenticated_invocation() {
        let temp = TempDir::new().unwrap();
        phoenix_skills::builtin::extract_to(temp.path()).unwrap();
        let tool = SkillTool {
            audience: phoenix_skills::SkillAudience::GlobalCoordinator,
            builtin_dir: Some(temp.path().to_path_buf()),
        };

        let result = tool
            .run(
                json!({"skill_name": "phoenix-api", "args": "untrusted input"}),
                test_context(temp.path().to_path_buf()),
            )
            .await;

        assert!(!result.is_success());
        assert!(result.output().contains("do not accept arguments"));
    }

    #[test]
    fn coordinator_skill_schema_omits_arguments_and_filesystem_claims() {
        let tool = SkillTool::for_global_coordinator();
        assert!(tool.input_schema()["properties"].get("args").is_none());
        assert!(!tool.description().contains(".claude/skills"));
    }

    #[tokio::test]
    async fn test_skill_empty_name() {
        let tmp = TempDir::new().unwrap();
        let tool = SkillTool::default();
        let result = tool
            .run(
                json!({"skill_name": ""}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(!result.is_success());
        assert!(result.output().contains("skill_name is required"));
    }

    #[tokio::test]
    async fn test_skill_missing_name() {
        let tmp = TempDir::new().unwrap();
        let tool = SkillTool::default();
        let result = tool
            .run(json!({}), test_context(tmp.path().to_path_buf()))
            .await;
        assert!(!result.is_success());
        assert!(result.output().contains("skill_name is required"));
    }

    // -- Delegation to invoke_skill (smoke tests through the tool interface) --

    #[tokio::test]
    async fn test_skill_not_found() {
        let tmp = TempDir::new().unwrap();
        let tool = SkillTool::default();
        let result = tool
            .run(
                json!({"skill_name": "nonexistent"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(!result.is_success());
        assert!(result.output().contains("not found"));
    }

    #[tokio::test]
    async fn test_skill_not_found_lists_available() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "build", "build", "Build stuff", "Build body.");

        let tool = SkillTool::default();
        let result = tool
            .run(
                json!({"skill_name": "deploy"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(!result.is_success());
        assert!(result.output().contains("build"));
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

        let tool = SkillTool::default();
        let result = tool
            .run(
                json!({"skill_name": "build"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(result.is_success());
        // invoke_skill strips frontmatter and prepends base directory
        assert!(result.output().contains("Run cargo build."));
        assert!(result.output().contains("Base directory for this skill:"));
        assert!(!result.output().contains("---"));
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

        let tool = SkillTool::default();
        let result = tool
            .run(
                json!({"skill_name": "review", "args": "src/main.rs"}),
                test_context(tmp.path().to_path_buf()),
            )
            .await;
        assert!(result.is_success());
        assert!(result
            .output()
            .contains("Please review src/main.rs carefully."));
    }
}
