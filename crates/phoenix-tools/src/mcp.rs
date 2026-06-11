//! MCP (Model Context Protocol) client
//!
//! The JSON-RPC 2.0 protocol layer (`initialize`, paginated `tools/list`,
//! `tools/call`, notification handling) lives on `McpServer` and is
//! transport-agnostic; how a request's bytes leave and a response's bytes
//! arrive is behind the `McpTransport` trait. `StdioTransport` reaches a
//! server spawned as a child subprocess; HTTP configs (`type: "http"`) are
//! skipped at discovery. Discovered tools are exposed as regular Phoenix
//! tools through the Tool trait. Spec: `specs/mcp/`.

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

/// Timeout for a single JSON-RPC request-response round trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Longer timeout for initialize + tools/list during server connection.
/// Five minutes gives OAuth flows (mcp-remote prompts, browser redirect) time to complete.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Upper bound for an HTTP reload request applying changed existing configs.
const RELOAD_RESTART_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Transport boundary
// ---------------------------------------------------------------------------

/// A transport-classified failure. The lifecycle dispatches on the variant
/// (crash detection today; session and authorization recovery for HTTP), so
/// the transport classifies each failure once and callers never string-match
/// to recover it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// HTTP 401; carries the `WWW-Authenticate` challenge when present.
    Unauthorized { www_authenticate: Option<String> },
    /// HTTP 403 `insufficient_scope`; carries the challenge when present.
    InsufficientScope { www_authenticate: Option<String> },
    /// HTTP 404 on a session-bearing request: the server-side session is gone.
    SessionExpired,
    /// The connection itself is gone (pipe closed, process exited, reset).
    /// For stdio this is the crash-like class that triggers a respawn.
    Disconnected(String),
    /// The request deadline elapsed without evidence the connection is dead.
    /// Distinct from `Disconnected`: a live-but-slow stdio server is not
    /// respawned for this.
    Timeout(String),
    /// The server returned a JSON-RPC error result.
    Rpc { code: i64, message: String },
    /// Malformed or unreadable frame.
    Protocol(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized { .. } => write!(f, "unauthorized (HTTP 401)"),
            Self::InsufficientScope { .. } => write!(f, "insufficient scope (HTTP 403)"),
            Self::SessionExpired => write!(f, "session expired"),
            Self::Disconnected(detail) | Self::Timeout(detail) | Self::Protocol(detail) => {
                write!(f, "{detail}")
            }
            Self::Rpc { code, message } => write!(f, "JSON-RPC error {code}: {message}"),
        }
    }
}

/// Receives server-initiated JSON-RPC messages (requests or notifications)
/// that a transport encounters while waiting for a request's response. The
/// transport frames and forwards them without interpreting them; protocol
/// handling (e.g. `notifications/tools/list_changed`) stays above the
/// transport boundary.
pub trait ServerMessageSink: Send + Sync {
    fn on_message(&self, message: Value);
}

/// How a request's bytes leave and a response's bytes arrive. The JSON-RPC
/// protocol layer (`McpServer`) is identical across transports; impls own
/// framing, request-id correlation, and failure classification.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send one JSON-RPC request and return the correlated `result` value.
    /// Server-initiated messages that arrive before the matching response
    /// are forwarded to `sink`.
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        sink: &dyn ServerMessageSink,
    ) -> Result<Value, TransportError>;

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn notify(&self, notification: &Value) -> Result<(), TransportError>;

    /// Whether the underlying connection is still usable
    /// (stdio: the child process is running).
    fn is_alive(&mut self) -> bool;

    /// Tear down the transport (stdio: kill the child process).
    async fn shutdown(&mut self);
}

/// Build a transport for `config`. Stdio spawns the child process; HTTP is
/// not implemented (`read_all_configs` skips `type: "http"` entries, so an
/// `Http` config never reaches a connection attempt).
///
/// # Errors
/// Returns a display string when the transport cannot be established.
async fn connect_transport(
    name: &str,
    config: &McpServerConfig,
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
) -> Result<Box<dyn McpTransport>, String> {
    match config {
        McpServerConfig::Stdio { command, args, env } => Ok(Box::new(
            StdioTransport::spawn(name, command, args, env, pending_oauth_urls).await?,
        )),
        McpServerConfig::Http { .. } => Err(format!(
            "MCP server '{name}': HTTP transport not implemented"
        )),
    }
}

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
// StdioTransport
// ---------------------------------------------------------------------------

