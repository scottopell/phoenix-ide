//! Sub-agent tools - tools for sub-agent lifecycle management
//!
//! - `spawn_agents`: Spawn sub-agents (parent only)
//! - `submit_result`: Submit successful result (sub-agent only)
//! - `submit_error`: Submit error result (sub-agent only)

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use phoenix_agents::AgentDefinition;
use serde::Deserialize;
use serde_json::{json, Value};

/// Tool for sub-agents to submit their final result
pub struct SubmitResultTool;

#[derive(Debug, Deserialize)]
struct SubmitResultInput {
    result: String,
}

#[async_trait]
impl Tool for SubmitResultTool {
    fn name(&self) -> &'static str {
        "submit_result"
    }

    fn description(&self) -> String {
        "Submit your final result to the parent conversation. Call this when you have completed your assigned task. After calling this, your conversation ends.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["result"],
            "properties": {
                "result": {
                    "type": "string",
                    "description": "Your final result, summary, or output"
                }
            }
        })
    }

    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        // Validate input structure
        match serde_json::from_value::<SubmitResultInput>(input) {
            Ok(parsed) => {
                // The actual state transition is handled by the transition function,
                // not here. This tool just validates and returns the result.
                // The executor will detect this is submit_result and handle specially.
                ToolOutput::success(format!("Result submitted: {}", parsed.result))
            }
            Err(e) => ToolOutput::error(format!("Invalid input: {e}")),
        }
    }
}

/// Tool for sub-agents to report failure
pub struct SubmitErrorTool;

#[derive(Debug, Deserialize)]
struct SubmitErrorInput {
    error: String,
}

#[async_trait]
impl Tool for SubmitErrorTool {
    fn name(&self) -> &'static str {
        "submit_error"
    }

    fn description(&self) -> String {
        "Report that you cannot complete the assigned task. Call this if you encounter an unrecoverable error or determine the task is impossible. After calling this, your conversation ends.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["error"],
            "properties": {
                "error": {
                    "type": "string",
                    "description": "Description of why the task could not be completed"
                }
            }
        })
    }

    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        match serde_json::from_value::<SubmitErrorInput>(input) {
            Ok(parsed) => {
                // Same as submit_result - actual transition handled by state machine
                ToolOutput::success(format!("Error submitted: {}", parsed.error))
            }
            Err(e) => ToolOutput::error(format!("Invalid input: {e}")),
        }
    }
}

/// Tool for parent conversations to spawn sub-agents.
///
/// Holds the working-directory's discovered named agents (sorted by name) so
/// `input_schema` can render them as an `agent_type` enum (REQ-AG-004). The
/// catalog is captured per-conversation at registry-construction time; the
/// `Tool` trait's `input_schema(&self)` has no working-directory parameter, so
/// capturing here is what lets a static schema method emit a dynamic enum.
#[derive(Default)]
pub struct SpawnAgentsTool {
    agents: Vec<AgentDefinition>,
}

impl SpawnAgentsTool {
    /// A spawn tool with no named agents — `agent_type` is omitted from the
    /// schema. Used by tests and any caller without a discovered catalog.
    #[must_use]
    pub fn new() -> Self {
        Self { agents: Vec::new() }
    }

    /// A spawn tool whose schema exposes the given named agents as the
    /// `agent_type` enum. `agents` must be sorted by name (the discovery
    /// contract) for the rendered schema to be cache-stable (REQ-AG-008).
    #[must_use]
    pub fn with_agents(agents: Vec<AgentDefinition>) -> Self {
        Self { agents }
    }
}

#[derive(Debug, Deserialize)]
struct SpawnAgentsInput {
    tasks: Vec<TaskSpec>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Authoritative parsing/resolution happens in the executor.
struct TaskSpec {
    task: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    mode: Option<String>, // "explore" or "work"
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_turns: Option<u32>,
    #[serde(default)]
    agent_type: Option<String>,
}

#[async_trait]
impl Tool for SpawnAgentsTool {
    fn name(&self) -> &'static str {
        "spawn_agents"
    }

    fn description(&self) -> String {
        "Spawn sub-agents to execute tasks in parallel. Each sub-agent runs independently and returns a result. Omit agent_type for a generic Phoenix sub-agent, or set agent_type to one of the discovered named personas. Use for: multiple perspectives on code review, exploring unfamiliar parts of a codebase, parallel research or analysis tasks, or divide-and-conquer problem solving.".to_string()
    }

