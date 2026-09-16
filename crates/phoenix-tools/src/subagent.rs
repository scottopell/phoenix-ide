//! Sub-agent tools - tools for sub-agent lifecycle management
//!
//! - `spawn_agents`: Spawn sub-agents (parent only)
//! - `submit_result`: Submit successful result (sub-agent only)
//! - `submit_error`: Submit error result (sub-agent only)

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use phoenix_agents::{AgentDefinition, ModelEffort};
use phoenix_core::domain::sm_state::SpawnAgentsInput;
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

#[derive(Debug, Clone)]
pub struct SpawnModelChoice {
    pub model: String,
    pub connection: String,
    pub efforts: Vec<ModelEffort>,
}

#[derive(Default)]
pub struct SpawnAgentsTool {
    agents: Vec<AgentDefinition>,
    tiers: Vec<String>,
    models: Vec<SpawnModelChoice>,
}

impl SpawnAgentsTool {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_agents(agents: Vec<AgentDefinition>) -> Self {
        Self::with_execution_choices(agents, Vec::new(), Vec::new())
    }

    #[must_use]
    pub fn with_execution_choices(
        mut agents: Vec<AgentDefinition>,
        mut tiers: Vec<String>,
        mut models: Vec<SpawnModelChoice>,
    ) -> Self {
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        tiers.sort();
        tiers.dedup();
        models.sort_by(|a, b| (&a.model, &a.connection).cmp(&(&b.model, &b.connection)));
        Self {
            agents,
            tiers,
            models,
        }
    }
}