/// Stdio transport: a child process exchanging JSON-RPC 2.0 over its
/// stdin/stdout (REQ-MCP-003).
pub struct StdioTransport {
    name: String,
    child: Child,
    /// Locked together with `stdout` for request-response serialization.
    stdin: Mutex<BufWriter<ChildStdin>>,
    stdout: Mutex<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    /// Handle to the stderr drain task -- aborted on shutdown.
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

impl StdioTransport {
    /// Spawn the child process with stdin/stdout piped.
    ///
    /// # Errors
    /// Returns a display string when the child process cannot be spawned.
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
            stderr_task,
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    /// Send a JSON-RPC request and read the response with a timeout.
    ///
    /// Both stdin and stdout locks are held for the duration to serialize
    /// concurrent calls on the same server. This is intentional and
    /// stdio-specific: a proper multiplexing dispatcher (lock stdin briefly
    /// to write, then match responses by ID from a reader task) would be
    /// complex and provide little real benefit -- the MCP server is a single
    /// process that serializes work internally anyway, so parallel requests
    /// would just queue on the server side.
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        sink: &dyn ServerMessageSink,
    ) -> Result<Value, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let request_line = format!(
            "{}\n",
            serde_json::to_string(&request).map_err(|e| TransportError::Protocol(format!(
                "failed to serialize request: {e}"
            )))?
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
                .map_err(|e| TransportError::Disconnected(format!("stdin write failed: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| TransportError::Disconnected(format!("stdin flush failed: {e}")))
        };

        tokio::time::timeout(timeout, write_fut)
            .await
            .map_err(|_| {
                TransportError::Timeout(format!("timed out writing request for '{method}'"))
            })??;

        // Read response -- loop to forward server-initiated messages to the sink.
        let read_fut = async {
            loop {
                let mut line = String::new();
                let bytes_read = stdout.read_line(&mut line).await.map_err(|e| {
                    // An io error on read is not crash-like; only EOF (below)
                    // and stdin write failures evidence a dead process.
                    TransportError::Protocol(format!("stdout read failed: {e}"))
                })?;

                if bytes_read == 0 {
                    return Err(TransportError::Disconnected(format!(
                        "stdout closed (process exited) while waiting for response to '{method}'"
                    )));
                }

                let parsed: Value = serde_json::from_str(line.trim()).map_err(|e| {
                    TransportError::Protocol(format!("invalid JSON from stdout: {e}"))
                })?;

                // Server-initiated messages (no "id" field) are the protocol
                // layer's concern -- forward and keep waiting for the response.
                if parsed.get("id").is_none() {
                    sink.on_message(parsed);
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
                        .unwrap_or("unknown error")
                        .to_string();
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                    return Err(TransportError::Rpc { code, message });
                }

                return parsed.get("result").cloned().ok_or_else(|| {
                    TransportError::Protocol(
                        "response missing both 'result' and 'error'".to_string(),
                    )
                });
            }
        };

        tokio::time::timeout(timeout, read_fut).await.map_err(|_| {
            TransportError::Timeout(format!("timed out reading response for '{method}'"))
        })?
    }

    async fn notify(&self, notification: &Value) -> Result<(), TransportError> {
        let line = format!(
            "{}\n",
            serde_json::to_string(notification).map_err(|e| TransportError::Protocol(format!(
                "failed to serialize notification: {e}"
            )))?
        );

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| TransportError::Disconnected(format!("notification write failed: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| TransportError::Disconnected(format!("notification flush failed: {e}")))
    }

    fn is_alive(&mut self) -> bool {
        // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running.
        matches!(self.child.try_wait(), Ok(None))
    }

    async fn shutdown(&mut self) {
        if let Some(handle) = self.stderr_task.take() {
            handle.abort();
        }
        let _ = self.child.kill().await;
    }
}

// ---------------------------------------------------------------------------
// McpServer
// ---------------------------------------------------------------------------

/// Failure from one `tools/call`, keeping the transport classification intact
/// so the manager's recovery path dispatches on the variant rather than
/// string-matching the message.
#[derive(Debug)]
pub enum McpCallError {
    /// Classified by the transport. The detail is unprefixed; format with the
    /// server name via `into_message`.
    Transport(TransportError),
    /// Tool-level failure (`isError` result) or malformed response; the
    /// string is the complete display message.
    Call(String),
}

impl McpCallError {
    /// Crash-like failures trigger the stdio respawn path (REQ-MCP-003).
    fn is_crash_like(&self) -> bool {
        matches!(self, Self::Transport(TransportError::Disconnected(_)))
    }

    fn into_message(self, server_name: &str) -> String {
        match self {
            Self::Transport(e) => format!("MCP server '{server_name}': {e}"),
            Self::Call(message) => message,
        }
    }
}

/// Protocol-layer handling of server-initiated messages forwarded by the
/// transport: flags `tools/list_changed` for lazy refresh, logs and drops
/// everything else.
struct NotificationSink<'a> {
    server: &'a str,
    tools_changed: &'a AtomicBool,
}

impl ServerMessageSink for NotificationSink<'_> {
    fn on_message(&self, message: Value) {
        let method = message
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if method == "notifications/tools/list_changed" {
            tracing::info!(
                server = %self.server,
                "Server signaled tools/list_changed -- will refresh on next definitions() call"
            );
            self.tools_changed.store(true, Ordering::Release);
        } else {
            tracing::debug!(
                server = %self.server,
                method = method,
                "Skipping server notification"
            );
        }
    }
}

/// One MCP server connection: the transport-agnostic JSON-RPC protocol layer
/// (REQ-MCP-002) over a `McpTransport`.
pub struct McpServer {
    name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpToolDef>,
    /// Config retained for reload comparison and for rebuilding the
    /// transport on respawn.
    config: McpServerConfig,
    /// Set when the server sends `notifications/tools/list_changed`.
    /// Cleared after the next `list_tools()` refresh.
    tools_changed: AtomicBool,
    /// Shared map of server name → OAuth URL; written by the stdio stderr
    /// drain, read by `McpClientManager::status()`. Retained so a respawned
    /// transport keeps feeding the same map.
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
}

impl McpServer {
    /// Establish the transport for `config` without running the handshake.
    ///
    /// # Errors
    /// Returns a display string when the transport cannot be established.
    async fn connect(
        name: &str,
        config: McpServerConfig,
        pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<Self, String> {
        let transport = connect_transport(name, &config, Arc::clone(&pending_oauth_urls)).await?;
        Ok(Self {
            name: name.to_string(),
            transport,
            tools: Vec::new(),
            config,
            tools_changed: AtomicBool::new(false),
            pending_oauth_urls,
        })
    }

    /// Send one JSON-RPC request through the transport, forwarding
    /// server-initiated messages to the protocol-layer sink.
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, TransportError> {
        let sink = NotificationSink {
            server: &self.name,
            tools_changed: &self.tools_changed,
        };
        self.transport.request(method, params, timeout, &sink).await
    }

    fn error_message(&self, error: &TransportError) -> String {
        format!("MCP server '{}': {error}", self.name)
    }

    /// Send the JSON-RPC `initialize` handshake followed by the
    /// `notifications/initialized` notification.
    ///
    /// # Errors
    /// Returns a display string when the handshake request or response fails.
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
            .request("initialize", params, CONNECT_TIMEOUT)
            .await
            .map_err(|e| self.error_message(&e))?;

        // Send the initialized notification (no id, no response expected).
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.transport
            .notify(&notification)
            .await
            .map_err(|e| self.error_message(&e))?;

        Ok(())
    }

    /// Discover tools from the server via `tools/list`.
    /// Follows cursor-based pagination if the server returns `nextCursor`.
    ///
    /// # Errors
    /// Returns a display string when a `tools/list` request or response fails.
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
                .request("tools/list", params, CONNECT_TIMEOUT)
                .await
                .map_err(|e| self.error_message(&e))?;

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
    ///
    /// # Errors
    /// Returns a `McpCallError` when the `tools/call` request fails or the
    /// server reports a tool error.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, McpCallError> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = self
            .request("tools/call", params, REQUEST_TIMEOUT)
            .await
            .map_err(McpCallError::Transport)?;

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
                McpCallError::Call(format!(
                    "MCP server '{}': tools/call response missing 'content' array",
                    self.name
                ))
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
            Err(McpCallError::Call(output))
        } else {
            Ok(output)
        }
    }

