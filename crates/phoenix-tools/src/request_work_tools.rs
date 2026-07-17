//! `request_work_tools` — an Explore agent's capability-escalation request.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct RequestWorkToolsTool;

#[async_trait]
impl Tool for RequestWorkToolsTool {
    fn name(&self) -> &'static str {
        "request_work_tools"
    }

    fn description(&self) -> String {
        "Request user approval to use the full Work toolset while continuing the current Explore conversation. Use this when completing exploration requires capabilities such as unrestricted shell or network access. This does not propose a task or begin the Work lifecycle. Explain the concrete investigation that needs the additional capabilities. This must be the only tool call in the response.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["reason"],
            "properties": {
                "reason": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Why the exploration needs the full Work toolset and what you intend to investigate."
                }
            },
            "additionalProperties": false
        })
    }

    async fn run(&self, _input: Value, _ctx: ToolContext) -> ToolOutput {
        ToolOutput::error("request_work_tools was not intercepted by the state machine. This is a bug — please report it.")
    }
}
