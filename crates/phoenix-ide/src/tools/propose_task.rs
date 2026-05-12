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
         shown to the user as the plan; start it with an `# H1` title. \
         Prefer the taskmd 1.0 convention: name the file \
         `NNNNN-pX-status--slug.md` (status one of: ready, in-progress, \
         brainstorming) under your project's tasks directory — that gives the \
         task a stable id/priority/status/slug. (taskmd files MUST live under \
         the tasks directory; a non-taskmd `.md` file works too as a plain \
         brief with no metadata, and can live anywhere in the worktree.) This \
         must be the only tool call in the response."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["task_file"],
            "properties": {
                "task_file": {
                    "type": "string",
                    "description": "Path (relative to your working directory) to an existing markdown (.md) file. Prefer a taskmd 1.0 filename (NNNNN-pX-status--slug.md) under your project's tasks directory — it derives id/priority/status/slug from the name. A taskmd-pattern filename is ONLY accepted under the tasks directory; any other .md file (e.g. docs/plan.md) is treated as a plain task brief (title from its first `# H1`, no metadata) and may live anywhere in the worktree."
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
