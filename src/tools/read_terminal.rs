//! `read_terminal` tool — REQ-TERM-011
//!
//! Returns the current vt100 screen contents of the conversation's active terminal.
//! Optionally waits for output quiescence (300 ms of silence) before reading,
//! which gives more meaningful results after running a command.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// How long to wait for quiescence before giving up and returning anyway.
const QUIESCENCE_TIMEOUT_SECS: u64 = 5;

pub struct ReadTerminalTool;

#[derive(Debug, Deserialize)]
struct ReadTerminalInput {
    /// When `true` (default), the tool blocks until the terminal output stream
    /// has been quiet for 300 ms before returning, giving more meaningful
    /// results after running a command.  Set to `false` to return immediately.
    #[serde(default = "default_wait")]
    wait_for_quiescence: bool,
}

fn default_wait() -> bool {
    true
}

#[async_trait]
impl Tool for ReadTerminalTool {
    fn name(&self) -> &'static str {
        "read_terminal"
    }

    fn description(&self) -> String {
        "Read the current contents of the terminal screen for this conversation. \
         Returns the visible text exactly as it appears in the terminal (vt100 screen buffer). \
         \n\nWhen `wait_for_quiescence` is true (default), the tool waits up to 5 s for the \
         output stream to go quiet for 300 ms before returning — this is the right setting \
         after running a command, because it waits for the command to finish producing output. \
         Set to false to read immediately (useful for checking progress of a long-running command).\
         \n\nReturns an error if no terminal is open for this conversation."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wait_for_quiescence": {
                    "type": "boolean",
                    "description": "Wait for output to go quiet for 300 ms before reading (default: true). \
                                   Set false to read the current screen immediately.",
                    "default": true
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: ReadTerminalInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let Some(handle) = ctx.terminals.get(&ctx.conversation_id) else {
            return ToolOutput::error(
                "No terminal is open for this conversation. \
                 Open a terminal from the UI before calling read_terminal.",
            );
        };

        if parsed.wait_for_quiescence {
            // Subscribe before reading so we don't miss a quiescence tick
            // that fires between now and when we call recv().
            let mut quiescence_rx = handle.quiescence_tx.subscribe();
            let current = *quiescence_rx.borrow();

            let _ = tokio::time::timeout(Duration::from_secs(QUIESCENCE_TIMEOUT_SECS), async {
                loop {
                    if quiescence_rx.changed().await.is_err() {
                        break; // sender dropped (session ended)
                    }
                    if *quiescence_rx.borrow() > current {
                        break; // new quiescence tick received
                    }
                }
            })
            .await;
        }

        let screen_contents = {
            let parser = handle.parser.lock().expect("parser lock");
            parser.screen().contents()
        };

        if screen_contents.trim().is_empty() {
            ToolOutput::success("(terminal screen is empty)")
        } else {
            ToolOutput::success(screen_contents)
        }
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
    async fn test_no_terminal_returns_error() {
        let tool = ReadTerminalTool;
        let result = tool
            .run(json!({"wait_for_quiescence": false}), test_context())
            .await;
        assert!(!result.success);
        assert!(result.output.contains("No terminal"));
    }

    #[tokio::test]
    async fn test_default_wait_is_true() {
        let input: ReadTerminalInput = serde_json::from_value(json!({})).unwrap();
        assert!(input.wait_for_quiescence);
    }

    #[tokio::test]
    async fn test_wait_false_parses() {
        let input: ReadTerminalInput =
            serde_json::from_value(json!({"wait_for_quiescence": false})).unwrap();
        assert!(!input.wait_for_quiescence);
    }
}
