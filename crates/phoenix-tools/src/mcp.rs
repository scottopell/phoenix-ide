//! MCP (Model Context Protocol) client
//!
//! The JSON-RPC 2.0 protocol layer (`initialize`, paginated `tools/list`,
//! `tools/call`, notification handling) lives on `McpServer` and is
//! transport-agnostic; how a request's bytes leave and a response's bytes
//! arrive is behind the `McpTransport` trait. `StdioTransport` reaches a
//! server spawned as a child subprocess; `HttpTransport` reaches a remote
//! server over the Streamable HTTP transport. Discovered tools are exposed
//! as regular Phoenix tools through the Tool trait. Spec: `specs/mcp/`.

pub mod http;
pub mod stdio;

pub use http::HttpTransport;
pub use stdio::StdioTransport;

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Timeout for a single JSON-RPC request-response round trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Longer timeout for initialize + tools/list during server connection.
/// Five minutes gives OAuth flows (mcp-remote prompts, browser redirect) time to complete.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Upper bound for an HTTP reload request applying changed existing configs.
const RELOAD_RESTART_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for a fire-and-forget JSON-RPC notification; notifications never
/// legitimately take as long as a tool call.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// The protocol revision advertised in the `initialize` request. A
    /// transport property: stdio predates the Streamable HTTP transport,
    /// while HTTP servers negotiate from the revision that introduced it.
    fn requested_protocol_version(&self) -> &'static str;

    /// Whether the underlying connection is still usable
    /// (stdio: the child process is running).
    fn is_alive(&mut self) -> bool;

    /// Tear down the transport (stdio: kill the child process).
    async fn shutdown(&mut self);
}

/// Build a transport for `config`: stdio spawns the child process, HTTP
/// builds the client (the connection itself is exercised by `initialize`).
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
        McpServerConfig::Http { url, headers, auth } => {
            Ok(Box::new(HttpTransport::connect(name, url, headers, auth)?))
        }
    }
}

/// Insert a server into the map, terminating any instance it displaces so an
/// evicted connection is shut down (ending its HTTP session with the DELETE)
/// rather than silently dropped. Displacement is rare -- it takes an insert
/// racing a hold/reload window -- but a leaked remote session is invisible,
/// so every insert routes through here.
async fn insert_server(
    servers: &RwLock<HashMap<String, McpServer>>,
    name: &str,
    server: McpServer,
) {
    let displaced = servers.write().await.insert(name.to_string(), server);
    if let Some(mut displaced) = displaced {
        tracing::warn!(
            server = %name,
            "Insert displaced an existing MCP server instance; terminating it"
        );
        displaced.terminate().await;
    }
}

/// Publish a freshly connected server -- but only if `ticket` is still the
/// current connect attempt for `name`. A connect that outlived its reload
/// (e.g. abandoned at the reload deadline) must not resurrect a server a
/// newer reload removed, or displace its replacement with stale config; a
/// superseded server is terminated instead (ending any session it created).
/// The check and the insert share the `servers` write lock so a concurrent
/// reload's ticket revocation cannot interleave between them. Returns
/// whether the server was published.
async fn publish_if_current(
    servers: &RwLock<HashMap<String, McpServer>>,
    tickets: &std::sync::Mutex<HashMap<String, u64>>,
    name: &str,
    ticket: u64,
    server: McpServer,
) -> bool {
    let (published, mut leftover) = {
        let mut servers = servers.write().await;
        if tickets.lock().unwrap().get(name).copied() == Some(ticket) {
            (true, servers.insert(name.to_string(), server))
        } else {
            (false, Some(server))
        }
    };
    if let Some(leftover) = leftover.as_mut() {
        if published {
            tracing::warn!(
                server = %name,
                "Publish displaced an existing MCP server instance; terminating it"
            );
        } else {
            tracing::warn!(
                server = %name,
                "Discarding a late MCP connect superseded by a newer reload"
            );
        }
        leftover.terminate().await;
    }
    published
}

/// Extract a string-to-string map from an optional JSON object, dropping
/// non-string values.
fn string_map(value: Option<&Value>) -> HashMap<String, String> {
    value
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|val| (k.clone(), val.to_string())))
                .collect()
        })
        .unwrap_or_default()
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
// McpServer
// ---------------------------------------------------------------------------

/// Failure from one MCP request round trip (`tools/call`, `tools/list`),
/// keeping the transport classification intact so recovery paths dispatch on
/// the variant rather than string-matching the message.
#[derive(Debug)]
pub enum McpRequestError {
    /// Classified by the transport. The detail is unprefixed; format with the
    /// server name via `into_message`.
    Transport(TransportError),
    /// Tool-level failure (`isError` result) or malformed response; the
    /// string is the complete display message.
    Other(String),
}

