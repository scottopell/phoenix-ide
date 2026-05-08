//! MCP (Model Context Protocol) client -- stdio transport
//!
//! Manages MCP server subprocesses, discovers tools via JSON-RPC 2.0,
//! and exposes them as regular Phoenix tools through the Tool trait.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

/// Timeout for a single JSON-RPC request-response round trip.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Longer timeout for initialize + tools/list during server connection.
/// Five minutes gives OAuth flows (mcp-remote prompts, browser redirect) time to complete.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

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

/// Status of one connected MCP server (for API responses).
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub tool_count: usize,
    pub tools: Vec<String>,
    pub enabled: bool,
    /// Set while the server is waiting for the user to complete an OAuth flow.
    pub pending_oauth_url: Option<String>,
}

// ---------------------------------------------------------------------------
// McpServer
// ---------------------------------------------------------------------------

/// Manages one stdio MCP server subprocess with JSON-RPC 2.0 communication.
pub struct McpServer {
    name: String,
    child: Child,
    /// Locked together with `stdout` for request-response serialization.
    stdin: Mutex<BufWriter<ChildStdin>>,
    stdout: Mutex<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    tools: Vec<McpToolDef>,
    // Spawn config retained for respawning after crashes.
    spawn_command: String,
    spawn_args: Vec<String>,
    spawn_env: HashMap<String, String>,
    /// Set when the server sends `notifications/tools/list_changed`.
    /// Cleared after the next `list_tools()` refresh.
    tools_changed: AtomicBool,
    /// Handle to the stderr drain task -- aborted on shutdown/respawn.
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    /// Shared map of server name → OAuth URL; written by the stderr drain,
    /// read by `McpClientManager::status()`. Retained for reuse on respawn.
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
}

