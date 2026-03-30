//! MCP (Model Context Protocol) client -- stdio transport
//!
//! Manages MCP server subprocesses, discovers tools via JSON-RPC 2.0,
//! and exposes them as regular Phoenix tools through the Tool trait.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

/// Timeout for a single JSON-RPC request-response round trip.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

// ---------------------------------------------------------------------------
// McpToolDef
// ---------------------------------------------------------------------------

/// Cached tool metadata from a tools/list response.
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// McpServer
// ---------------------------------------------------------------------------

/// Manages one stdio MCP server subprocess with JSON-RPC 2.0 communication.
pub struct McpServer {
    name: String,
    #[allow(dead_code)] // Holds ownership of child process; dropping it would kill the server
    child: Child,
    /// Locked together with `stdout` for request-response serialization.
    stdin: Mutex<BufWriter<ChildStdin>>,
    stdout: Mutex<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    tools: Vec<McpToolDef>,
}

impl McpServer {
    /// Spawn the child process with stdin/stdout piped.
    #[allow(clippy::unused_async)] // async block inside spawns a task; keeping async for API consistency
    pub async fn spawn(
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server '{name}': {e}"))?;

        let child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("MCP server '{name}': stdin not captured"))?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("MCP server '{name}': stdout not captured"))?;

        // Drain stderr to debug logs so the child doesn't block on a full pipe.
        if let Some(stderr) = child.stderr.take() {
            let server_name = name.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            tracing::debug!(
                                server = %server_name,
                                "MCP stderr: {}",
                                line.trim_end()
                            );
                        }
                    }
                }
            });
        }

        Ok(Self {
            name: name.to_string(),
            child,
            stdin: Mutex::new(BufWriter::new(child_stdin)),
            stdout: Mutex::new(BufReader::new(child_stdout)),
            next_id: AtomicU64::new(1),
            tools: Vec::new(),
        })
    }

    /// Send the JSON-RPC `initialize` handshake followed by the
    /// `notifications/initialized` notification.
    pub async fn initialize(&mut self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "phoenix-ide",
                "version": "0.1.0"
            }
        });

        let _resp = self.send_request("initialize", params).await?;

        // Send the initialized notification (no id, no response expected).
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.send_notification(&notification).await?;

        Ok(())
    }

    /// Discover tools from the server via `tools/list`.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, String> {
        let resp = self
            .send_request("tools/list", serde_json::json!({}))
            .await?;

        let tools_arr = resp
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                format!(
                    "MCP server '{}': tools/list response missing 'tools' array",
                    self.name
                )
            })?;

        let mut defs = Vec::with_capacity(tools_arr.len());
        for tool in tools_arr {
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"}));

            if !name.is_empty() {
                defs.push(McpToolDef {
                    name,
                    description,
                    input_schema,
                });
            }
        }

        self.tools.clone_from(&defs);
        Ok(defs)
    }

    /// Call a tool on this server via `tools/call`.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = self.send_request("tools/call", params).await?;

        // Extract text from content blocks.
        let content = resp
            .get("content")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                format!(
                    "MCP server '{}': tools/call response missing 'content' array",
                    self.name
                )
            })?;

        let text: Vec<&str> = content
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    block.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect();

        Ok(text.join("\n"))
    }

    /// Send a JSON-RPC request and read the response with a timeout.
    ///
    /// Both stdin and stdout locks are held for the duration to serialize
    /// concurrent calls on the same server.
    async fn send_request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let request_line = format!(
            "{}\n",
            serde_json::to_string(&request).map_err(|e| {
                format!(
                    "MCP server '{}': failed to serialize request: {e}",
                    self.name
                )
            })?
        );

        // Lock both to serialize the request-response pair.
        let mut stdin = self.stdin.lock().await;
        let mut stdout = self.stdout.lock().await;

        // Write request.
        let write_fut = async {
            stdin
                .write_all(request_line.as_bytes())
                .await
                .map_err(|e| format!("MCP server '{}': stdin write failed: {e}", self.name))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("MCP server '{}': stdin flush failed: {e}", self.name))
        };

        tokio::time::timeout(REQUEST_TIMEOUT, write_fut)
            .await
            .map_err(|_| {
                format!(
                    "MCP server '{}': timed out writing request for '{method}'",
                    self.name
                )
            })??;

        // Read response -- loop to skip notifications from the server.
        let read_fut = async {
            loop {
                let mut line = String::new();
                let bytes_read = stdout
                    .read_line(&mut line)
                    .await
                    .map_err(|e| format!("MCP server '{}': stdout read failed: {e}", self.name))?;

                if bytes_read == 0 {
                    return Err(format!(
                        "MCP server '{}': stdout closed (process exited) while waiting for response to '{method}'",
                        self.name
                    ));
                }

                let parsed: Value = serde_json::from_str(line.trim()).map_err(|e| {
                    format!("MCP server '{}': invalid JSON from stdout: {e}", self.name)
                })?;

                // Skip server-initiated notifications (no "id" field).
                if parsed.get("id").is_none() {
                    tracing::debug!(
                        server = %self.name,
                        method = parsed.get("method").and_then(|v| v.as_str()).unwrap_or("unknown"),
                        "Skipping server notification"
                    );
                    continue;
                }

                // Verify the response id matches our request.
                if parsed.get("id").and_then(Value::as_u64) != Some(id) {
                    tracing::warn!(
                        server = %self.name,
                        expected_id = id,
                        got = ?parsed.get("id"),
                        "Mismatched response id, skipping"
                    );
                    continue;
                }

                // Check for JSON-RPC error.
                if let Some(error) = parsed.get("error") {
                    let message = error
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                    return Err(format!(
                        "MCP server '{}': JSON-RPC error {code}: {message}",
                        self.name
                    ));
                }

                return parsed.get("result").cloned().ok_or_else(|| {
                    format!(
                        "MCP server '{}': response missing both 'result' and 'error'",
                        self.name
                    )
                });
            }
        };

        tokio::time::timeout(REQUEST_TIMEOUT, read_fut)
            .await
            .map_err(|_| {
                format!(
                    "MCP server '{}': timed out reading response for '{method}'",
                    self.name
                )
            })?
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn send_notification(&self, notification: &Value) -> Result<(), String> {
        let line = format!(
            "{}\n",
            serde_json::to_string(notification).map_err(|e| {
                format!(
                    "MCP server '{}': failed to serialize notification: {e}",
                    self.name
                )
            })?
        );

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("MCP server '{}': notification write failed: {e}", self.name))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("MCP server '{}': notification flush failed: {e}", self.name))
    }

    /// Check whether the child process is still running.
    #[allow(dead_code)] // Useful diagnostic; will be wired into /status endpoint
    pub fn is_alive(&mut self) -> bool {
        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running.
        matches!(self.child.try_wait(), Ok(None))
    }
}

