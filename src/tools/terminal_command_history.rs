//! `terminal_command_history` tool — REQ-TERM-023.
//!
//! Returns up to `count` recent commands from the terminal's ring buffer,
//! newest first.  Requires OSC 133 shell integration to be active.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::terminal::ShellIntegrationStatus;

pub struct TerminalCommandHistoryTool;

#[derive(Debug, Deserialize)]
struct HistoryInput {
    #[serde(default = "default_count")]
    count: usize,
}

fn default_count() -> usize {
    3
}

#[async_trait]
impl Tool for TerminalCommandHistoryTool {
    fn name(&self) -> &'static str {
        "terminal_command_history"
    }

    fn description(&self) -> String {
        "Returns the last N completed commands from the terminal (newest first). \
         Requires shell integration (OSC 133) to be active. \
         `count` defaults to 3, maximum 5."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Number of recent commands to return (default 3, max 5).",
                    "default": 3,
                    "minimum": 1,
                    "maximum": 5
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: HistoryInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        // Clamp count to [1, 5].
        let count = parsed.count.clamp(1, 5);

        let Some(handle) = ctx.terminals.get(&ctx.conversation_id) else {
            return ToolOutput::error("no terminal is open for this conversation");
        };

        let status = *handle
            .shell_integration_status
            .lock()
            .expect("shell_integration_status lock poisoned");

        if status != ShellIntegrationStatus::Detected {
            return ToolOutput::error(
                "shell integration is not active for this terminal \
                 — install the shell integration snippet to enable command tracking",
            );
        }

        let records = {
            let tracker = handle.tracker.lock().expect("tracker lock poisoned");
            tracker
                .recent_commands(count)
                .into_iter()
                .map(|rec| {
                    let started_secs = rec
                        .started_at
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    json!({
                        "command": rec.command_text,
                        "output": rec.output,
                        "exit_code": rec.exit_code,
                        "duration_ms": rec.duration_ms,
                        "started_at": started_secs,
                    })
                })
                .collect::<Vec<_>>()
        };

        if records.is_empty() {
            return ToolOutput::error("no commands have completed in this terminal session yet");
        }

        ToolOutput::success(
            serde_json::Value::Array(records).to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::BrowserSessionManager;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn test_context() -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            std::path::PathBuf::from("/tmp"),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
        )
    }

    #[tokio::test]
    async fn no_terminal_returns_error() {
        let tool = TerminalCommandHistoryTool;
        let result = tool.run(json!({}), test_context()).await;
        assert!(!result.success);
        assert!(result.output.contains("no terminal"));
    }

    #[tokio::test]
    async fn default_count_parses() {
        let input: HistoryInput = serde_json::from_value(json!({})).unwrap();
        assert_eq!(input.count, 3);
    }

    #[tokio::test]
    async fn count_5_parses() {
        let input: HistoryInput = serde_json::from_value(json!({"count": 5})).unwrap();
        assert_eq!(input.count, 5);
    }
}