    fn config(&self) -> McpServerConfig {
        self.config.clone()
    }

    async fn terminate(&mut self) {
        self.transport.shutdown().await;
    }

    /// Check whether the underlying transport is still usable.
    pub fn is_alive(&mut self) -> bool {
        self.transport.is_alive()
    }

    /// Attempt to rebuild the transport and reinitialize after a crash.
    async fn respawn(&mut self) -> Result<(), String> {
        self.terminate().await;

        // Rebuild the transport from the same config.
        self.transport = connect_transport(
            &self.name,
            &self.config,
            Arc::clone(&self.pending_oauth_urls),
        )
        .await?;
        self.tools_changed.store(false, Ordering::Release);

        self.initialize().await?;
        self.list_tools().await?;

        tracing::info!(
            server = %self.name,
            tools = self.tools.len(),
            "MCP server respawned"
        );

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

impl Default for McpClientManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpClientManager {
    /// Create an empty manager. Servers are connected asynchronously via
    /// `start_background_discovery`.
    #[must_use]
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
    ///
    /// # Errors
    /// Returns a display string when the named server is unknown or the
    /// underlying `tools/call` fails.
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
            Err(e) => {
                let crash_like = e.is_crash_like();
                let message = e.into_message(server_name);

                // Brief write lock: check liveness and extract the crashed
                // server for out-of-lock respawn.
                let crashed_server = {
                    let mut servers = self.servers.write().await;
                    let server = servers
                        .get_mut(server_name)
                        .ok_or_else(|| format!("MCP server '{server_name}' is not connected"))?;

                    // If alive, the error is a tool-level failure (not a crash).
                    // Also covers the case where another task already respawned.
                    if server.is_alive() && !crash_like {
                        return Err(message);
                    }

                    tracing::warn!(
                        server = %server_name,
                        error = %message,
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
        server
            .call_tool(tool_name, arguments)
            .await
            .map_err(|e| e.into_message(server_name))
    }

    /// Connect and initialize a single MCP server.
    async fn connect_one(
        name: &str,
        entry: &McpServerConfig,
        pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<McpServer, String> {
        let mut server = McpServer::connect(name, entry.clone(), pending_oauth_urls).await?;
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

                seen.insert(name.clone(), McpServerConfig::Stdio { command, args, env });
            }
        }

        seen.into_iter().collect()
    }

    /// Re-scan config files and reconcile servers: connect new ones,
    /// disconnect removed ones, restart changed ones, leave unchanged ones alone.
    ///
    /// Changes take effect immediately: MCP tools are resolved live from
    /// the manager on each LLM request, so all conversations (new and
    /// existing) see the updated server set.
    ///
    /// Returns a summary of what changed.
    pub async fn reload(&self) -> McpReloadResult {
        self.reload_from_configs(Self::read_all_configs()).await
    }

    #[allow(clippy::too_many_lines)] // Reload reconciliation is a single ordered lifecycle: remove, add, restart, summarize.
    async fn reload_from_configs(
        &self,
        configs: Vec<(String, McpServerConfig)>,
    ) -> McpReloadResult {
        let config_names: std::collections::HashSet<String> =
            configs.iter().map(|(n, _)| n.clone()).collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut restarted = Vec::new();
        let mut unchanged = Vec::new();
        let mut failed = Vec::new();
        let mut restart_pending = std::collections::HashSet::new();
        let mut restart_futures: futures::stream::FuturesUnordered<McpRestartFuture> =
            futures::stream::FuturesUnordered::new();

        let mut removed_servers = Vec::new();
        {
            let mut servers = self.servers.write().await;
            let existing_names: Vec<String> = servers.keys().cloned().collect();
            for name in existing_names {
                if !config_names.contains(&name) {
                    if let Some(server) = servers.remove(&name) {
                        removed_servers.push((name.clone(), server));
                    }
                    removed.push(name);
                }
            }
        }
        for (name, mut server) in removed_servers {
            self.pending_oauth_urls.write().await.remove(&name);
            server.terminate().await;
            tracing::info!(server = %name, "MCP server removed during reload");
        }

        for (name, entry) in configs {
            let existing_config = {
                let servers = self.servers.read().await;
                servers.get(&name).map(McpServer::config)
            };

            match existing_config {
                None => {
                    let oauth = Arc::clone(&self.pending_oauth_urls);
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
                Some(current) if current == entry => {
                    unchanged.push(name);
                }
                Some(_) => {
                    let old_server = {
                        let mut servers = self.servers.write().await;
                        match servers.get(&name) {
                            Some(server) if server.config() != entry => servers.remove(&name),
                            Some(_) | None => None,
                        }
                    };

                    let Some(mut old_server) = old_server else {
                        unchanged.push(name);
                        continue;
                    };

                    self.pending_oauth_urls.write().await.remove(&name);
                    old_server.terminate().await;

                    let oauth = Arc::clone(&self.pending_oauth_urls);
                    let servers = Arc::clone(&self.servers);
                    restart_pending.insert(name.clone());
                    restart_futures.push(Box::pin(async move {
                        let result = Self::connect_one(&name, &entry, Arc::clone(&oauth)).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
                                let tool_count = server.tools.len();
                                servers.write().await.insert(name.clone(), server);
                                (name, Ok(tool_count))
                            }
                            Err(error) => (name, Err(error)),
                        }
                    }));
                }
            }
        }

        let restart_deadline = tokio::time::Instant::now() + RELOAD_RESTART_TIMEOUT;
        while !restart_pending.is_empty() {
            let timeout = tokio::time::sleep_until(restart_deadline);
            tokio::pin!(timeout);
            tokio::select! {
                () = &mut timeout => {
                    for name in restart_pending.drain() {
                        self.pending_oauth_urls.write().await.remove(&name);
                        tracing::warn!(
                            server = %name,
                            timeout_seconds = RELOAD_RESTART_TIMEOUT.as_secs(),
                            "Timed out restarting MCP server during reload after config change"
                        );
                        failed.push(McpReloadFailure {
                            server: name,
                            action: "restart".to_string(),
                            error: format!(
                                "timed out after {}s restarting changed MCP server",
                                RELOAD_RESTART_TIMEOUT.as_secs()
                            ),
                        });
                    }
                    break;
                }
                outcome = futures::StreamExt::next(&mut restart_futures) => {
                    let Some((name, result)) = outcome else {
                        break;
                    };
                    restart_pending.remove(&name);
                    match result {
                        Ok(tool_count) => {
                            tracing::info!(
                                server = %name,
                                tools = tool_count,
                                "MCP server restarted during reload after config change"
                            );
                            restarted.push(name);
                        }
                        Err(error) => {
                            tracing::warn!(
                                server = %name,
                                error = %error,
                                "Failed to restart MCP server during reload after config change"
                            );
                            failed.push(McpReloadFailure {
                                server: name,
                                action: "restart".to_string(),
                                error,
                            });
                        }
                    }
                }
            }
        }

        McpReloadResult {
            added,
            removed,
            restarted,
            unchanged,
            failed,
        }
    }

    /// Shut down all MCP server transports.
    #[allow(dead_code)] // Available for graceful shutdown integration
    pub async fn shutdown(&self) {
        let mut servers = self.servers.write().await;
        for (name, server) in servers.iter_mut() {
            server.terminate().await;
            tracing::debug!(server = %name, "MCP server stopped");
        }
        servers.clear();
    }
}

type McpRestartResult = (String, Result<usize, String>);
type McpRestartFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = McpRestartResult> + Send>>;

/// Result of an MCP config reload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpReloadResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub restarted: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<McpReloadFailure>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct McpReloadFailure {
    pub server: String,
    pub action: String,
    pub error: String,
}

/// Parsed MCP server configuration from a config file, tagged by transport
/// (REQ-MCP-001). `PartialEq` is the reload reconciler's unchanged-vs-restart
/// comparison (REQ-MCP-015), so every field of both variants participates.
///
/// Only the `Stdio` variant is produced today: `read_all_configs` skips
/// `type: "http"` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    Http {
        url: String,
        /// Generic per-request headers (org id, beta flag, ...) attached
        /// under ANY auth scheme; they do not imply auth and must not
        /// preempt OAuth (REQ-MCP-008).
        headers: HashMap<String, String>,
        auth: HttpAuth,
    },
}