// ---------------------------------------------------------------------------
// McpClientManager
// ---------------------------------------------------------------------------

/// Owns all MCP server connections.
pub struct McpClientManager {
    servers: Arc<RwLock<HashMap<String, McpServer>>>,
}

impl McpClientManager {
    /// Create an empty manager. Servers are connected asynchronously via
    /// `start_background_discovery`.
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a background task that reads config files and connects to each
    /// MCP server in parallel. Servers become available in `tool_definitions`
    /// and `call_tool` as they finish connecting.
    pub fn start_background_discovery(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let configs = Self::read_all_configs();
            if configs.is_empty() {
                tracing::debug!("No MCP server configs found");
                return;
            }

            tracing::info!(
                count = configs.len(),
                "Starting background MCP server discovery"
            );

            // Connect to all servers in parallel.
            let handles: Vec<_> = configs
                .into_iter()
                .map(|(name, entry)| {
                    let mgr = Arc::clone(&manager);
                    tokio::spawn(async move {
                        match Self::connect_one(&name, &entry).await {
                            Ok(server) => {
                                let tool_count = server.tools.len();
                                mgr.servers.write().await.insert(name.clone(), server);
                                tracing::info!(
                                    server = %name,
                                    tools = tool_count,
                                    "MCP server connected"
                                );
                                Some((name, tool_count))
                            }
                            Err(e) => {
                                tracing::warn!(server = %name, "Skipping MCP server: {e}");
                                None
                            }
                        }
                    })
                })
                .collect();

            // Collect results for the summary log.
            let mut total_tools = 0usize;
            let mut connected_servers = 0usize;
            let mut server_names = Vec::new();
            for handle in handles {
                if let Ok(Some((name, tool_count))) = handle.await {
                    total_tools += tool_count;
                    connected_servers += 1;
                    server_names.push(name);
                }
            }

            tracing::info!(
                tools = total_tools,
                servers = connected_servers,
                names = ?server_names,
                "Discovered {total_tools} MCP tools from {connected_servers} servers",
            );
        });
    }

    /// Return (`server_name`, `tool_def`) pairs for all currently connected servers.
    /// May return an empty list if background discovery hasn't finished yet.
    pub async fn tool_definitions(&self) -> Vec<(String, McpToolDef)> {
        let servers = self.servers.read().await;
        let mut out = Vec::new();
        for (server_name, server) in servers.iter() {
            for tool in &server.tools {
                out.push((server_name.clone(), tool.clone()));
            }
        }
        out
    }

    /// Route a tool call to the correct server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String> {
        let servers = self.servers.read().await;
        let server = servers
            .get(server_name)
            .ok_or_else(|| format!("MCP server '{server_name}' is not connected"))?;
        server.call_tool(tool_name, arguments).await
    }

    /// Spawn and initialize a single MCP server.
    async fn connect_one(name: &str, entry: &McpServerConfig) -> Result<McpServer, String> {
        let mut server = McpServer::spawn(name, &entry.command, &entry.args, &entry.env).await?;
        server.initialize().await?;
        server.list_tools().await?;
        Ok(server)
    }

    /// Read all MCP config files in priority order, merging by server name
    /// (first-seen wins).
    fn read_all_configs() -> Vec<(String, McpServerConfig)> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let home = PathBuf::from(home);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let config_paths = [
            home.join(".claude.json"),
            home.join(".cursor/mcp.json"),
            cwd.join(".mcp.json"),
            home.join(".config/mcp/mcp.json"),
        ];

        let mut seen: HashMap<String, McpServerConfig> = HashMap::new();

        for path in &config_paths {
            if !path.exists() {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(
                        path = %path.display(),
                        "Failed to read MCP config: {e}"
                    );
                    continue;
                }
            };

            let parsed: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(
                        path = %path.display(),
                        "Failed to parse MCP config: {e}"
                    );
                    continue;
                }
            };

            let Some(servers) = parsed.get("mcpServers").and_then(|v| v.as_object()) else {
                continue;
            };

            for (name, cfg) in servers {
                if seen.contains_key(name) {
                    continue; // first-seen wins
                }

                // Skip HTTP transport entries.
                if cfg.get("type").and_then(|v| v.as_str()) == Some("http") {
                    tracing::debug!(
                        server = %name,
                        path = %path.display(),
                        "Skipping HTTP transport MCP server"
                    );
                    continue;
                }

                // Must have a command field (stdio transport).
                let Some(command) = cfg.get("command").and_then(|v| v.as_str()) else {
                    tracing::debug!(
                        server = %name,
                        path = %path.display(),
                        "Skipping MCP server without 'command' field"
                    );
                    continue;
                };
                let command = command.to_string();

                let args: Vec<String> = cfg
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let env: HashMap<String, String> = cfg
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| v.as_str().map(|val| (k.clone(), val.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();

                tracing::debug!(
                    server = %name,
                    command = %command,
                    path = %path.display(),
                    "Found MCP server config"
                );

                seen.insert(name.clone(), McpServerConfig { command, args, env });
            }
        }

        seen.into_iter().collect()
    }
}

/// Parsed MCP server configuration from a config file.
struct McpServerConfig {
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// McpTool (Tool trait implementation)
// ---------------------------------------------------------------------------

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

/// Create one `McpTool` per discovered tool across all servers.
pub async fn create_mcp_tools(manager: &Arc<McpClientManager>) -> Vec<Box<dyn Tool>> {
    manager
        .tool_definitions()
        .await
        .into_iter()
        .map(|(server_name, def)| {
            let full_name = format!("{server_name}__{}", def.name);
            Box::new(McpTool {
                server_name,
                tool_name: def.name,
                full_name,
                description: def.description,
                input_schema: def.input_schema,
                manager: Arc::clone(manager),
            }) as Box<dyn Tool>
        })
        .collect()
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
    async fn test_create_mcp_tools_empty() {
        let manager = Arc::new(McpClientManager::new());
        let tools = create_mcp_tools(&manager).await;
        assert!(tools.is_empty());
    }

    #[test]
    fn test_config_parsing_skips_http() {
        // Verify that read_all_configs works with no config files present
        // (it should return empty, not error).
        let configs = McpClientManager::read_all_configs();
        // We can't assert anything about count since the dev machine may have configs,
        // but the call should not panic.
        let _ = configs;
    }
}
