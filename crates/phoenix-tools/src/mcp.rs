//! `Tool`-trait wrapper over the MCP client engine.
//!
//! The protocol, transports, OAuth, and the multi-server [`McpClientManager`]
//! live in the `phoenix-mcp` crate, which has no dependency on the [`Tool`]
//! trait. This module is the thin adapter that exposes a discovered MCP tool as
//! a Phoenix [`Tool`]: [`McpTool`] wraps one `{server}__{tool}` and
//! [`create_mcp_tool_by_name`] resolves one live by name. The engine surface
//! callers reach through `tools::mcp::` is re-exported here.

use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub use phoenix_mcp::{oauth, McpClientManager};

/// Wraps a single MCP tool as a Phoenix Tool.
pub struct McpTool {
    server_name: String,
    tool_name: String,
    full_name: String,
    description: String,
    input_schema: Value,
    manager: Arc<McpClientManager>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        // Spawn call_tool as a detached task so that cancellation never drops
        // the future mid-write while it holds the stdin/stdout mutex locks.
        // If we cancelled by dropping the select'd future directly, a partial
        // JSON-RPC write could corrupt the server's stdin stream.
        let manager = Arc::clone(&self.manager);
        let server_name = self.server_name.clone();
        let tool_name = self.tool_name.clone();

        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = manager.call_tool(&server_name, &tool_name, input).await;
            // If the receiver was dropped (cancellation), this send fails silently.
            let _ = tx.send(result);
        });

        tokio::select! {
            biased;

            () = ctx.cancel.cancelled() => {
                tracing::debug!(
                    tool = %self.full_name,
                    "MCP tool call cancelled -- spawned task will complete in background"
                );
                ToolOutput::error("[mcp tool call cancelled]")
            }

            result = rx => {
                match result {
                    Ok(Ok(text)) => ToolOutput::success(text),
                    Ok(Err(e)) => ToolOutput::error(e),
                    // Spawned task panicked or was aborted
                    Err(_) => ToolOutput::error("MCP tool call task terminated unexpectedly"),
                }
            }
        }
    }
}

/// Look up a single MCP tool by its full `{server}__{tool}` name.
/// Used by `ToolRegistryExecutor` for live resolution of MCP tools
/// that aren't in the static registry.
pub async fn create_mcp_tool_by_name(
    manager: &Arc<McpClientManager>,
    full_name: &str,
) -> Option<Box<dyn Tool>> {
    let (server_name, tool_name) = full_name.split_once("__")?;
    let defs = manager.tool_definitions().await;
    let (srv, def) = defs
        .into_iter()
        .find(|(s, d)| s == server_name && d.name == tool_name)?;

    let name = format!("{srv}__{}", def.name);
    Some(Box::new(McpTool {
        server_name: srv,
        tool_name: def.name,
        full_name: name,
        description: def.description,
        input_schema: def.input_schema,
        manager: Arc::clone(manager),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_naming() {
        let manager = Arc::new(McpClientManager::new());

        let tool = McpTool {
            server_name: "slack".to_string(),
            tool_name: "send_message".to_string(),
            full_name: "slack__send_message".to_string(),
            description: "Send a Slack message".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            manager,
        };

        assert_eq!(tool.name(), "slack__send_message");
        assert_eq!(tool.description(), "Send a Slack message");
    }

    #[tokio::test]
    async fn test_create_mcp_tool_by_name_empty() {
        let manager = Arc::new(McpClientManager::new());
        let tool = create_mcp_tool_by_name(&manager, "slack__send_message").await;
        assert!(tool.is_none());
    }
}