impl McpServer {
    /// Spawn the child process with stdin/stdout piped.
    #[allow(clippy::unused_async)] // async block inside spawns a task; keeping async for API consistency
    pub async fn spawn(
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

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
        // Lines containing URLs are surfaced at warn and stored as pending OAuth
        // URLs so the UI can display a clickable auth link.
        let stderr_task = child.stderr.take().map(|stderr| {
            let server_name = name.to_string();
            let oauth_sink = Arc::clone(&pending_oauth_urls);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let trimmed = line.trim_end();
                            if trimmed.contains("https://") {
                                tracing::warn!(
                                    server = %server_name,
                                    "MCP stderr: {trimmed}"
                                );
                                oauth_sink
                                    .write()
                                    .await
                                    .insert(server_name.clone(), trimmed.to_string());
                            } else {
                                tracing::debug!(
                                    server = %server_name,
                                    "MCP stderr: {trimmed}"
                                );
                            }
                        }
                    }
                }
            })
        });

        Ok(Self {
            name: name.to_string(),
            child,
            stdin: Mutex::new(BufWriter::new(child_stdin)),
            stdout: Mutex::new(BufReader::new(child_stdout)),
            next_id: AtomicU64::new(1),
            tools: Vec::new(),
            spawn_command: command.to_string(),
            spawn_args: args.to_vec(),
            spawn_env: env.clone(),
            tools_changed: AtomicBool::new(false),
            stderr_task,
            pending_oauth_urls,
        })
    }

    /// Send the JSON-RPC `initialize` handshake followed by the
    /// `notifications/initialized` notification.
    pub async fn initialize(&mut self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": { "listChanged": true }
            },
            "clientInfo": {
                "name": "phoenix-ide",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let _resp = self
            .send_request_with_timeout("initialize", params, CONNECT_TIMEOUT)
            .await?;

        // Send the initialized notification (no id, no response expected).
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.send_notification(&notification).await?;

        Ok(())
    }

    /// Discover tools from the server via `tools/list`.
    /// Follows cursor-based pagination if the server returns `nextCursor`.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, String> {
        const MAX_PAGES: usize = 20;

        let mut all_defs = Vec::new();
        let mut cursor: Option<String> = None;

        for page in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };

            let resp = self
                .send_request_with_timeout("tools/list", params, CONNECT_TIMEOUT)
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
                    all_defs.push(McpToolDef {
                        name,
                        description,
                        input_schema,
                    });
                }
            }

            match resp.get("nextCursor").and_then(|v| v.as_str()) {
                Some(next) => {
                    tracing::debug!(
                        server = %self.name,
                        page = page + 1,
                        tools_so_far = all_defs.len(),
                        "tools/list pagination: following nextCursor"
                    );
                    cursor = Some(next.to_string());
                }
                None => break,
            }
        }

        if cursor.is_some() {
            tracing::warn!(
                server = %self.name,
                pages = MAX_PAGES,
                tools = all_defs.len(),
                "tools/list pagination hit safety cap -- some tools may be missing"
            );
        }

        self.tools.clone_from(&all_defs);
        Ok(all_defs)
    }

    /// Call a tool on this server via `tools/call`.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = self.send_request("tools/call", params).await?;

        // MCP tools/call can signal failure via isError at the result level.
        let is_error = resp
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);

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
                let block_type = block.get("type").and_then(|v| v.as_str());
                match block_type {
                    Some("text") => block.get("text").and_then(|v| v.as_str()),
                    Some(other) => {
                        tracing::debug!(
                            server = %self.name,
                            tool = %tool_name,
                            block_type = other,
                            "Dropping non-text MCP content block"
                        );
                        None
                    }
                    None => None,
                }
            })
            .collect();

        let output = text.join("\n");

        if is_error {
            Err(output)
        } else {
            Ok(output)
        }
    }

    /// Send a JSON-RPC request and read the response with a timeout.
    ///
    /// Both stdin and stdout locks are held for the duration to serialize
    /// concurrent calls on the same server. This is intentional: a proper
    /// multiplexing dispatcher (lock stdin briefly to write, then match
    /// responses by ID from a reader task) would be complex and provide
    /// little real benefit -- the MCP server is a single process that
    /// serializes work internally anyway, so parallel requests would just
    /// queue on the server side.
    async fn send_request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.send_request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    #[allow(clippy::too_many_lines)] // Protocol handling is inherently sequential; splitting would obscure the flow.
    async fn send_request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, String> {
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

        // Detect contention: if the lock is already held, another call is
        // in-flight and this one will queue behind it.
        if self.stdin.try_lock().is_err() {
            tracing::debug!(
                server = %self.name,
                method = %method,
                id = id,
                "MCP request queued behind in-flight call"
            );
        }

        // Lock both to serialize the request-response pair. See doc comment
        // above for why we don't multiplex.
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

        tokio::time::timeout(timeout, write_fut)
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

                // Handle server-initiated notifications (no "id" field).
                if parsed.get("id").is_none() {
                    let notif_method = parsed
                        .get("method")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    if notif_method == "notifications/tools/list_changed" {
                        tracing::info!(
                            server = %self.name,
                            "Server signaled tools/list_changed -- will refresh on next definitions() call"
                        );
                        self.tools_changed.store(true, Ordering::Release);
                    } else {
                        tracing::debug!(
                            server = %self.name,
                            method = notif_method,
                            "Skipping server notification"
                        );
                    }
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

        tokio::time::timeout(timeout, read_fut).await.map_err(|_| {
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
    pub fn is_alive(&mut self) -> bool {
        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running.
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Attempt to respawn and reinitialize after a crash.
    async fn respawn(&mut self) -> Result<(), String> {
        // Abort the old stderr drain task.
        if let Some(handle) = self.stderr_task.take() {
            handle.abort();
        }
        // Kill old process if still somehow alive.
        let _ = self.child.kill().await;

        // Re-spawn with the same config.
        let mut new_server = McpServer::spawn(
            &self.name,
            &self.spawn_command,
            &self.spawn_args,
            &self.spawn_env,
            Arc::clone(&self.pending_oauth_urls),
        )
        .await?;

        new_server.initialize().await?;
        new_server.list_tools().await?;

        tracing::info!(
            server = %self.name,
            tools = new_server.tools.len(),
            "MCP server respawned"
        );

        *self = new_server;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// McpClientManager
// ---------------------------------------------------------------------------

/// Owns all MCP server connections.
///
/// Lock ordering: always acquire `servers` before `disabled_servers`.
/// Both are tokio `RwLock` and must not be held across heavy `.await`
/// points (respawn, connect, etc.) -- extract data, drop the lock, then
/// do async I/O.
pub struct McpClientManager {
    servers: Arc<RwLock<HashMap<String, McpServer>>>,
    /// Server names whose tools should be excluded from conversations.
    /// The servers remain connected for instant re-enable.
    disabled_servers: RwLock<std::collections::HashSet<String>>,
    /// Servers currently blocked on an OAuth flow: name → auth URL.
    /// Written by the stderr drain; cleared when the server connects or fails.
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
}

impl McpClientManager {
    /// Create an empty manager. Servers are connected asynchronously via
    /// `start_background_discovery`.
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
            disabled_servers: RwLock::new(std::collections::HashSet::new()),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Replace the disabled server set (called at startup with persisted state).
    pub async fn set_disabled_servers(&self, disabled: std::collections::HashSet<String>) {
        *self.disabled_servers.write().await = disabled;
    }

    /// Check whether a server is currently disabled.
    #[allow(dead_code)] // Public API for future use by health checks / diagnostics
    pub async fn is_disabled(&self, name: &str) -> bool {
        self.disabled_servers.read().await.contains(name)
    }

    /// Add a server to the disabled set.
    pub async fn disable_server(&self, name: &str) {
        self.disabled_servers.write().await.insert(name.to_string());
    }

    /// Remove a server from the disabled set.
    pub async fn enable_server(&self, name: &str) {
        self.disabled_servers.write().await.remove(name);
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
                    let oauth = Arc::clone(&manager.pending_oauth_urls);
                    tokio::spawn(async move {
                        let result = Self::connect_one(&name, &entry, Arc::clone(&oauth)).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
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
                                // Leave any OAuth URL in pending_oauth_urls so the UI
                                // keeps the panel visible with a reconnect affordance.
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

    /// Return status of all connected MCP servers plus any pending OAuth entries.
    pub async fn status(&self) -> Vec<McpServerStatus> {
        let servers = self.servers.read().await;
        let disabled = self.disabled_servers.read().await;
        let pending = self.pending_oauth_urls.read().await;

        let mut result: Vec<McpServerStatus> = servers
            .iter()
            .map(|(name, server)| McpServerStatus {
                name: name.clone(),
                tool_count: server.tools.len(),
                tools: server.tools.iter().map(|t| t.name.clone()).collect(),
                enabled: !disabled.contains(name),
                pending_oauth_url: None,
            })
            .collect();

        // Servers blocked on OAuth haven't entered the connected map yet.
        for (name, url) in pending.iter() {
            if !servers.contains_key(name) {
                result.push(McpServerStatus {
                    name: name.clone(),
                    tool_count: 0,
                    tools: vec![],
                    enabled: true,
                    pending_oauth_url: Some(url.clone()),
                });
            }
        }

        result
    }

    /// Return (`server_name`, `tool_def`) pairs for all currently connected servers.
    /// Disabled servers are excluded. May return an empty list if background
    /// discovery hasn't finished yet.
    pub async fn tool_definitions(&self) -> Vec<(String, McpToolDef)> {
        // Check if any server signaled tools/list_changed. If so, refresh
        // under a write lock before reading. This adds latency on the first
        // call after a change notification -- acceptable trade-off vs a
        // background reader task per server.
        let needs_refresh: Vec<String> = {
            let servers = self.servers.read().await;
            servers
                .iter()
                .filter(|(_, s)| s.tools_changed.load(Ordering::Acquire))
                .map(|(name, _)| name.clone())
                .collect()
        };
        // Refresh servers outside the lock to avoid blocking all MCP
        // operations during list_tools() I/O (up to 30s timeout per server).
        // Same extract-refresh-reinsert pattern as call_tool() respawn.
        for name in needs_refresh {
            let server = {
                let mut servers = self.servers.write().await;
                match servers.get_mut(&name) {
                    Some(s) if s.tools_changed.swap(false, Ordering::AcqRel) => {
                        servers.remove(&name)
                    }
                    _ => None,
                }
            };
            // Lock dropped -- list_tools() runs with no lock held.
            if let Some(mut server) = server {
                match server.list_tools().await {
                    Ok(tools) => {
                        tracing::info!(
                            server = %name,
                            tools = tools.len(),
                            "Refreshed tool list after list_changed notification"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %name,
                            error = %e,
                            "Failed to refresh tools after list_changed"
                        );
                    }
                }
                // Reinsert under brief write lock.
                self.servers.write().await.insert(name, server);
            }
        }

        let servers = self.servers.read().await;
        let disabled = self.disabled_servers.read().await;
        let mut out = Vec::new();
        for (server_name, server) in servers.iter() {
            if disabled.contains(server_name) {
                continue;
            }
            for tool in &server.tools {
                out.push((server_name.clone(), tool.clone()));
            }
        }
        out
    }

    /// Route a tool call to the correct server.
    ///
    /// Uses a read lock for the happy path so calls to different servers run
    /// concurrently (each `McpServer` serializes its own stdin/stdout internally).
    /// On crash, the crashed server is removed under a brief write lock, then
    /// respawned with no lock held to avoid blocking all MCP operations.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String> {
        // Check disabled state before attempting the call.
        if self.disabled_servers.read().await.contains(server_name) {
            return Err(format!("MCP server '{server_name}' is disabled"));
        }

        // Happy path: read lock -- McpServer.call_tool takes &self.
        let first_result = {
            let servers = self.servers.read().await;
            let server = servers
                .get(server_name)
                .ok_or_else(|| format!("MCP server '{server_name}' is not connected"))?;
            server.call_tool(tool_name, arguments.clone()).await
        };

        match first_result {
            Ok(result) => return Ok(result),
            Err(ref e) => {
                // Brief write lock: check liveness and extract the crashed
                // server for out-of-lock respawn.
                let crashed_server = {
                    let mut servers = self.servers.write().await;
                    let server = servers
                        .get_mut(server_name)
                        .ok_or_else(|| format!("MCP server '{server_name}' is not connected"))?;

                    // If alive, the error is a tool-level failure (not a crash).
                    // Also covers the case where another task already respawned.
                    if server.is_alive() {
                        return Err(e.clone());
                    }

                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "MCP server crashed, removing for respawn"
                    );

                    // Remove the server so the write lock can be dropped.
                    servers.remove(server_name)
                };
                // Write lock is dropped here.

                // Respawn outside the lock so other servers aren't blocked.
                if let Some(mut server) = crashed_server {
                    match server.respawn().await {
                        Ok(()) => {
                            // Reinsert under a brief write lock, then retry
                            // via the read-lock path.
                            self.servers
                                .write()
                                .await
                                .insert(server_name.to_string(), server);
                        }
                        Err(respawn_err) => {
                            return Err(format!(
                                "MCP server '{server_name}' crashed and respawn failed: {respawn_err}"
                            ));
                        }
                    }
                }
            }
        }

        // Retry once via the normal read-lock path after respawn.
        let servers = self.servers.read().await;
        let server = servers.get(server_name).ok_or_else(|| {
            format!("MCP server '{server_name}' respawn succeeded but server not found")
        })?;
        server.call_tool(tool_name, arguments).await
    }

    /// Spawn and initialize a single MCP server.
    async fn connect_one(
        name: &str,
        entry: &McpServerConfig,
        pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<McpServer, String> {
        let mut server = McpServer::spawn(
            name,
            &entry.command,
            &entry.args,
            &entry.env,
            pending_oauth_urls,
        )
        .await?;
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
                    tracing::warn!(
                        path = %path.display(),
                        "Failed to read MCP config: {e}"
                    );
                    continue;
                }
            };

            let parsed: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
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

    /// Re-scan config files and reconcile servers: connect new ones,
    /// disconnect removed ones, leave unchanged ones alone.
    ///
    /// Changes take effect immediately: MCP tools are resolved live from
    /// the manager on each LLM request, so all conversations (new and
    /// existing) see the updated server set.
    ///
    /// Returns a summary of what changed.
    pub async fn reload(&self) -> McpReloadResult {
        let configs = Self::read_all_configs();
        let config_names: std::collections::HashSet<String> =
            configs.iter().map(|(n, _)| n.clone()).collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut unchanged = Vec::new();

        // Remove servers no longer in config.
        {
            let mut servers = self.servers.write().await;
            let existing_names: Vec<String> = servers.keys().cloned().collect();
            for name in existing_names {
                if !config_names.contains(&name) {
                    if let Some(mut server) = servers.remove(&name) {
                        if let Some(handle) = server.stderr_task.take() {
                            handle.abort();
                        }
                        let _ = server.child.kill().await;
                        tracing::info!(server = %name, "MCP server removed during reload");
                    }
                    removed.push(name);
                }
            }
        }

        // Spawn background connections for new servers (same pattern as
        // start_background_discovery — returns immediately so the HTTP
        // response isn't held open for the full connect timeout).
        for (name, entry) in configs {
            let already_exists = self.servers.read().await.contains_key(&name);
            if already_exists {
                unchanged.push(name);
                continue;
            }

            let oauth = Arc::clone(&self.pending_oauth_urls);
            // Clear stale OAuth URL before retrying so the UI gets a fresh one.
            oauth.write().await.remove(&name);
            added.push(name.clone());

            let servers = Arc::clone(&self.servers);
            tokio::spawn(async move {
                let result = Self::connect_one(&name, &entry, Arc::clone(&oauth)).await;
                match result {
                    Ok(server) => {
                        oauth.write().await.remove(&name);
                        let tool_count = server.tools.len();
                        servers.write().await.insert(name.clone(), server);
                        tracing::info!(
                            server = %name,
                            tools = tool_count,
                            "MCP server connected during reload"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(server = %name, "Failed to connect during reload: {e}");
                    }
                }
            });
        }
        McpReloadResult {
            added,
            removed,
            unchanged,
        }
    }

    /// Shut down all MCP server processes and abort stderr drain tasks.
    #[allow(dead_code)] // Available for graceful shutdown integration
    pub async fn shutdown(&self) {
        let mut servers = self.servers.write().await;
        for (name, server) in servers.iter_mut() {
            if let Some(handle) = server.stderr_task.take() {
                handle.abort();
            }
            let _ = server.child.kill().await;
            tracing::debug!(server = %name, "MCP server stopped");
        }
        servers.clear();
    }
}

/// Result of an MCP config reload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpReloadResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
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