#[async_trait]
impl Tool for SpawnAgentsTool {
    fn name(&self) -> &'static str {
        "spawn_agents"
    }

    fn description(&self) -> String {
        "Spawn sub-agents to execute tasks. Explore sub-agents may run in parallel. Work sub-agents run one at a time per parent: include at most one Work task per call and wait for it to finish before spawning another. Each sub-agent has an independent conversation and returns its own result. Work sub-agents use the resolved task cwd directly. An omitted or blank cwd inherits the parent cwd; Work/Branch overrides stay within the parent worktree, while Direct overrides are unscoped. Phoenix does not create a separate child worktree or merge child changes. Omit agent_type for a generic Phoenix sub-agent, or set agent_type to one of the available named personas. Omit execution to use the named worker's preferences, or otherwise inherit the parent's model, connection, and effort. Choose a configured tier or an explicit model connection to override execution. Execution selection is independent of permissions. Use for: multiple perspectives on code review, exploring unfamiliar parts of a codebase, parallel research or analysis tasks, or divide-and-conquer problem solving.".to_string()
    }

    fn input_schema(&self) -> Value {
        let mut task_props = json!({
            "task": {
                "type": "string",
                "description": "Task description for the sub-agent"
            },
            "cwd": {
                "type": "string",
                "description": "Working directory override. Omit or leave blank to inherit the parent's cwd. Relative paths resolve from the parent's cwd."
            },
            "mode": {
                "type": "string",
                "enum": ["explore", "work"],
                "description": "Sub-agent mode. Explore (default): read-only tools; Explore sub-agents may run in parallel. Work: full tool suite and runs one at a time per parent. Mode does not choose the model, connection, or effort. Include at most one Work task per call and wait for the active Work child to finish before spawning another. Work uses the resolved task cwd directly. An omitted or blank cwd inherits the parent cwd; Work/Branch overrides stay within the parent worktree, while Direct overrides are unscoped. Phoenix does not create a separate child worktree or merge child changes. Work mode requires a write-capable parent (Work, Branch, or Direct)."
            },
            "max_turns": {
                "type": "integer",
                "minimum": 1,
                "description": "Maximum LLM turns before forced completion. Defaults to 20 (explore) or 50 (work)."
            }
        });

        let mut execution_options = Vec::new();
        if !self.tiers.is_empty() {
            execution_options.push(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["type", "name"],
                "properties": {
                    "type": { "type": "string", "enum": ["tier"] },
                    "name": { "type": "string", "enum": self.tiers }
                }
            }));
        }
        for model in &self.models {
            let mut properties = json!({
                "type": { "type": "string", "enum": ["model"] },
                "model": { "type": "string", "enum": [model.model] },
                "connection": { "type": "string", "enum": [model.connection] }
            });
            if !model.efforts.is_empty() {
                properties["reasoning_effort"] = json!({
                    "type": "string",
                    "enum": model.efforts,
                    "description": "Optional effort for this model connection. Omit to use this model's default effort."
                });
            }
            execution_options.push(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["type", "model", "connection"],
                "properties": properties
            }));
        }
        if !execution_options.is_empty() {
            task_props["execution"] = json!({
                "oneOf": execution_options,
                "description": "Execution override: select one configured tier or one exact available model connection. Omit to use the named worker's preferences, otherwise inherit the parent's model, connection, and effort. Explicit models never silently fall back."
            });
        }

        if !self.agents.is_empty() {
            use std::fmt::Write as _;
            let names: Vec<&str> = self.agents.iter().map(|a| a.name.as_str()).collect();
            let mut description = String::from(
                "Named agent persona to spawn. Omit this field for a generic Phoenix sub-agent. When set, it supplies the sub-agent's instructions and execution preferences. An execution override replaces those preferences; permissions remain separate. Available named personas:",
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
            "additionalProperties": false,
            "required": ["tasks"],
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
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
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
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
            execution: None,
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
    fn schema_pins_each_model_to_its_connection_and_efforts() {
        let schema = SpawnAgentsTool::with_execution_choices(
            Vec::new(),
            vec!["fast".into(), "capable".into()],
            vec![
                SpawnModelChoice {
                    model: "gpt-a".into(),
                    connection: "codex".into(),
                    efforts: vec![ModelEffort::High],
                },
                SpawnModelChoice {
                    model: "gpt-a".into(),
                    connection: "gateway".into(),
                    efforts: Vec::new(),
                },
            ],
        )
        .input_schema();
        let choices = &schema["properties"]["tasks"]["items"]["properties"]["execution"]["oneOf"];
        assert_eq!(
            choices[0]["properties"]["name"]["enum"],
            json!(["capable", "fast"])
        );
        assert_eq!(
            choices[1]["properties"]["connection"]["enum"],
            json!(["codex"])
        );
        assert_eq!(
            choices[1]["properties"]["reasoning_effort"]["enum"],
            json!(["high"])
        );
        assert_eq!(
            choices[1]["required"],
            json!(["type", "model", "connection"])
        );
        assert_eq!(
            choices[2]["properties"]["connection"]["enum"],
            json!(["gateway"])
        );
        assert!(choices[2]["properties"].get("reasoning_effort").is_none());
        assert_eq!(choices[2]["additionalProperties"], false);
    }

    #[test]
    fn generic_schema_has_no_unresolved_execution_choices() {
        let schema = SpawnAgentsTool::new().input_schema();
        let properties = &schema["properties"]["tasks"]["items"]["properties"];
        assert!(properties.get("model").is_none());
        assert!(properties.get("execution").is_none());
    }

    #[test]
    fn task_parser_rejects_legacy_model_and_mixed_execution() {
        for task in [
            json!({"task":"Review", "model":"opus"}),
            json!({"task":"Review", "execution":{"type":"tier", "name":"fast", "model":"opus"}}),
            json!({"task":"Review", "execution":{"type":"model", "model":"opus"}}),
        ] {
            assert!(serde_json::from_value::<SpawnAgentsInput>(json!({"tasks":[task]})).is_err());
        }
        assert!(serde_json::from_value::<SpawnAgentsInput>(json!({"tasks":[{"task":"Review", "execution":{"type":"model", "model":"gpt-a", "connection":"codex"}}]})).is_ok());
        assert!(serde_json::from_value::<SpawnAgentsInput>(
            json!({"tasks":[{"task":"Review", "execution":{"type":"tier", "name":"fast"}}]})
        )
        .is_ok());
    }

    #[test]
    fn mode_guidance_separates_permissions_from_execution() {
        let schema = SpawnAgentsTool::new().input_schema();
        let guidance = schema["properties"]["tasks"]["items"]["properties"]["mode"]["description"]
            .as_str()
            .unwrap();
        assert!(guidance.contains("Mode does not choose the model, connection, or effort"));
        assert!(!guidance.contains("cheap model"));
    }

    #[test]
    fn schema_describes_work_environment_without_advertising_parallel_work() {
        let tool = SpawnAgentsTool::new();
        let schema = tool.input_schema();
        let mode_guidance = schema["properties"]["tasks"]["items"]["properties"]["mode"]
            ["description"]
            .as_str()
            .unwrap();
        let description = tool.description();

        for expected in [
            "omitted or blank cwd inherits the parent cwd",
            "Work/Branch overrides stay within the parent worktree",
            "Direct overrides are unscoped",
            "does not create a separate child worktree",
            "merge child changes",
        ] {
            assert!(
                description.contains(expected),
                "tool description is missing {expected:?}: {description}"
            );
            assert!(
                mode_guidance.contains(expected),
                "mode guidance is missing {expected:?}: {mode_guidance}"
            );
        }
        assert!(mode_guidance.contains("Work, Branch, or Direct"));
        for expected in [
            "Explore sub-agents may run in parallel",
            "Work sub-agents run one at a time per parent",
            "at most one Work task per call",
            "wait for the active Work child to finish",
        ] {
            assert!(
                description.contains(expected) || mode_guidance.contains(expected),
                "spawn guidance is missing {expected:?}: {description}\n{mode_guidance}"
            );
        }
        for unsupported_claim in [
            "parallel with other Work sub-agents",
            "writes are not locked",
            "assignments should be disjoint",
        ] {
            assert!(!description.contains(unsupported_claim));
            assert!(!mode_guidance.contains(unsupported_claim));
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
