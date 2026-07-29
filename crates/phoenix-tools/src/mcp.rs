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

use phoenix_mcp::McpToolCallError;
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
        let result = self
            .manager
            .call_tool_cancellable(&self.server_name, &self.tool_name, input, ctx.cancel)
            .await;
        match result {
            Ok(text) => ToolOutput::success(text),
            Err(McpToolCallError::Cancelled) => ToolOutput::error("[mcp tool call cancelled]"),
            Err(McpToolCallError::Failed(error)) => ToolOutput::error(error),
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
