//! `terminal_last_command` tool — REQ-TERM-022.
//!
//! Returns the most recently completed command from the terminal's ring buffer.
//! Requires OSC 133 shell integration to be active.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::terminal::ShellIntegrationStatus;

pub struct TerminalLastCommandTool;

#[async_trait]
impl Tool for TerminalLastCommandTool {
    fn name(&self) -> &'static str {
        "terminal_last_command"
    }

    fn description(&self) -> String {
        "Returns the most recently completed command from the terminal, including its output \
         and exit code. Requires shell integration (OSC 133) to be active. \
         Use this instead of read_terminal after running commands."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn run(&self, _input: Value, ctx: ToolContext) -> ToolOutput {
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

        let record = {
            let tracker = handle.tracker.lock().expect("tracker lock poisoned");
            tracker.last_command().cloned()
        };

        let Some(rec) = record else {
            return ToolOutput::error("no commands have completed in this terminal session yet");
        };

        let started_secs = rec
            .started_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        ToolOutput::success(
            json!({
                "command": rec.command_text,
                "output": rec.output,
                "exit_code": rec.exit_code,
                "duration_ms": rec.duration_ms,
                "started_at": started_secs,
            })
            .to_string(),
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
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            Arc::new(crate::tools::TmuxRegistry::new()),
            None,
        )
    }

    #[tokio::test]
    async fn no_terminal_returns_error() {
        let tool = TerminalLastCommandTool;
        let result = tool.run(json!({}), test_context()).await;
        assert!(!result.success);
        assert!(result.output.contains("no terminal"));
    }
}
