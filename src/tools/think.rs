//! Think tool - allows LLM to think out loud without side effects
//!
//! REQ-THINK-001: Thought Recording
//! REQ-THINK-002: Tool Schema

use super::{Tool, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

/// Think tool for LLM reasoning
pub struct ThinkTool;

#[derive(Debug, Deserialize)]
struct ThinkInput {
    #[allow(dead_code)] // Deserialized for validation, content echoed via input json
    thoughts: String,
}

#[async_trait]
impl Tool for ThinkTool {
    fn name(&self) -> &'static str {
        "think"
    }

    fn description(&self) -> String {
        "Think out loud, take notes, form plans. Has no external effects.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["thoughts"],
            "properties": {
                "thoughts": {
                    "type": "string",
                    "description": "The thoughts, notes, or plans to record"
                }
            }
        })
    }

    async fn run(&self, input: Value) -> ToolOutput {
        // Parse input (mainly for validation)
        match serde_json::from_value::<ThinkInput>(input) {
            Ok(_) => ToolOutput::success("recorded"),
            Err(e) => ToolOutput::error(format!("Invalid input: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_think_records() {
        let tool = ThinkTool;
        let result = tool
            .run(json!({"thoughts": "Planning my approach..."}))
            .await;
        assert!(result.success);
        assert_eq!(result.output, "recorded");
    }

    #[tokio::test]
    async fn test_think_empty_thoughts() {
        let tool = ThinkTool;
        let result = tool.run(json!({"thoughts": ""})).await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_think_missing_thoughts() {
        let tool = ThinkTool;
        let result = tool.run(json!({})).await;
        assert!(!result.success);
    }
}
