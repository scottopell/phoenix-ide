//! `propose_task` tool — pure data carrier for task proposals (REQ-PROJ-012)
//!
//! This tool is intercepted at the state machine level (`LlmResponse` handler)
//! before it ever reaches `ToolExecuting`. The `run()` method exists only as a
//! fallback and should never be called in practice.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool definition for `propose_task`. Registered in Explore mode so the LLM
/// sees it in its tool list. Intercepted before execution -- the state machine
/// transitions to `AwaitingTaskApproval` instead.
pub struct ProposeTaskTool;

#[async_trait]
impl Tool for ProposeTaskTool {
    fn name(&self) -> &'static str {
        "propose_task"
    }

    fn description(&self) -> String {
        "Propose a task for the user to review and approve. This is the \
         gateway from Explore mode (read-only) to Work mode (write access). \
         The task will be shown to the user for review — they can approve it, \
         request changes, or discard it. This must be the only tool call in \
         the response (do not combine with other tool calls)."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["title", "priority", "plan"],
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short title for the task (used in filenames and branch names)"
                },
                "priority": {
                    "type": "string",
                    "enum": ["p0", "p1", "p2", "p3"],
                    "description": "Priority level: p0 (critical), p1 (high), p2 (medium), p3 (low)"
                },
                "plan": {
                    "type": "string",
                    "description": "The full task plan in markdown. Include: summary, context, what to do, and acceptance criteria."
                }
            }
        })
    }

    async fn run(&self, _input: Value, _ctx: ToolContext) -> ToolOutput {
        // This should never be called — propose_task is intercepted at the
        // state machine level before entering ToolExecuting.
        ToolOutput::error(
            "propose_task was not intercepted by the state machine. \
             This is a bug — please report it.",
        )
    }
}
