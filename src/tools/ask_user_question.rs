//! `ask_user_question` tool — pauses execution for user input (REQ-AUQ-001)
//!
//! This tool is intercepted at the state machine level (`LlmResponse` handler)
//! before it ever reaches `ToolExecuting`. The `run()` method exists only as a
//! fallback and should never be called in practice.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool definition for `ask_user_question`. Registered in all non-sub-agent
/// registries so the LLM sees it in its tool list. Intercepted before execution
/// -- the state machine transitions to `AwaitingUserResponse` instead.
pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &'static str {
        "ask_user_question"
    }

    fn description(&self) -> String {
        "Ask the user clarifying questions when you need input to proceed. \
         Use when there are multiple valid approaches and user preference \
         matters. Provide 1-4 questions with 2-4 options each. Users can \
         also type custom answers. This must be the only tool call in \
         the response (do not combine with other tool calls)."
            .to_string()
    }

    /// REQ-AUQ-008: Rarely used, defer full schema to reduce prompt size.
    fn defer_loading(&self) -> bool {
        true
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["questions"],
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "1-4 questions to ask the user",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "required": ["question", "header", "options"],
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The full question text"
                            },
                            "header": {
                                "type": "string",
                                "description": "Short header label (max 12 characters)",
                                "maxLength": 12
                            },
                            "options": {
                                "type": "array",
                                "description": "2-4 options for the user to choose from",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "required": ["label"],
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Option label shown to the user"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Optional longer description of this option"
                                        },
                                        "preview": {
                                            "type": "string",
                                            "description": "Optional preview content shown when this option is selected"
                                        }
                                    }
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Whether the user can select multiple options (default: false)",
                                "default": false
                            }
                        }
                    }
                }
            }
        })
    }

    async fn run(&self, _input: Value, _ctx: ToolContext) -> ToolOutput {
        // This should never be called — ask_user_question is intercepted at the
        // state machine level before entering ToolExecuting.
        ToolOutput::error(
            "ask_user_question was not intercepted by the state machine. \
             This is a bug — please report it.",
        )
    }
}
