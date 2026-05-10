//! `propose_task` tool — pure data carrier for task proposals (REQ-PROJ-012)
//!
//! This tool is intercepted at the state machine level (`LlmResponse` handler)
//! before it ever reaches `ToolExecuting`. The `run()` method exists only as a
//! fallback and should never be called in practice.
//!
//! The tool now references an existing task file on disk under `tasks/`. The
//! agent (in Explore mode) writes/edits the task file via `patch` — task file
//! content lives on disk, not in the tool call. This keeps revisions as file
//! edits instead of round-tripping the full plan through tool args.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct ProposeTaskTool;

#[async_trait]
impl Tool for ProposeTaskTool {
    fn name(&self) -> &'static str {
        "propose_task"
    }

    fn description(&self) -> String {
        "Propose a task for the user to review and approve. This is the \
         gateway from Explore mode (read-only) to Work mode (write access). \
         Pass the path to a task file under `tasks/` — either an existing \
         task file you want to begin working on, or a new task file you \
         created with `patch` in this conversation. Priority and status \
         come from the filename (taskmd 1.0); the body is free-form \
         markdown and is shown to the user as the plan. Status must be \
         one of: ready, in-progress, brainstorming. This must be the only \
         tool call in the response."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["task_file"],
            "properties": {
                "task_file": {
                    "type": "string",
                    "description": "Path (relative to your working directory) to an existing task file under tasks/. The filename must follow the taskmd naming convention (e.g. tasks/01234-p2-ready--my-slug.md) — taskmd 1.0 derives id, priority, status, and slug from the filename, with no frontmatter."
                }
            }
        })
    }

    async fn run(&self, _input: Value, _ctx: ToolContext) -> ToolOutput {
        ToolOutput::error(
            "propose_task was not intercepted by the state machine. \
             This is a bug — please report it.",
        )
    }
}
