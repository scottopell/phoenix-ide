//! Sub-agent tools - tools for sub-agent lifecycle management
//!
//! - spawn_agents: Spawn sub-agents (parent only)
//! - submit_result: Submit successful result (sub-agent only)
//! - submit_error: Submit error result (sub-agent only)

use super::{Tool, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

// ============================================================================
// submit_result - Sub-agent successful completion
// ============================================================================

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

    async fn run(&self, input: Value, _cancel: CancellationToken) -> ToolOutput {
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

// ============================================================================
// submit_error - Sub-agent error completion
// ============================================================================

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

    async fn run(&self, input: Value, _cancel: CancellationToken) -> ToolOutput {
        match serde_json::from_value::<SubmitErrorInput>(input) {
            Ok(parsed) => {
                // Same as submit_result - actual transition handled by state machine
                ToolOutput::success(format!("Error submitted: {}", parsed.error))
            }
            Err(e) => ToolOutput::error(format!("Invalid input: {e}")),
        }
    }
}

// ============================================================================
// spawn_agents - Parent spawns sub-agents
// ============================================================================

/// Tool for parent conversations to spawn sub-agents
pub struct SpawnAgentsTool;

#[derive(Debug, Deserialize)]
struct SpawnAgentsInput {
    tasks: Vec<TaskSpec>,
}

#[derive(Debug, Deserialize)]
struct TaskSpec {
    task: String,
    #[serde(default)]
    cwd: Option<String>,
}

#[async_trait]
impl Tool for SpawnAgentsTool {
    fn name(&self) -> &'static str {
        "spawn_agents"
    }

    fn description(&self) -> String {
        "Spawn sub-agents to execute tasks in parallel. Each sub-agent runs independently and returns a result. Use for: multiple perspectives on code review, exploring unfamiliar parts of a codebase, parallel research or analysis tasks, or divide-and-conquer problem solving.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["tasks"],
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["task"],
                        "properties": {
                            "task": {
                                "type": "string",
                                "description": "Task description for the sub-agent"
                            },
                            "cwd": {
                                "type": "string",
                                "description": "Working directory (defaults to parent's cwd)"
                            }
                        }
                    },
                    "minItems": 1,
                    "description": "List of tasks to execute in parallel"
                }
            }
        })
    }

    async fn run(&self, input: Value, _cancel: CancellationToken) -> ToolOutput {
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
                        let cwd_info = t.cwd.as_ref().map_or(String::new(), |c| format!(" (cwd: {c})"));
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
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_submit_result_valid() {
        let tool = SubmitResultTool;
        let result = tool
            .run(
                json!({"result": "Task completed successfully"}),
                CancellationToken::new(),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("Result submitted"));
    }

    #[tokio::test]
    async fn test_submit_result_missing_field() {
        let tool = SubmitResultTool;
        let result = tool.run(json!({}), CancellationToken::new()).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_submit_error_valid() {
        let tool = SubmitErrorTool;
        let result = tool
            .run(
                json!({"error": "Could not find the file"}),
                CancellationToken::new(),
            )
            .await;
        assert!(result.success); // Tool execution succeeds, even though it reports an error
        assert!(result.output.contains("Error submitted"));
    }

    #[tokio::test]
    async fn test_spawn_agents_valid() {
        let tool = SpawnAgentsTool;
        let result = tool
            .run(
                json!({
                    "tasks": [
                        {"task": "Review security"},
                        {"task": "Review performance", "cwd": "/project"}
                    ]
                }),
                CancellationToken::new(),
            )
            .await;
        assert!(result.success);
        assert!(result.output.contains("Spawning 2 sub-agent(s)"));
    }

    #[tokio::test]
    async fn test_spawn_agents_empty_tasks() {
        let tool = SpawnAgentsTool;
        let result = tool
            .run(json!({"tasks": []}), CancellationToken::new())
            .await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_spawn_agents_missing_tasks() {
        let tool = SpawnAgentsTool;
        let result = tool.run(json!({}), CancellationToken::new()).await;
        assert!(!result.success);
    }
}