impl McpRequestError {
    fn into_message(self, server_name: &str) -> String {
        match self {
            Self::Transport(e) => format!("MCP server '{server_name}': {e}"),
            Self::Other(message) => message,
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
    /// Identifies the current transport instance; reassigned whenever the
    /// transport is (re)built. A failure observed against one generation
    /// must not tear down a later one (a stale error racing a completed
    /// recovery).
    generation: u64,
    /// Set when the server sends `notifications/tools/list_changed`.
    /// Cleared after the next `list_tools()` refresh.
    tools_changed: AtomicBool,
    /// Shared map of server name → OAuth URL; written by the stdio stderr
    /// drain, read by `McpClientManager::status()`. Retained so a respawned
    /// transport keeps feeding the same map.
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
}

/// Monotonic source for `McpServer::generation`.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
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
            generation: next_generation(),
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
            "protocolVersion": self.transport.requested_protocol_version(),
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
    /// Returns a `McpRequestError` when a `tools/list` request or response
    /// fails, so callers can dispatch recovery on the transport
    /// classification (the lazy `list_changed` refresh re-establishes an
    /// expired HTTP session, REQ-MCP-005).
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, McpRequestError> {
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
                .map_err(McpRequestError::Transport)?;

            let tools_arr = resp
                .get("tools")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    McpRequestError::Other(format!(
                        "MCP server '{}': tools/list response missing 'tools' array",
                        self.name
                    ))
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
    /// Returns a `McpRequestError` when the `tools/call` request fails or the
    /// server reports a tool error.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, McpRequestError> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = self
            .request("tools/call", params, REQUEST_TIMEOUT)
            .await
            .map_err(McpRequestError::Transport)?;

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
                McpRequestError::Other(format!(
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
            Err(McpRequestError::Other(output))
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

    /// The recovery verb for this server's transport: stdio respawns a
    /// process, HTTP reconnects a client.
    fn recovery_action(&self) -> &'static str {
        match &self.config {
            McpServerConfig::Stdio { .. } => "respawn",
            McpServerConfig::Http { .. } => "reconnect",
        }
    }

    /// Whether `error` warrants tearing down and re-establishing the
    /// transport before one retry. Stdio recovers only from a crash-like
    /// `Disconnected` -- a live-but-slow server is not respawned
    /// (REQ-MCP-003). HTTP additionally recovers from a timeout (REQ-MCP-007)
    /// and an expired session, which re-initializes (REQ-MCP-005).
    fn should_reestablish(&self, error: &McpRequestError) -> bool {
        let McpRequestError::Transport(transport_error) = error else {
            return false;
        };
        match &self.config {
            McpServerConfig::Stdio { .. } => {
                matches!(transport_error, TransportError::Disconnected(_))
            }
            McpServerConfig::Http { .. } => matches!(
                transport_error,
                TransportError::Disconnected(_)
                    | TransportError::Timeout(_)
                    | TransportError::SessionExpired
            ),
        }
    }

    /// Run the post-connect handshake: `initialize` then the first
    /// `tools/list`.
    ///
    /// On failure the transport is shut down before returning: `initialize`
    /// may already have created a server-side HTTP session, and dropping the
    /// transport without the session DELETE would leak it until expiry
    /// (REQ-MCP-005).
    async fn handshake(&mut self) -> Result<(), String> {
        let result = match self.initialize().await {
            Ok(()) => {
                let name = self.name.clone();
                self.list_tools()
                    .await
                    .map(|_| ())
                    .map_err(|e| e.into_message(&name))
            }
            Err(e) => Err(e),
        };
        if result.is_err() {
            self.terminate().await;
        }
        result
    }

    /// Rebuild the transport from the retained config and re-run the
    /// handshake (stdio: respawn the process; HTTP: fresh client + session).
    async fn reestablish(&mut self) -> Result<(), String> {
        self.terminate().await;

        self.transport = connect_transport(
            &self.name,
            &self.config,
            Arc::clone(&self.pending_oauth_urls),
        )
        .await?;
        self.generation = next_generation();
        self.tools_changed.store(false, Ordering::Release);

        self.handshake().await?;

        tracing::info!(
            server = %self.name,
            tools = self.tools.len(),
            action = self.recovery_action(),
            "MCP server connection re-established"
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// McpClientManager
// ---------------------------------------------------------------------------

/// Owns all MCP server connections.
///
/// Lock ordering: always acquire `servers` before `disabled_servers`,
/// `recovering`, or `connect_tickets`. The tokio `RwLock`s must not be held
/// across heavy `.await` points (respawn, connect, etc.) -- extract data,
/// drop the lock, then do async I/O. The sync mutexes are held only for map
/// access.
pub struct McpClientManager {
    servers: Arc<RwLock<HashMap<String, McpServer>>>,
    /// Server names whose tools should be excluded from conversations.
    /// The servers remain connected for instant re-enable.
    disabled_servers: RwLock<std::collections::HashSet<String>>,
    /// Servers temporarily held out of `servers` (mid-recovery after a
    /// transport failure, or mid tool-list refresh): the holder parks a
    /// watch sender here via `ServerClaim`; calls that find the server
    /// absent subscribe and wait for the sender to drop (work finished)
    /// instead of failing with "not connected".
    recovering: std::sync::Mutex<HashMap<String, tokio::sync::watch::Sender<()>>>,
    /// The current connect attempt per server name. Every spawned connect
    /// (discovery, reload-added, reload-restart) records a ticket here and
    /// publishes its result only while that ticket is still current
    /// (`publish_if_current`), so an attempt outlived by a newer reload
    /// cannot resurrect a removed server or displace its replacement with
    /// stale config.
    connect_tickets: Arc<std::sync::Mutex<HashMap<String, u64>>>,
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
            recovering: std::sync::Mutex::new(HashMap::new()),
            connect_tickets: Arc::new(std::sync::Mutex::new(HashMap::new())),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new connect attempt for `server_name`, superseding any
    /// earlier attempt still in flight.
    fn issue_connect_ticket(&self, server_name: &str) -> u64 {
        let ticket = next_generation();
        self.connect_tickets
            .lock()
            .unwrap()
            .insert(server_name.to_string(), ticket);
        ticket
    }

    /// The recovery-claim map. Held only for map access, never across an
    /// await, so poisoning would require a panic that is already fatal.
    fn recovering_map(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, tokio::sync::watch::Sender<()>>> {
        self.recovering.lock().unwrap()
    }

    /// Park a claim for `server_name`. Must be called while holding the
    /// `servers` write lock that removes the entry, so a concurrent caller
    /// can never observe the server absent without also seeing the claim.
    fn claim_server(&self, server_name: &str) -> ServerClaim<'_> {
        let (sender, _) = tokio::sync::watch::channel(());
        self.recovering_map()
            .insert(server_name.to_string(), sender);
        ServerClaim {
            manager: self,
            name: server_name.to_string(),
        }
    }

    /// If a claim is parked for `server_name`, wait for it to be released.
    /// The claim is a drop guard released on every holder exit path and
    /// every stage of re-establish is itself deadline-bounded, so this wait
    /// cannot be stranded; a follow-up map lookup observes the outcome.
    async fn await_claim_release(&self, server_name: &str) {
        let receiver = self
            .recovering_map()
            .get(server_name)
            .map(tokio::sync::watch::Sender::subscribe);
        if let Some(mut receiver) = receiver {
            // changed() resolves (with Err) when the sender drops.
            let _ = receiver.changed().await;
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
                    let ticket = manager.issue_connect_ticket(&name);
                    tokio::spawn(async move {
                        let result = Self::connect_one(&name, &entry, Arc::clone(&oauth)).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
                                let tool_count = server.tools.len();
                                if !publish_if_current(
                                    &mgr.servers,
                                    &mgr.connect_tickets,
                                    &name,
                                    ticket,
                                    server,
                                )
                                .await
                                {
                                    return None;
                                }
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
            let extracted = {
                let mut servers = self.servers.write().await;
                match servers.get_mut(&name) {
                    Some(s) if s.tools_changed.swap(false, Ordering::AcqRel) => {
                        // Claim the hold under the same lock that removes the
                        // entry, so a tool call landing mid-refresh (or
                        // mid-refresh-recovery) waits for the outcome instead
                        // of failing with "not connected".
                        let claim = self.claim_server(&name);
                        servers.remove(&name).map(|server| (server, claim))
                    }
                    _ => None,
                }
            };
            // Lock dropped -- list_tools() runs with no lock held.
            if let Some((mut server, claim)) = extracted {
                let keep = match server.list_tools().await {
                    Ok(tools) => {
                        tracing::info!(
                            server = %name,
                            tools = tools.len(),
                            "Refreshed tool list after list_changed notification"
                        );
                        true
                    }
                    // A recoverable transport failure (e.g. the HTTP session
                    // expired mid-refresh) re-establishes the connection,
                    // which re-runs the handshake and tools/list, instead of
                    // leaving the server with a stale tool list (REQ-MCP-005,
                    // REQ-MCP-007).
                    Err(e) if server.should_reestablish(&e) => {
                        tracing::warn!(
                            server = %name,
                            error = %e.into_message(&name),
                            "Tool refresh hit a transport failure, re-establishing connection"
                        );
                        match server.reestablish().await {
                            Ok(()) => true,
                            Err(reestablish_err) => {
                                tracing::warn!(
                                    server = %name,
                                    error = %reestablish_err,
                                    "Re-establish failed after refresh failure, dropping server"
                                );
                                false
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %name,
                            error = %e.into_message(&name),
                            "Failed to refresh tools after list_changed"
                        );
                        true
                    }
                };
                // Reinsert under brief write lock -- unless re-establish
                // failed, in which case the transport is torn down and stale
                // definitions must not be advertised from it; a reload
                // reconnects it (REQ-MCP-015). The claim is released after,
                // so waiters observe the outcome.
                if keep {
                    insert_server(&self.servers, &name, server).await;
                }
                drop(claim);
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

    /// One `tools/call` attempt via the read-lock path. A call arriving while
    /// the server is held out of the map for recovery joins the parked claim
    /// instead of failing with "not connected"; with no claim parked, the
    /// re-lookup settles whether the server is genuinely gone or a recovery
    /// just finished.
    async fn attempt_call(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<CallAttempt, String> {
        let servers = self.servers.read().await;
        if let Some(server) = servers.get(server_name) {
            return Ok(CallAttempt::run(server, tool_name, arguments).await);
        }
        drop(servers);
        self.await_claim_release(server_name).await;
        let servers = self.servers.read().await;
        let server = servers
            .get(server_name)
            .ok_or_else(|| format!("MCP server '{server_name}' is not connected"))?;
        Ok(CallAttempt::run(server, tool_name, arguments).await)
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

        let attempt = self
            .attempt_call(server_name, tool_name, &arguments)
            .await?;

        match attempt.result {
            Ok(result) => return Ok(result),
            Err(e) => {
                // One concurrent failing call leads the recovery; the others
                // follow by waiting for it to finish, then retrying.
                enum Recovery<'a> {
                    Lead {
                        server: Box<McpServer>,
                        action: &'static str,
                        claim: ServerClaim<'a>,
                    },
                    Follow(tokio::sync::watch::Receiver<()>),
                    /// The failing transport was already replaced; go
                    /// straight to the retry.
                    Retry,
                }

                // Brief write lock: classify the failure and either claim the
                // recovery (extracting the server for out-of-lock work) or
                // join one already in flight.
                let recovery = {
                    let mut servers = self.servers.write().await;
                    if let Some(mut server) = servers.remove(server_name) {
                        // The failing instance is still the one in the map:
                        // its own judgement applies. A healthy transport with
                        // a non-recoverable failure is a tool-level error --
                        // surface it; retrying could re-execute a
                        // side-effecting call.
                        if server.generation == attempt.generation
                            && server.is_alive()
                            && !attempt.recoverable
                        {
                            servers.insert(server_name.to_string(), server);
                            return Err(e.into_message(server_name));
                        }

                        // A failure from a transport that has already been
                        // replaced (another task finished a recovery, or
                        // reload swapped the server) is stale: the fresh
                        // instance's policy must not re-judge it. Surface it
                        // if the serving instance deemed it non-recoverable;
                        // otherwise retry on the fresh instance instead of
                        // tearing it down.
                        if server.generation == attempt.generation {
                            let action = server.recovery_action();
                            tracing::warn!(
                                server = %server_name,
                                error = %e.into_message(server_name),
                                action = action,
                                "MCP server connection lost, removing to re-establish"
                            );

                            // Claim the recovery while still holding the
                            // servers lock, so a concurrent failing call
                            // cannot observe the server absent without also
                            // seeing the claim.
                            Recovery::Lead {
                                server: Box::new(server),
                                action,
                                claim: self.claim_server(server_name),
                            }
                        } else {
                            servers.insert(server_name.to_string(), server);
                            if !attempt.recoverable {
                                return Err(e.into_message(server_name));
                            }
                            Recovery::Retry
                        }
                    } else {
                        // Absent with a claim parked: a concurrent call is
                        // already re-establishing this server -- wait for it
                        // rather than failing with "not connected".
                        let receiver = self
                            .recovering_map()
                            .get(server_name)
                            .map(tokio::sync::watch::Sender::subscribe);
                        let Some(receiver) = receiver else {
                            return Err(format!("MCP server '{server_name}' is not connected"));
                        };
                        Recovery::Follow(receiver)
                    }
                };
                // Write lock is dropped here.

                match recovery {
                    // Re-establish outside the lock so other servers aren't
                    // blocked. The claim guard is dropped (waking followers)
                    // only after the server is back in the map -- and on the
                    // error path, only after the drop decision is final.
                    Recovery::Lead {
                        mut server,
                        action,
                        claim,
                    } => {
                        let result = server.reestablish().await;
                        if result.is_ok() {
                            insert_server(&self.servers, server_name, *server).await;
                        }
                        drop(claim);
                        if let Err(reestablish_err) = result {
                            return Err(format!(
                                "MCP server '{server_name}' connection lost and {action} failed: {reestablish_err}"
                            ));
                        }
                    }
                    // Wait for the leader's claim guard to drop. Unbounded by
                    // design: the guard releases on every leader exit path
                    // (including unwind), and each re-establish stage is
                    // itself deadline-bounded, so a slow-but-successful
                    // recovery is never misreported as a failure here.
                    Recovery::Follow(mut receiver) => {
                        let _ = receiver.changed().await;
                    }
                    Recovery::Retry => {}
                }
            }
        }

        // Retry once via the normal read-lock path after recovery. The server
        // can be absent here when a followed recovery failed and dropped it.
        let servers = self.servers.read().await;
        let server = servers.get(server_name).ok_or_else(|| {
            format!("MCP server '{server_name}' is not connected after a failed recovery")
        })?;
        server
            .call_tool(tool_name, arguments)
            .await
            .map_err(|e| e.into_message(server_name))
    }

    /// Connect and initialize a single MCP server. A failed handshake shuts
    /// the transport down (ending any session `initialize` created) before
    /// the error is returned.
    async fn connect_one(
        name: &str,
        entry: &McpServerConfig,
        pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<McpServer, String> {
        let mut server = McpServer::connect(name, entry.clone(), pending_oauth_urls).await?;
        server.handshake().await?;
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

                if let Some(config) = Self::classify_config_entry(name, cfg) {
                    tracing::debug!(
                        server = %name,
                        path = %path.display(),
                        "Found MCP server config"
                    );
                    seen.insert(name.clone(), config);
                } else {
                    tracing::debug!(
                        server = %name,
                        path = %path.display(),
                        "Skipping unusable MCP server config"
                    );
                }
            }
        }

        seen.into_iter().collect()
    }

    /// Classify one `mcpServers` entry into a transport-tagged config
    /// (REQ-MCP-001): `"type": "http"` + `url` selects HTTP, a `command`
    /// field selects stdio. Returns `None` (the entry is skipped) when
    /// neither is usable, with the reason at `debug` level.
    fn classify_config_entry(name: &str, cfg: &Value) -> Option<McpServerConfig> {
        if cfg.get("type").and_then(|v| v.as_str()) == Some("http") {
            let Some(url) = cfg.get("url").and_then(|v| v.as_str()) else {
                tracing::debug!(server = %name, "HTTP MCP server without 'url' field");
                return None;
            };
            let auth = match cfg.get("auth") {
                None => HttpAuth::None,
                Some(auth) => Self::classify_http_auth(name, auth)?,
            };
            return Some(McpServerConfig::Http {
                url: url.to_string(),
                headers: string_map(cfg.get("headers")),
                auth,
            });
        }

        // Must have a command field (stdio transport).
        let Some(command) = cfg.get("command").and_then(|v| v.as_str()) else {
            tracing::debug!(server = %name, "MCP server without 'command' field");
            return None;
        };

        let args: Vec<String> = cfg
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Some(McpServerConfig::Stdio {
            command: command.to_string(),
            args,
            env: string_map(cfg.get("env")),
        })
    }

    /// Classify an HTTP entry's `auth` object: `{"bearer": "<token>"}` or
    /// `{"headers": {...}}` is an explicit static credential (REQ-MCP-008).
    /// An unrecognized or malformed shape skips the server -- silently
    /// downgrading an intended credential to no-auth (or dropping part of
    /// it) would change which authorization path a 401 takes.
    fn classify_http_auth(name: &str, auth: &Value) -> Option<HttpAuth> {
        if let Some(token) = auth.get("bearer").and_then(|v| v.as_str()) {
            return Some(HttpAuth::Static(StaticCred::Bearer(token.to_string())));
        }
        if let Some(headers_value) = auth.get("headers") {
            let Some(object) = headers_value.as_object() else {
                tracing::debug!(server = %name, "'auth.headers' is not an object");
                return None;
            };
            let mut headers = HashMap::new();
            for (key, value) in object {
                let Some(value) = value.as_str() else {
                    tracing::debug!(
                        server = %name,
                        header = %key,
                        "'auth.headers' value is not a string"
                    );
                    return None;
                };
                headers.insert(key.clone(), value.to_string());
            }
            if headers.is_empty() {
                tracing::debug!(server = %name, "'auth.headers' is empty");
                return None;
            }
            return Some(HttpAuth::Static(StaticCred::Headers(headers)));
        }
        tracing::debug!(server = %name, "HTTP MCP server with unrecognized 'auth' shape");
        None
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

    /// Look up `name`'s running config, settling any in-flight hold first: a
    /// server held out of the map for a refresh or recovery must not be
    /// misread as absent (which reload would treat as newly added, starting
    /// a duplicate connection racing the holder's reinsert). The loop
    /// re-checks after each release because a new claim can arm at any
    /// moment; with no claim parked, absence is settled.
    async fn settled_config(&self, name: &str) -> Option<McpServerConfig> {
        loop {
            {
                let servers = self.servers.read().await;
                if let Some(server) = servers.get(name) {
                    return Some(server.config());
                }
            }
            let receiver = self
                .recovering_map()
                .get(name)
                .map(tokio::sync::watch::Sender::subscribe);
            match receiver {
                Some(mut receiver) => {
                    let _ = receiver.changed().await;
                }
                None => return None,
            }
        }
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

        // Removal sweep. A held server (claim parked, entry absent) whose
        // name left the config must also be removed -- the holder would
        // otherwise reinsert it later as a zombie -- so claimed names are
        // settled and swept alongside the map keys.
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
        let claimed: Vec<String> = self.recovering_map().keys().cloned().collect();
        for name in claimed {
            if !config_names.contains(&name) && !removed.contains(&name) {
                self.await_claim_release(&name).await;
                if let Some(server) = self.servers.write().await.remove(&name) {
                    removed_servers.push((name.clone(), server));
                    removed.push(name);
                }
            }
        }
        for (name, mut server) in removed_servers {
            self.pending_oauth_urls.write().await.remove(&name);
            server.terminate().await;
            tracing::info!(server = %name, "MCP server removed during reload");
        }
        // Revoke connect tickets for names no longer configured, superseding
        // any still-in-flight connect attempt (e.g. one abandoned at a
        // previous reload's deadline) so its late publish is discarded
        // instead of resurrecting a removed server.
        self.connect_tickets
            .lock()
            .unwrap()
            .retain(|name, _| config_names.contains(name));

        for (name, entry) in configs {
            let existing_config = self.settled_config(&name).await;

            match existing_config {
                None => {
                    let oauth = Arc::clone(&self.pending_oauth_urls);
                    oauth.write().await.remove(&name);
                    added.push(name.clone());

                    let servers = Arc::clone(&self.servers);
                    let tickets = Arc::clone(&self.connect_tickets);
                    let ticket = self.issue_connect_ticket(&name);
                    tokio::spawn(async move {
                        let result = Self::connect_one(&name, &entry, Arc::clone(&oauth)).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
                                let tool_count = server.tools.len();
                                if publish_if_current(&servers, &tickets, &name, ticket, server)
                                    .await
                                {
                                    tracing::info!(
                                        server = %name,
                                        tools = tool_count,
                                        "MCP server connected during reload"
                                    );
                                }
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
                    let tickets = Arc::clone(&self.connect_tickets);
                    let ticket = self.issue_connect_ticket(&name);
                    restart_pending.insert(name.clone());
                    // The connect runs as a detached task: when the reload
                    // deadline drops the awaiting future below, the task is
                    // abandoned, not cancelled, so a partially established
                    // connection still finishes -- publishing (late, ticket
                    // permitting) on success, or terminating the transport on
                    // a handshake failure so a created HTTP session is
                    // DELETEd rather than leaked by a cancelled future.
                    let task_name = name.clone();
                    let task = tokio::spawn(async move {
                        let result = Self::connect_one(&name, &entry, Arc::clone(&oauth)).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
                                let tool_count = server.tools.len();
                                publish_if_current(&servers, &tickets, &name, ticket, server).await;
                                (name, Ok(tool_count))
                            }
                            Err(error) => (name, Err(error)),
                        }
                    });
                    restart_futures.push(Box::pin(async move {
                        task.await.unwrap_or_else(|join_error| {
                            (task_name, Err(format!("restart task failed: {join_error}")))
                        })
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
                            "Timed out restarting MCP server during reload after config change; the connect continues in the background"
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

/// An exclusive hold on a server temporarily out of the `servers` map
/// (re-establishing after a transport failure, or refreshing its tool list).
/// Dropping it releases the claim and wakes waiters on every exit path --
/// success, error, panic unwind, or a dropped future -- so a dead holder can
/// never strand the callers waiting on it.
struct ServerClaim<'a> {
    manager: &'a McpClientManager,
    name: String,
}

impl Drop for ServerClaim<'_> {
    fn drop(&mut self) {
        self.manager.recovering_map().remove(&self.name);
    }
}

/// Outcome of one `tools/call` attempt: the result, the generation of the
/// instance that served it, and that instance's own judgement of whether a
/// failure is a recoverable transport error. Recoverability is decided by
/// the serving instance's policy at attempt time -- not by whatever later
/// occupies the map slot, whose transport (and policy) may differ.
struct CallAttempt {
    result: Result<String, McpRequestError>,
    generation: u64,
    recoverable: bool,
}

impl CallAttempt {
    async fn run(server: &McpServer, tool_name: &str, arguments: &Value) -> Self {
        let result = server.call_tool(tool_name, arguments.clone()).await;
        let recoverable = result
            .as_ref()
            .err()
            .is_some_and(|e| server.should_reestablish(e));
        Self {
            result,
            generation: server.generation,
            recoverable,
        }
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
    fn test_read_all_configs_does_not_panic() {
        // Verify that read_all_configs works with no config files present
        // (it should return empty, not error).
        let configs = McpClientManager::read_all_configs();
        // We can't assert anything about count since the dev machine may have configs,
        // but the call should not panic.
        let _ = configs;
    }

    #[test]
    fn classify_entry_selects_stdio_for_command() {
        let cfg = serde_json::json!({
            "command": "uvx",
            "args": ["server"],
            "env": {"KEY": "v"},
        });
        let config = McpClientManager::classify_config_entry("s", &cfg).expect("stdio config");
        assert_eq!(
            config,
            McpServerConfig::Stdio {
                command: "uvx".to_string(),
                args: vec!["server".to_string()],
                env: HashMap::from([("KEY".to_string(), "v".to_string())]),
            }
        );
    }

    #[test]
    fn classify_entry_selects_http_with_headers_and_no_auth() {
        let cfg = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": {"X-Org": "acme"},
        });
        let config = McpClientManager::classify_config_entry("s", &cfg).expect("http config");
        assert_eq!(
            config,
            McpServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::from([("X-Org".to_string(), "acme".to_string())]),
                auth: HttpAuth::None,
            }
        );
    }

    #[test]
    fn classify_entry_parses_static_credentials() {
        let bearer = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "auth": {"bearer": "tok"},
        });
        assert_eq!(
            McpClientManager::classify_config_entry("s", &bearer),
            Some(McpServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::Static(StaticCred::Bearer("tok".to_string())),
            })
        );

        let auth_headers = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "auth": {"headers": {"X-Api-Key": "k"}},
        });
        assert_eq!(
            McpClientManager::classify_config_entry("s", &auth_headers),
            Some(McpServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::Static(StaticCred::Headers(HashMap::from([(
                    "X-Api-Key".to_string(),
                    "k".to_string()
                )]))),
            })
        );
    }

    #[test]
    fn classify_entry_skips_unusable_entries() {
        // No command and not HTTP.
        let neither = serde_json::json!({"args": ["x"]});
        assert_eq!(McpClientManager::classify_config_entry("s", &neither), None);

        // HTTP without a url.
        let no_url = serde_json::json!({"type": "http"});
        assert_eq!(McpClientManager::classify_config_entry("s", &no_url), None);

        // Unrecognized auth shape must not silently downgrade to no-auth.
        let bad_auth = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "auth": {"oauth2": true},
        });
        assert_eq!(
            McpClientManager::classify_config_entry("s", &bad_auth),
            None
        );

        // Malformed auth.headers must not become a partial/empty credential.
        for headers in [
            serde_json::json!("not-an-object"),
            serde_json::json!({"X-Api-Key": 42}),
            serde_json::json!({}),
        ] {
            let malformed = serde_json::json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "auth": {"headers": headers},
            });
            assert_eq!(
                McpClientManager::classify_config_entry("s", &malformed),
                None,
                "auth.headers {malformed} must skip the server"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Trait-seam tests: the protocol layer driven over a scripted transport.
    // -----------------------------------------------------------------------

    struct ScriptedExchange {
        /// Server-initiated messages forwarded to the sink before the result.
        server_messages: Vec<Value>,
        result: Result<Value, TransportError>,
        /// Sleep before responding -- lets a test order concurrent exchanges.
        delay: Duration,
    }

    fn exchange(result: Result<Value, TransportError>) -> ScriptedExchange {
        ScriptedExchange {
            server_messages: Vec::new(),
            result,
            delay: Duration::ZERO,
        }
    }

    fn delayed_exchange(result: Result<Value, TransportError>, delay_ms: u64) -> ScriptedExchange {
        ScriptedExchange {
            server_messages: Vec::new(),
            result,
            delay: Duration::from_millis(delay_ms),
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
            if exchange.delay > Duration::ZERO {
                tokio::time::sleep(exchange.delay).await;
            }
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

        fn requested_protocol_version(&self) -> &'static str {
            "2024-11-05"
        }

        fn is_alive(&mut self) -> bool {
            true
        }

        async fn shutdown(&mut self) {}
    }

    type RequestLog = Arc<std::sync::Mutex<Vec<(String, Value)>>>;
    type NotificationLog = Arc<std::sync::Mutex<Vec<Value>>>;

    fn fake_server_with_config(
        script: Vec<ScriptedExchange>,
        config: McpServerConfig,
    ) -> (McpServer, RequestLog, NotificationLog) {
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
            config,
            generation: next_generation(),
            tools_changed: AtomicBool::new(false),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
        };
        (server, requests, notifications)
    }

    fn fake_server(script: Vec<ScriptedExchange>) -> (McpServer, RequestLog, NotificationLog) {
        fake_server_with_config(
            script,
            McpServerConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
            },
        )
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
        assert_eq!(
            requests[0].1.get("protocolVersion").and_then(Value::as_str),
            Some("2024-11-05"),
            "stdio advertises the pre-Streamable-HTTP revision"
        );
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

        assert!(!server.should_reestablish(&err));
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
            delay: Duration::ZERO,
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
        assert!(server.should_reestablish(&crash));

        let timeout = server
            .call_tool("report", serde_json::json!({}))
            .await
            .expect_err("timeout must fail");
        assert!(
            !server.should_reestablish(&timeout),
            "a live-but-slow server must not be classified as crashed"
        );
        assert_eq!(
            timeout.into_message("fake"),
            "MCP server 'fake': timed out reading response for 'tools/call'"
        );
    }

    #[tokio::test]
    async fn http_recovery_policy_covers_timeout_and_session_expiry() {
        let http_config = McpServerConfig::Http {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
        };
        let (server, _, _) = fake_server_with_config(Vec::new(), http_config);

        for recoverable in [
            TransportError::Disconnected("reset".to_string()),
            TransportError::Timeout("timed out".to_string()),
            TransportError::SessionExpired,
        ] {
            assert!(
                server.should_reestablish(&McpRequestError::Transport(recoverable.clone())),
                "{recoverable:?} must reconnect an HTTP server"
            );
        }
        for surfaced in [
            TransportError::Unauthorized {
                www_authenticate: None,
            },
            TransportError::Rpc {
                code: -1,
                message: "x".to_string(),
            },
        ] {
            assert!(
                !server.should_reestablish(&McpRequestError::Transport(surfaced.clone())),
                "{surfaced:?} must be surfaced, not retried"
            );
        }
    }

    fn stdio_test_config(command: &str) -> McpServerConfig {
        McpServerConfig::Stdio {
            command: command.to_string(),
            args: Vec::new(),
            env: HashMap::new(),
        }
    }

    /// A failing call whose server got replaced mid-flight (here: an HTTP
    /// instance swapped for a stdio one, as a reload would) must be judged by
    /// the policy of the instance that served it -- `SessionExpired` is
    /// recoverable for the serving HTTP instance, so the call retries on the
    /// replacement instead of surfacing the stale error or re-reconnecting.
    #[tokio::test]
    async fn stale_recoverable_error_retries_on_the_replacement_server() {
        let manager = Arc::new(McpClientManager::new());
        let (serving, _, _) = fake_server_with_config(
            vec![delayed_exchange(Err(TransportError::SessionExpired), 200)],
            McpServerConfig::Http {
                url: "http://127.0.0.1:1/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::None,
            },
        );
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), serving);

        let (replacement, replacement_requests, _) = fake_server_with_config(
            vec![exchange(Ok(serde_json::json!({
                "content": [{"type": "text", "text": "fresh"}]
            })))],
            stdio_test_config("replacement"),
        );
        let swapper = Arc::clone(&manager);
        let swap = tokio::spawn(async move {
            // Queued behind the in-flight call's read lock; lands as soon as
            // the failing call releases it, before the recovery write lock.
            tokio::time::sleep(Duration::from_millis(100)).await;
            swapper
                .servers
                .write()
                .await
                .insert("fake".to_string(), replacement);
        });

        let result = manager
            .call_tool("fake", "report", serde_json::json!({}))
            .await;
        swap.await.expect("swap task");

        assert_eq!(
            result.expect("stale recoverable error must retry on the replacement"),
            "fresh"
        );
        assert_eq!(replacement_requests.lock().unwrap().len(), 1);
    }

    /// The inverse: the serving instance (stdio) deems its timeout
    /// non-recoverable, so even though the map now holds a replacement, the
    /// stale failure surfaces and the replacement is never re-invoked --
    /// retrying could re-execute a side-effecting call.
    #[tokio::test]
    async fn stale_nonrecoverable_error_surfaces_without_retry() {
        let manager = Arc::new(McpClientManager::new());
        let (serving, _, _) = fake_server_with_config(
            vec![delayed_exchange(
                Err(TransportError::Timeout(
                    "timed out reading response for 'tools/call'".to_string(),
                )),
                200,
            )],
            stdio_test_config("serving"),
        );
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), serving);

        let (replacement, replacement_requests, _) =
            fake_server_with_config(Vec::new(), stdio_test_config("replacement"));
        let swapper = Arc::clone(&manager);
        let swap = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            swapper
                .servers
                .write()
                .await
                .insert("fake".to_string(), replacement);
        });

        let err = manager
            .call_tool("fake", "report", serde_json::json!({}))
            .await
            .expect_err("stale non-recoverable error must surface");
        swap.await.expect("swap task");

        assert!(err.contains("timed out"), "got: {err}");
        assert_eq!(
            replacement_requests.lock().unwrap().len(),
            0,
            "a non-recoverable failure must not re-execute on the replacement"
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
        if os.environ.get("MCP_EMIT_PING"):
            # A server-initiated request whose id collides with the client's.
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "method": "ping"}) + "\n")
            sys.stdout.flush()
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

        // Port 1 refuses connections, so the HTTP handshake fails fast.
        let changed = McpServerConfig::Http {
            url: "http://127.0.0.1:1/mcp".to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
        };
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;

        // The variant switch is a config change: the stdio server is torn
        // down and the (unreachable) HTTP replacement is a restart failure,
        // never "unchanged".
        assert!(result.unchanged.is_empty());
        assert!(result.restarted.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].server, "fixture");
        assert_eq!(result.failed[0].action, "restart");
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

    #[tokio::test]
    async fn stdio_server_request_with_colliding_id_is_not_mistaken_for_reply() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = McpClientManager::new();
        let mut config = fixture_config(&script, &marker, "v1", "env1");
        as_stdio_mut(&mut config)
            .2
            .insert("MCP_EMIT_PING".to_string(), "1".to_string());
        connect_fixture(&manager, &config).await;

        // The fixture emits a server-initiated `ping` request reusing the
        // call's own id before answering; it must be forwarded to the sink,
        // not parsed as a result-less reply that fails the call.
        let output = manager
            .call_tool("fixture", "report", serde_json::json!({}))
            .await
            .expect("ping must not be mistaken for the reply");

        assert_eq!(output, "label=v1;env=env1");
        manager.shutdown().await;
    }
}