    fn input_schema(&self) -> Value {
        let mut task_props = json!({
            "task": {
                "type": "string",
                "description": "Task description for the sub-agent"
            },
            "cwd": {
                "type": "string",
                "description": "Working directory (defaults to parent's cwd)"
            },
            "mode": {
                "type": "string",
                "enum": ["explore", "work"],
                "description": "Sub-agent mode. Explore (default): read-only tools, registry/provider-selected cheap model. Work: full tool suite, inherits the parent model. Work mode requires the parent to be in Work mode."
            },
            "model": {
                "type": "string",
                "description": "LLM model override. When set, it must be one of the model IDs available in this environment's model registry. Defaults based on mode."
            },
            "max_turns": {
                "type": "integer",
                "minimum": 1,
                "description": "Maximum LLM turns before forced completion. Defaults to 20 (explore) or 50 (work)."
            }
        });

        // REQ-AG-004: surface discovered named agents as a typed agent_type
        // enum. Omitted entirely when none are discovered, so the schema is a
        // strict subset of the agent-free shape. The catalog is pre-sorted by
        // name, so the rendered enum is byte-stable turn-to-turn (REQ-AG-008).
        if !self.agents.is_empty() {
            use std::fmt::Write as _;
            let names: Vec<&str> = self.agents.iter().map(|a| a.name.as_str()).collect();
            let mut description = String::from(
                "Named agent persona to spawn. Omit this field for a generic Phoenix sub-agent. When set, it supplies the sub-agent's persona and its default model/mode. Available named personas:",
            );
            for agent in &self.agents {
                let _ = write!(description, "\n- {}: {}", agent.name, agent.description);
            }
            task_props["agent_type"] = json!({
                "type": "string",
                "enum": names,
                "description": description
            });
        }

        json!({
            "type": "object",
            "required": ["tasks"],
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["task"],
                        "properties": task_props
                    },
                    "minItems": 1,
                    "maxItems": 10,
                    "description": "List of tasks to execute in parallel (max 10). Omit agent_type on a task to spawn a generic Phoenix sub-agent."
                }
            }
        })
    }

    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        match serde_json::from_value::<SpawnAgentsInput>(input) {
            Ok(parsed) => {
                if parsed.tasks.is_empty() {
                    return ToolOutput::error("At least one task is required");
                }

                // The actual spawning is handled by the executor when it receives
                // the SpawnAgentsComplete event. Here we just validate and return
                // a description of what will be spawned.
                let task_summaries: Vec<String> = parsed
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let cwd_info = t
                            .cwd
                            .as_ref()
                            .map_or(String::new(), |c| format!(" (cwd: {c})"));
                        format!("{}. {}{}", i + 1, truncate(&t.task, 100), cwd_info)
                    })
                    .collect();

                ToolOutput::success(format!(
                    "Spawning {} sub-agent(s):\n{}",
                    parsed.tasks.len(),
                    task_summaries.join("\n")
                ))
            }
            Err(e) => ToolOutput::error(format!("Invalid input: {e}")),
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", s.get(..max_len.saturating_sub(3)).unwrap_or(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::BrowserSessionManager;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn test_context() -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            PathBuf::from("/tmp"),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
            None,
        )
    }

    #[tokio::test]
    async fn test_submit_result_valid() {
        let tool = SubmitResultTool;
        let result = tool
            .run(
                json!({"result": "Task completed successfully"}),
                test_context(),
            )
            .await;
        assert!(result.is_success());
        assert!(result.output().contains("Result submitted"));
    }

    #[tokio::test]
    async fn test_submit_result_missing_field() {
        let tool = SubmitResultTool;
        let result = tool.run(json!({}), test_context()).await;
        assert!(!result.is_success());
    }

    #[tokio::test]
    async fn test_submit_error_valid() {
        let tool = SubmitErrorTool;
        let result = tool
            .run(json!({"error": "Could not find the file"}), test_context())
            .await;
        assert!(result.is_success()); // Tool execution succeeds, even though it reports an error
        assert!(result.output().contains("Error submitted"));
    }

    fn agent(name: &str, description: &str) -> AgentDefinition {
        AgentDefinition {
            name: name.to_string(),
            description: description.to_string(),
            body: format!("You are {name}."),
            path: std::path::PathBuf::from(format!("/agents/{name}.md")),
            source_dir: ".claude/agents".to_string(),
            model: None,
            mode: None,
            tools: None,
        }
    }

    #[test]
    fn schema_omits_agent_type_when_no_agents() {
        // REQ-AG-004: agent-free schema is a strict subset of today's shape.
        let schema = SpawnAgentsTool::new().input_schema();
        let props = &schema["properties"]["tasks"]["items"]["properties"];
        assert!(props.get("agent_type").is_none());
    }

    #[test]
    fn schema_renders_agent_type_enum_sorted() {
        // REQ-AG-004 / REQ-AG-008: enum present, in the given (sorted) order,
        // with per-agent description lines.
        let tool = SpawnAgentsTool::with_agents(vec![
            agent("docs-writer", "Writes docs"),
            agent("security-reviewer", "Finds vulns"),
        ]);
        let schema = tool.input_schema();
        let agent_type = &schema["properties"]["tasks"]["items"]["properties"]["agent_type"];
        assert_eq!(
            agent_type["enum"],
            json!(["docs-writer", "security-reviewer"])
        );
        let desc = agent_type["description"].as_str().unwrap();
        assert!(desc.contains("docs-writer: Writes docs"));
        assert!(desc.contains("security-reviewer: Finds vulns"));
    }

    #[test]
    fn schema_documents_generic_subagents_with_named_agents() {
        let tool = SpawnAgentsTool::with_agents(vec![
            agent("docs-writer", "Writes docs"),
            agent("security-reviewer", "Finds vulns"),
        ]);
        let schema = tool.input_schema();
        let tool_desc = tool.description();
        assert!(
            tool_desc.contains("Omit agent_type for a generic Phoenix sub-agent"),
            "tool description should advertise generic sub-agents: {tool_desc}"
        );

        let tasks_desc = schema["properties"]["tasks"]["description"]
            .as_str()
            .unwrap();
        assert!(
            tasks_desc.contains("Omit agent_type"),
            "tasks description should advertise generic sub-agents: {tasks_desc}"
        );

        let agent_type = &schema["properties"]["tasks"]["items"]["properties"]["agent_type"];
        assert_eq!(
            agent_type["enum"],
            json!(["docs-writer", "security-reviewer"]),
            "known named personas must remain enum-validated"
        );
        let desc = agent_type["description"].as_str().unwrap();
        assert!(
            desc.contains("Omit this field for a generic Phoenix sub-agent"),
            "agent_type description should not imply named personas are exhaustive: {desc}"
        );
        assert!(desc.contains("docs-writer: Writes docs"));
        assert!(desc.contains("security-reviewer: Finds vulns"));
    }

    #[test]
    fn schema_model_guidance_is_provider_neutral() {
        let schema = SpawnAgentsTool::new().input_schema();
        let props = &schema["properties"]["tasks"]["items"]["properties"];
        let mode_guidance = props["mode"]["description"].as_str().unwrap();
        let override_guidance = props["model"]["description"].as_str().unwrap();
        let guidance = format!("{mode_guidance}\n{override_guidance}");

        assert!(
            mode_guidance.contains("registry/provider-selected cheap model"),
            "mode guidance should describe the provider-neutral explore default: {mode_guidance}"
        );
        assert!(
            mode_guidance.contains("inherits the parent model"),
            "mode guidance should describe the work-mode default: {mode_guidance}"
        );
        assert!(
            override_guidance.contains("model IDs available in this environment's model registry"),
            "model override guidance should describe registry validation: {override_guidance}"
        );
        for provider_specific_alias in ["claude-haiku-4-5", "claude-sonnet-4-6", "haiku model"] {
            assert!(
                !guidance.contains(provider_specific_alias),
                "spawn_agents schema guidance should not mention provider-specific alias {provider_specific_alias:?}: {guidance}"
            );
        }
    }

    #[test]
    fn schema_is_byte_stable_across_calls() {
        // REQ-AG-008: repeated input_schema() over the same catalog is identical.
        let tool =
            SpawnAgentsTool::with_agents(vec![agent("a", "A"), agent("b", "B"), agent("c", "C")]);
        assert_eq!(tool.input_schema(), tool.input_schema());
    }

    #[tokio::test]
    async fn test_spawn_agents_valid() {
        let tool = SpawnAgentsTool::new();
        let result = tool
            .run(
                json!({
                    "tasks": [
                        {"task": "Review security"},
                        {"task": "Review performance", "cwd": "/project"}
                    ]
                }),
                test_context(),
            )
            .await;
        assert!(result.is_success());
        assert!(result.output().contains("Spawning 2 sub-agent(s)"));
    }

    #[tokio::test]
    async fn test_spawn_agents_empty_tasks() {
        let tool = SpawnAgentsTool::new();
        let result = tool.run(json!({"tasks": []}), test_context()).await;
        assert!(!result.is_success());
    }

    #[tokio::test]
    async fn test_spawn_agents_missing_tasks() {
        let tool = SpawnAgentsTool::new();
        let result = tool.run(json!({}), test_context()).await;
        assert!(!result.is_success());
    }
}