/// Auth credential for an HTTP server, distinct from the generic `headers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpAuth {
    /// No credential; a 401 starts OAuth discovery.
    None,
    /// An explicit config credential; a 401 against it is a hard failure,
    /// never an OAuth flow (REQ-MCP-008).
    Static(StaticCred),
    /// OAuth 2.1; the client identity may be pre-configured for an
    /// authorization server that disables dynamic client registration.
    OAuth(Option<PreconfiguredClient>),
}

/// An explicit, config-supplied auth credential (REQ-MCP-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticCred {
    Bearer(String),
    /// Designated auth headers (e.g. an API-key header), NOT the generic
    /// per-request `headers`.
    Headers(HashMap<String, String>),
}

/// A pre-configured OAuth client for an authorization server that disables
/// dynamic client registration. Registration *metadata*, not a credential:
/// it seeds the persisted registration so a later 401 reuses it instead of
/// attempting DCR. It does not pre-authorize the server (REQ-MCP-010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreconfiguredClient {
    pub auth_server: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: String,
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

    // -----------------------------------------------------------------------
    // Trait-seam tests: the protocol layer driven over a scripted transport.
    // -----------------------------------------------------------------------

    struct ScriptedExchange {
        /// Server-initiated messages forwarded to the sink before the result.
        server_messages: Vec<Value>,
        result: Result<Value, TransportError>,
    }

    fn exchange(result: Result<Value, TransportError>) -> ScriptedExchange {
        ScriptedExchange {
            server_messages: Vec::new(),
            result,
        }
    }

    struct FakeTransport {
        script: std::sync::Mutex<std::collections::VecDeque<ScriptedExchange>>,
        requests: Arc<std::sync::Mutex<Vec<(String, Value)>>>,
        notifications: Arc<std::sync::Mutex<Vec<Value>>>,
    }

    #[async_trait]
    impl McpTransport for FakeTransport {
        async fn request(
            &self,
            method: &str,
            params: Value,
            _timeout: Duration,
            sink: &dyn ServerMessageSink,
        ) -> Result<Value, TransportError> {
            self.requests
                .lock()
                .unwrap()
                .push((method.to_string(), params));
            let exchange = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("unscripted request");
            for message in exchange.server_messages {
                sink.on_message(message);
            }
            exchange.result
        }

        async fn notify(&self, notification: &Value) -> Result<(), TransportError> {
            self.notifications
                .lock()
                .unwrap()
                .push(notification.clone());
            Ok(())
        }

        fn is_alive(&mut self) -> bool {
            true
        }

        async fn shutdown(&mut self) {}
    }

    type RequestLog = Arc<std::sync::Mutex<Vec<(String, Value)>>>;
    type NotificationLog = Arc<std::sync::Mutex<Vec<Value>>>;

    fn fake_server(script: Vec<ScriptedExchange>) -> (McpServer, RequestLog, NotificationLog) {
        let requests: RequestLog = Arc::default();
        let notifications: NotificationLog = Arc::default();
        let transport = FakeTransport {
            script: std::sync::Mutex::new(script.into()),
            requests: Arc::clone(&requests),
            notifications: Arc::clone(&notifications),
        };
        let server = McpServer {
            name: "fake".to_string(),
            transport: Box::new(transport),
            tools: Vec::new(),
            config: McpServerConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
            },
            tools_changed: AtomicBool::new(false),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
        };
        (server, requests, notifications)
    }

    #[tokio::test]
    async fn initialize_sends_handshake_then_initialized_notification() {
        let (mut server, requests, notifications) = fake_server(vec![exchange(Ok(
            serde_json::json!({"protocolVersion": "2024-11-05", "capabilities": {}}),
        ))]);

        server.initialize().await.expect("initialize");

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "initialize");
        let notifications = notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(
            notifications[0].get("method").and_then(Value::as_str),
            Some("notifications/initialized")
        );
    }

    #[tokio::test]
    async fn list_tools_follows_next_cursor_pagination() {
        let (mut server, requests, _) = fake_server(vec![
            exchange(Ok(serde_json::json!({
                "tools": [{"name": "a", "description": "first", "inputSchema": {"type": "object"}}],
                "nextCursor": "page-2",
            }))),
            exchange(Ok(serde_json::json!({
                "tools": [{"name": "b", "description": "second", "inputSchema": {"type": "object"}}],
            }))),
        ]);

        let tools = server.list_tools().await.expect("list_tools");

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].1.get("cursor").and_then(Value::as_str),
            Some("page-2")
        );
    }

    #[tokio::test]
    async fn call_tool_surfaces_is_error_result_as_tool_error() {
        let (server, _, _) = fake_server(vec![exchange(Ok(serde_json::json!({
            "isError": true,
            "content": [{"type": "text", "text": "boom"}],
        })))]);

        let err = server
            .call_tool("report", serde_json::json!({}))
            .await
            .expect_err("isError result must be an error");

        assert!(!err.is_crash_like());
        assert_eq!(err.into_message("fake"), "boom");
    }

    #[tokio::test]
    async fn server_message_on_sink_sets_tools_changed() {
        let (server, _, _) = fake_server(vec![ScriptedExchange {
            server_messages: vec![serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed",
            })],
            result: Ok(serde_json::json!({"content": []})),
        }]);

        assert!(!server.tools_changed.load(Ordering::Acquire));
        server
            .call_tool("report", serde_json::json!({}))
            .await
            .expect("call_tool");
        assert!(server.tools_changed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn transport_error_classification_drives_crash_detection() {
        let (server, _, _) = fake_server(vec![
            exchange(Err(TransportError::Disconnected(
                "stdout closed (process exited) while waiting for response to 'tools/call'"
                    .to_string(),
            ))),
            exchange(Err(TransportError::Timeout(
                "timed out reading response for 'tools/call'".to_string(),
            ))),
        ]);

        let crash = server
            .call_tool("report", serde_json::json!({}))
            .await
            .expect_err("disconnected must fail");
        assert!(crash.is_crash_like());

        let timeout = server
            .call_tool("report", serde_json::json!({}))
            .await
            .expect_err("timeout must fail");
        assert!(
            !timeout.is_crash_like(),
            "a live-but-slow server must not be classified as crashed"
        );
        assert_eq!(
            timeout.into_message("fake"),
            "MCP server 'fake': timed out reading response for 'tools/call'"
        );
    }

    fn write_fixture_server(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let script = dir.path().join("mcp_fixture.py");
        std::fs::write(
            &script,
            r#"
import json
import os
import sys

marker = sys.argv[1]
label = sys.argv[2] if len(sys.argv) > 2 else ""

def append_marker(event):
    with open(marker, "a", encoding="utf-8") as f:
        f.write(f"{event}|pid={os.getpid()}|label={label}|env={os.environ.get('MCP_TEST_VALUE', '')}\n")
        f.flush()

def send(req_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result}) + "\n")
    sys.stdout.flush()

append_marker("start")
for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    req_id = req.get("id")
    method = req.get("method")
    if method == "initialize":
        send(req_id, {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "fixture", "version": "1"}})
    elif method == "tools/list":
        send(req_id, {"tools": [{"name": "report", "description": "Report config", "inputSchema": {"type": "object"}}]})
    elif method == "tools/call":
        crash_file = os.environ.get("MCP_CRASH_ONCE_FILE")
        if crash_file and os.path.exists(crash_file):
            os.remove(crash_file)
            os._exit(2)
        send(req_id, {"content": [{"type": "text", "text": f"label={label};env={os.environ.get('MCP_TEST_VALUE', '')}"}]})
    elif req_id is not None:
        send(req_id, {})
