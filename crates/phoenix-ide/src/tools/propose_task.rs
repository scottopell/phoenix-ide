//! `propose_task` tool — pure data carrier for task proposals (REQ-PROJ-012)
//!
//! This tool is intercepted at the state machine level (`LlmResponse` handler)
//! before it ever reaches `ToolExecuting`. The `run()` method exists only as a
//! fallback and should never be called in practice.
//!
//! The tool references an existing markdown file on disk in the conversation's
//! working directory. The agent (in Explore mode) writes/edits that file via
//! `patch` — task content lives on disk, not in the tool call, so revisions are
//! file edits instead of round-tripping the full plan through tool args. A
//! taskmd 1.0 filename (`NNNNN-pX-status--slug.md`) is one accepted form (it
//! additionally yields id/priority/status/slug and an automatic
//! `ready → in-progress` rename on approval); any other `.md` file works too.

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
         Pass the path to a markdown file in your working directory — either \
         an existing file you want to begin working on, or one you created \
         with `patch` in this conversation. The body is free-form markdown, \
         shown to the user as the plan; start it with an `# H1` title. If \
         the filename follows the taskmd 1.0 convention \
         (`NNNNN-pX-status--slug.md`, conventionally under `tasks/`) it also \
         carries id/priority/status/slug — in that case status must be one \
         of: ready, in-progress, brainstorming. This must be the only tool \
         call in the response."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["task_file"],
            "properties": {
                "task_file": {
                    "type": "string",
                    "description": "Path (relative to your working directory) to an existing markdown (.md) file. A taskmd 1.0 filename (e.g. tasks/01234-p2-ready--my-slug.md) additionally derives id/priority/status/slug from the name; any other .md file (e.g. docs/plan.md) is treated as a plain task brief with the title taken from its first `# H1`."
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