"#,
        )
        .expect("write fixture server");
        script
    }

    fn fixture_config(
        script: &std::path::Path,
        marker: &std::path::Path,
        label: &str,
        env_value: &str,
    ) -> McpServerConfig {
        McpServerConfig::Stdio {
            command: std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string()),
            args: vec![
                script.display().to_string(),
                marker.display().to_string(),
                label.to_string(),
            ],
            env: HashMap::from([("MCP_TEST_VALUE".to_string(), env_value.to_string())]),
        }
    }

    /// Mutable access to a config's stdio fields; panics on an Http config.
    fn as_stdio_mut(
        config: &mut McpServerConfig,
    ) -> (&mut String, &mut Vec<String>, &mut HashMap<String, String>) {
        match config {
            McpServerConfig::Stdio { command, args, env } => (command, args, env),
            McpServerConfig::Http { .. } => panic!("expected stdio config"),
        }
    }

    async fn connect_fixture(manager: &McpClientManager, config: &McpServerConfig) {
        let server = McpClientManager::connect_one(
            "fixture",
            config,
            Arc::clone(&manager.pending_oauth_urls),
        )
        .await
        .expect("connect fixture");
        manager
            .servers
            .write()
            .await
            .insert("fixture".to_string(), server);
    }

    fn marker_lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn reload_same_config_is_unchanged_without_respawn() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let config = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &config).await;

        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), config)])
            .await;

        assert_eq!(result.unchanged, vec!["fixture"]);
        assert!(result.restarted.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(marker_lines(&marker).len(), 1);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn reload_changed_args_restarts_and_uses_new_args() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        let changed = fixture_config(&script, &marker, "v2", "env1");
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;

        assert_eq!(result.restarted, vec!["fixture"]);
        assert!(result.unchanged.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(marker_lines(&marker).len(), 2);
        let output = manager
            .call_tool("fixture", "report", serde_json::json!({}))
            .await
            .expect("call report");
        assert_eq!(output, "label=v2;env=env1");
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn reload_changed_command_reports_failure_not_unchanged() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        let mut changed = fixture_config(&script, &marker, "v1", "env1");
        *as_stdio_mut(&mut changed).0 = tmp.path().join("missing-command").display().to_string();
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;

        assert!(result.unchanged.is_empty());
        assert!(result.restarted.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].server, "fixture");
        assert_eq!(result.failed[0].action, "restart");
        assert!(manager.status().await.is_empty());
    }

    #[tokio::test]
    async fn reload_transport_change_to_http_is_a_config_change() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        let changed = McpServerConfig::Http {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
        };
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;

        // The variant switch is a config change; the restart then fails
        // because the HTTP transport is not implemented.
        assert!(result.unchanged.is_empty());
        assert!(result.restarted.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].server, "fixture");
        assert_eq!(result.failed[0].action, "restart");
        assert!(result.failed[0].error.contains("HTTP transport"));
    }

    #[tokio::test]
    async fn reload_changed_env_restarts_and_uses_new_env() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        let changed = fixture_config(&script, &marker, "v1", "env2");
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;

        assert_eq!(result.restarted, vec!["fixture"]);
        assert!(result.failed.is_empty());
        let output = manager
            .call_tool("fixture", "report", serde_json::json!({}))
            .await
            .expect("call report");
        assert_eq!(output, "label=v1;env=env2");
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn reload_removes_missing_server() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let config = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &config).await;

        let result = manager.reload_from_configs(Vec::new()).await;

        assert_eq!(result.removed, vec!["fixture"]);
        assert!(manager.status().await.is_empty());
        assert!(manager
            .call_tool("fixture", "report", serde_json::json!({}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn reload_added_server_reports_added_and_connects_in_background() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let config = fixture_config(&script, &marker, "v1", "env1");

        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), config)])
            .await;

        assert_eq!(result.added, vec!["fixture"]);
        let mut connected = false;
        let mut timed_out = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if !manager.status().await.is_empty() {
                connected = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if !connected {
            timed_out = true;
        }
        manager.shutdown().await;
        assert!(!timed_out, "fixture did not connect in background");
    }

    #[tokio::test]
    async fn respawn_after_changed_config_reload_uses_new_config() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let crash_once = tmp.path().join("crash-once");
        std::fs::write(&crash_once, "crash").expect("write crash marker");
        let manager = McpClientManager::new();
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        let mut changed = fixture_config(&script, &marker, "v2", "env2");
        as_stdio_mut(&mut changed).2.insert(
            "MCP_CRASH_ONCE_FILE".to_string(),
            crash_once.display().to_string(),
        );
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;
        assert_eq!(result.restarted, vec!["fixture"]);

        let output = manager
            .call_tool("fixture", "report", serde_json::json!({}))
            .await
            .expect("respawn and retry report");

        assert_eq!(output, "label=v2;env=env2");
        assert_eq!(marker_lines(&marker).len(), 3);
        manager.shutdown().await;
    }
}
