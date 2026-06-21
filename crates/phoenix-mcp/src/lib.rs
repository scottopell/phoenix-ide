//! MCP (Model Context Protocol) client engine.
//!
//! The JSON-RPC 2.0 protocol layer (`initialize`, paginated `tools/list`,
//! `tools/call`, notification handling) lives on `McpServer` and is
//! transport-agnostic; how a request's bytes leave and a response's bytes
//! arrive is behind the `McpTransport` trait. `StdioTransport` reaches a
//! server spawned as a child subprocess; `HttpTransport` reaches a remote
//! server over the Streamable HTTP transport. Discovered tools are surfaced
//! to callers as [`McpToolDef`] metadata and invoked via
//! [`McpClientManager::call_tool`]; the thin `Tool`-trait wrapper that exposes
//! them as Phoenix tools lives in the `phoenix-tools` crate. Spec: `specs/mcp/`.

pub mod http;
pub mod oauth;
pub mod stdio;

pub use http::HttpTransport;
pub use stdio::StdioTransport;

use async_trait::async_trait;
use oauth::{OAuthRegistrationRecord, OAuthStore, OAuthTokenRecord};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// The OAuth bearer for one HTTP server, shared between the manager (which
/// seeds it from the token store and rotates it on refresh) and the server's
/// transports (which attach it to every request, REQ-MCP-012). `None` until a
/// token exists.
pub type SharedBearer = Arc<std::sync::RwLock<Option<String>>>;

/// Timeout for a single JSON-RPC request-response round trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Longer timeout for initialize + tools/list during server connection.
/// Five minutes gives OAuth flows (mcp-remote prompts, browser redirect) time to complete.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Upper bound for an HTTP reload request applying changed existing configs.
const RELOAD_RESTART_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for a fire-and-forget JSON-RPC notification; notifications never
/// legitimately take as long as a tool call.
pub(crate) const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// The transport observed its session reset out-of-band -- a session-bearing
    /// GET-stream 404 (REQ-MCP-005, REQ-MCP-006). The stream cannot rebuild the
    /// transport itself; marking the tool list stale routes the next
    /// `tool_definitions` read through the lazy-refresh path, whose
    /// `SessionExpired` re-establishes the connection (new session + fresh GET
    /// stream). Default no-op for sinks with no such state.
    fn on_session_reset(&self) {}
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
/// `oauth_bearer` is the server's shared OAuth bearer cell, attached by the
/// HTTP transport to every request unless a static credential supersedes it.
/// `sink` is the protocol-layer handler the HTTP transport drives from its
/// server-initiated GET stream (REQ-MCP-006); stdio has no such stream and
/// ignores it (its notifications ride inline on the per-request sink).
///
/// # Errors
/// Returns a display string when the transport cannot be established.
async fn connect_transport(
    name: &str,
    config: &McpServerConfig,
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    oauth_bearer: &SharedBearer,
    sink: Arc<dyn ServerMessageSink>,
) -> Result<Box<dyn McpTransport>, String> {
    match config {
        McpServerConfig::Stdio { command, args, env } => Ok(Box::new(
            StdioTransport::spawn(name, command, args, env, pending_oauth_urls).await?,
        )),
        McpServerConfig::Http { url, headers, auth } => Ok(Box::new(HttpTransport::connect(
            name,
            url,
            headers,
            auth,
            Arc::clone(oauth_bearer),
            sink,
        )?)),
    }
}

/// Record a terminal connect/handshake/authorization failure so the status API
/// retains it (REQ-MCP-018). A server still awaiting authorization (its pending
/// OAuth URL is set) is surfaced as `unauthorized`, not failed, so it is
/// deliberately left out of the failed set. Cleared on the next successful
/// (re)connect or on config removal.
async fn record_connect_failure(
    failed_servers: &RwLock<HashMap<String, FailureRecord>>,
    pending_oauth_urls: &RwLock<HashMap<String, String>>,
    name: &str,
    config: &McpServerConfig,
    error: String,
) {
    if pending_oauth_urls.read().await.contains_key(name) {
        tracing::info!(server = %name, "MCP server awaiting authorization: {error}");
        return;
    }
    tracing::warn!(server = %name, error = %error, "MCP server failed to connect");
    failed_servers
        .write()
        .await
        .insert(name.to_string(), FailureRecord::from_config(config, error));
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

/// The in-flight connect attempts, keyed by server name: the ticket that
/// identifies the current attempt plus the config it is connecting. An entry
/// exists exactly while an attempt is in flight -- `publish_if_current`
/// removes it on publication and `clear_ticket_if_current` on failure -- so
/// reload can distinguish "a connect is already underway for this config"
/// from "nothing is happening".
type ConnectTickets = std::sync::Mutex<HashMap<String, (u64, McpServerConfig)>>;

/// Publish a freshly connected server -- but only if `ticket` is still the
/// current connect attempt for `name`. A connect that outlived its reload
/// (e.g. abandoned at the reload deadline) must not resurrect a server a
/// newer reload removed, or displace its replacement with stale config; a
/// superseded server is terminated instead (ending any session it created).
/// The check and the insert share the `servers` write lock so a concurrent
/// reload's ticket revocation cannot interleave between them; a matching
/// ticket entry is consumed (the attempt is finished). Returns whether the
/// server was published.
pub(crate) async fn publish_if_current(
    servers: &RwLock<HashMap<String, McpServer>>,
    tickets: &ConnectTickets,
    name: &str,
    ticket: u64,
    server: McpServer,
) -> bool {
    let (published, mut leftover) = {
        let mut servers = servers.write().await;
        let mut tickets = tickets.lock().unwrap();
        if tickets.get(name).map(|(current, _)| *current) == Some(ticket) {
            tickets.remove(name);
            drop(tickets);
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

/// What a reload's changed-config branch found when (re)taking a server's
/// map slot after settling any hold on it.
enum Slot {
    /// The old-config server, removed and owned for termination.
    Old(Box<McpServer>),
    /// The desired config is already running; nothing to restart.
    Desired,
    /// The slot is empty with no hold; the new connect fills the vacancy.
    Vacant,
}

/// Consume `name`'s ticket entry if it still belongs to this attempt. Called
/// on a failed connect: a dead attempt must not leave its ticket parked, or
/// a later reload would mistake it for an in-flight connect and decline to
/// start a replacement.
/// Clear `name`'s ticket entry if it still belongs to this attempt. Returns
/// whether it was current -- a `false` means a later reload/removal revoked
/// this attempt, so its outcome (including a failure record) must be discarded
/// rather than applied over the newer state.
fn clear_ticket_if_current(tickets: &ConnectTickets, name: &str, ticket: u64) -> bool {
    let mut tickets = tickets.lock().unwrap();
    if tickets.get(name).map(|(current, _)| *current) == Some(ticket) {
        tickets.remove(name);
        true
    } else {
        false
    }
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

/// The lifecycle state surfaced for a server (REQ-MCP-013, REQ-MCP-018). The
/// transient `connecting`/`reconnecting` states of `mcp.allium`'s `ConnState`
/// are not separately retained -- the status API distinguishes the three
/// states an operator acts on: a healthy server, one awaiting authorization,
/// and one that failed.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnState {
    Ready,
    Unauthorized,
    Failed,
}

/// Which transport a configured server uses, surfaced for the status panel.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    Http,
}

/// The declared auth scheme of a configured server, surfaced for the panel. A
/// `none` HTTP server still drives OAuth discovery on a 401; the `state` +
/// `pending_oauth_url` convey that, while this reflects the config as written.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthKind {
    None,
    Static,
    Oauth,
}

/// Status of one MCP server (for API responses, REQ-MCP-013, REQ-MCP-018).
/// A failed server is retained here with its error rather than vanishing, so a
/// misconfiguration is distinguishable from a server merely awaiting auth.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub state: McpConnState,
    pub transport: McpTransportKind,
    pub auth: McpAuthKind,
    pub tool_count: usize,
    pub tools: Vec<String>,
    pub enabled: bool,
    /// Set while the server is waiting for the user to complete an OAuth flow.
    pub pending_oauth_url: Option<String>,
    /// The failure cause when `state = failed`, cleared on a successful
    /// reconnect (REQ-MCP-018).
    pub last_error: Option<String>,
    /// On an `unauthorized` entry, a diagnostic when the OAuth redirect base is
    /// unreachable from another machine, so the authorize link will fail
    /// (REQ-MCP-020).
    pub auth_redirect_warning: Option<String>,
}

/// A connect/handshake/authorization failure retained for the status API
/// (REQ-MCP-018). Carries the transport/auth of the configured server so the
/// panel can render it without the server being in the connected map.
#[derive(Debug, Clone)]
struct FailureRecord {
    error: String,
    transport: McpTransportKind,
    auth: McpAuthKind,
}

impl FailureRecord {
    fn from_config(config: &McpServerConfig, error: String) -> Self {
        Self {
            error,
            transport: config.transport_kind(),
            auth: config.auth_kind(),
        }
    }
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

/// Failure establishing a connection (transport build or handshake). The 401
/// classification survives to the connect path, which dispatches on it:
/// `StaticAuthRejected` for config credentials, the OAuth entry point
/// otherwise (REQ-MCP-008, REQ-MCP-009).
#[derive(Debug)]
enum HandshakeFailure {
    /// The handshake was rejected with HTTP 401; carries the
    /// `WWW-Authenticate` challenge when present.
    Unauthorized {
        www_authenticate: Option<String>,
        message: String,
    },
    Other(String),
}

impl HandshakeFailure {
    fn classify(error: McpRequestError, server_name: &str) -> Self {
        match error {
            McpRequestError::Transport(TransportError::Unauthorized { www_authenticate }) => {
                Self::Unauthorized {
                    www_authenticate,
                    message: format!("MCP server '{server_name}': unauthorized (HTTP 401)"),
                }
            }
            other @ (McpRequestError::Transport(_) | McpRequestError::Other(_)) => {
                Self::Other(other.into_message(server_name))
            }
        }
    }
}

impl std::fmt::Display for HandshakeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized { message, .. } | Self::Other(message) => write!(f, "{message}"),
        }
    }
}

/// Protocol-layer handling of server-initiated messages forwarded by the
/// transport: flags `tools/list_changed` for lazy refresh, logs and drops
/// everything else. The same sink handles messages on a POST reply stream and
/// on the long-lived server-initiated GET stream (REQ-MCP-006), so it owns its
/// state (shared `tools_changed`) rather than borrowing the server: the GET
/// stream runs in a detached task that outlives any one request.
struct NotificationSink {
    server: String,
    tools_changed: Arc<AtomicBool>,
}

impl ServerMessageSink for NotificationSink {
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

    fn on_session_reset(&self) {
        // The session is gone; force the next definitions read to re-verify,
        // which re-establishes the connection on the expired-session tools/list.
        tracing::info!(
            server = %self.server,
            "GET stream observed a session reset -- will re-establish on next definitions() call"
        );
        self.tools_changed.store(true, Ordering::Release);
    }
}

/// Build the protocol-layer sink for a server, shared between the per-request
/// path and the transport's server-initiated GET stream.
fn notification_sink(name: &str, tools_changed: &Arc<AtomicBool>) -> Arc<dyn ServerMessageSink> {
    Arc::new(NotificationSink {
        server: name.to_string(),
        tools_changed: Arc::clone(tools_changed),
    })
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
    /// Cleared after the next `list_tools()` refresh. Shared (`Arc`) because
    /// the HTTP transport's server-initiated GET stream sets it from a
    /// detached task (REQ-MCP-006), not only the per-request sink.
    tools_changed: Arc<AtomicBool>,
    /// Shared map of server name → OAuth URL; written by the stdio stderr
    /// drain, read by `McpClientManager::status()`. Retained so a respawned
    /// transport keeps feeding the same map.
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    /// The OAuth bearer this server's transports attach to every request
    /// (REQ-MCP-012). Seeded from the token store at connect, rotated in
    /// place on refresh, and retained across re-establish so a rebuilt
    /// transport keeps the credential.
    oauth_bearer: SharedBearer,
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
        oauth_bearer: SharedBearer,
    ) -> Result<Self, String> {
        let tools_changed = Arc::new(AtomicBool::new(false));
        let transport = connect_transport(
            name,
            &config,
            Arc::clone(&pending_oauth_urls),
            &oauth_bearer,
            notification_sink(name, &tools_changed),
        )
        .await?;
        Ok(Self {
            name: name.to_string(),
            transport,
            tools: Vec::new(),
            config,
            generation: next_generation(),
            tools_changed,
            pending_oauth_urls,
            oauth_bearer,
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
            server: self.name.clone(),
            tools_changed: Arc::clone(&self.tools_changed),
        };
        self.transport.request(method, params, timeout, &sink).await
    }

    /// Send the JSON-RPC `initialize` handshake followed by the
    /// `notifications/initialized` notification.
    ///
    /// # Errors
    /// Returns a `McpRequestError` when the handshake request or response
    /// fails, so callers can dispatch on the transport classification.
    pub async fn initialize(&mut self) -> Result<(), McpRequestError> {
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
            .map_err(McpRequestError::Transport)?;

        // Send the initialized notification (no id, no response expected).
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.transport
            .notify(&notification)
            .await
            .map_err(McpRequestError::Transport)?;

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

    /// Whether this server is operating under an OAuth bearer (REQ-MCP-012):
    /// an OAuth-eligible HTTP config with a token attached. The re-auth
    /// recovery paths (silent refresh, scope step-up) only apply here — a 401
    /// on a static-credential or never-authorized server is not refreshable.
    fn oauth_active(&self) -> bool {
        oauth_resource_url(&self.config).is_some() && self.oauth_bearer.read().unwrap().is_some()
    }

    /// Run the post-connect handshake: `initialize` then the first
    /// `tools/list`.
    ///
    /// On failure the transport is shut down before returning: `initialize`
    /// may already have created a server-side HTTP session, and dropping the
    /// transport without the session DELETE would leak it until expiry
    /// (REQ-MCP-005).
    async fn handshake(&mut self) -> Result<(), HandshakeFailure> {
        let Err(error) = self.handshake_attempt().await else {
            return Ok(());
        };
        let recoverable = self.should_reestablish(&error);
        let failure = HandshakeFailure::classify(error, &self.name);
        self.terminate().await;
        if !recoverable {
            return Err(failure);
        }

        // A recoverable transport failure mid-handshake -- e.g. the server
        // dropped the just-created session before the first tools/list --
        // gets one retry on a fresh connection rather than skipping an
        // otherwise reachable server (REQ-MCP-005).
        tracing::warn!(
            server = %self.name,
            error = %failure,
            "Handshake hit a recoverable transport failure; retrying once on a fresh connection"
        );
        self.transport = connect_transport(
            &self.name,
            &self.config,
            Arc::clone(&self.pending_oauth_urls),
            &self.oauth_bearer,
            notification_sink(&self.name, &self.tools_changed),
        )
        .await
        .map_err(HandshakeFailure::Other)?;
        self.generation = next_generation();

        match self.handshake_attempt().await {
            Ok(()) => Ok(()),
            Err(error) => {
                let failure = HandshakeFailure::classify(error, &self.name);
                self.terminate().await;
                Err(failure)
            }
        }
    }

    /// One handshake pass: `initialize` then the first `tools/list`.
    async fn handshake_attempt(&mut self) -> Result<(), McpRequestError> {
        self.initialize().await?;
        self.list_tools().await?;
        Ok(())
    }

    /// Rebuild the transport from the retained config and re-run the
    /// handshake (stdio: respawn the process; HTTP: fresh client + session).
    async fn reestablish(&mut self) -> Result<(), HandshakeFailure> {
        self.terminate().await;

        self.transport = connect_transport(
            &self.name,
            &self.config,
            Arc::clone(&self.pending_oauth_urls),
            &self.oauth_bearer,
            notification_sink(&self.name, &self.tools_changed),
        )
        .await
        .map_err(HandshakeFailure::Other)?;
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
// OAuth lifecycle (REQ-MCP-009..013) -- the manager-side orchestration of the
// protocol mechanics in `mcp/oauth.rs`.
// ---------------------------------------------------------------------------

/// The pre-configured OAuth client id a config carries, if any. `None` for a
/// dynamically registered client and for non-OAuth configs. A change here
/// invalidates a stored token: it was minted under the old client identity.
fn preconfigured_client_id(config: &McpServerConfig) -> Option<&str> {
    match config {
        McpServerConfig::Http {
            auth: HttpAuth::OAuth(Some(preconfigured)),
            ..
        } => Some(&preconfigured.client_id),
        McpServerConfig::Http { .. } | McpServerConfig::Stdio { .. } => None,
    }
}

/// The URL of an HTTP server whose 401s drive the OAuth flow: an explicit
/// static credential opts out (`StaticAuthRejected`, REQ-MCP-008), stdio has no
/// auth at all.
fn oauth_resource_url(config: &McpServerConfig) -> Option<&str> {
    match config {
        McpServerConfig::Http {
            url,
            auth: HttpAuth::None | HttpAuth::OAuth(_),
            ..
        } => Some(url),
        McpServerConfig::Http {
            auth: HttpAuth::Static(_),
            ..
        }
        | McpServerConfig::Stdio { .. } => None,
    }
}

/// One in-flight authorization awaiting the operator's browser round trip
/// (`oauth_phase = awaiting_user` in `specs/mcp/mcp.allium`). Keyed by server
/// name in `OAuthRuntime::pending`; the `state_nonce` binds the callback to
/// exactly this flow, and dropping the flow (resolution, cancellation, or
/// displacement by a newer flow) releases any held recovery claim.
struct PendingAuthFlow {
    config: McpServerConfig,
    state_nonce: String,
    pkce_verifier: String,
    redirect_uri: String,
    token_endpoint: String,
    issuer: String,
    /// The authorization server advertises
    /// `authorization_response_iss_parameter_supported`, so a callback
    /// without a matching `iss` is rejected (REQ-MCP-011).
    iss_required: bool,
    registration: OAuthRegistrationRecord,
    resource: String,
    scopes: Vec<String>,
    /// Held when a scope step-up removed a ready server from the map: keeps
    /// callers parked on the claim until the re-authorized server is
    /// republished, so the triggering call replays instead of failing
    /// (deferred `ReAuthCallRetry`).
    claim: Option<ServerClaim>,
}

/// The cloneable subset of a pending flow that the callback path reads before
/// exchanging the code (the flow itself stays parked until the exchange
/// succeeds, so a failed exchange leaves the URL retryable).
#[derive(Clone)]
struct FlowSnapshot {
    pkce_verifier: String,
    redirect_uri: String,
    token_endpoint: String,
    issuer: String,
    iss_required: bool,
    registration: OAuthRegistrationRecord,
    resource: String,
    scopes: Vec<String>,
}

impl PendingAuthFlow {
    fn snapshot(&self) -> FlowSnapshot {
        FlowSnapshot {
            pkce_verifier: self.pkce_verifier.clone(),
            redirect_uri: self.redirect_uri.clone(),
            token_endpoint: self.token_endpoint.clone(),
            issuer: self.issuer.clone(),
            iss_required: self.iss_required,
            registration: self.registration.clone(),
            resource: self.resource.clone(),
            scopes: self.scopes.clone(),
        }
    }
}

/// Manager-side OAuth state, shared (via `Arc`) with the connect tasks that
/// outlive a `&self` borrow. Persistence is behind `OAuthStore`; the redirect
/// base is the server's own externally reachable address, set once the
/// listener is bound.
struct OAuthRuntime {
    store: std::sync::RwLock<Arc<dyn OAuthStore>>,
    redirect_base: std::sync::Mutex<Option<String>>,
    /// A diagnostic surfaced on `unauthorized` status entries when the resolved
    /// redirect base is unreachable from another machine (a loopback redirect
    /// on a remote-reachable bind, REQ-MCP-020), so the operator sees why the
    /// authorize link will fail before opening it.
    redirect_warning: std::sync::Mutex<Option<String>>,
    pending: std::sync::Mutex<HashMap<String, PendingAuthFlow>>,
    /// Loopback callback listeners, keyed by server name. A server whose
    /// pre-registered OAuth app only allows a fixed `http://localhost:<port>/callback`
    /// redirect (it cannot register Phoenix's own callback route) gets a listener
    /// on that port that bounces the browser to the real callback route
    /// (REQ-MCP-020). The `JoinHandle` is held so a re-prompt can abort *and
    /// await* a stale listener — releasing the port before rebinding it — and so
    /// a completed flow can abort its listener.
    loopback_listeners: std::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl Default for OAuthRuntime {
    fn default() -> Self {
        Self {
            store: std::sync::RwLock::new(Arc::new(oauth::MemoryOAuthStore::default())),
            redirect_base: std::sync::Mutex::new(None),
            redirect_warning: std::sync::Mutex::new(None),
            pending: std::sync::Mutex::new(HashMap::new()),
            loopback_listeners: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl OAuthRuntime {
    fn store(&self) -> Arc<dyn OAuthStore> {
        Arc::clone(&self.store.read().unwrap())
    }

    /// The redirect-unreachable diagnostic, when one applies (REQ-MCP-020).
    fn redirect_warning(&self) -> Option<String> {
        self.redirect_warning.lock().unwrap().clone()
    }

    /// The local callback the authorization server redirects to; `None` until
    /// the server address is known.
    fn redirect_uri(&self) -> Option<String> {
        self.redirect_base
            .lock()
            .unwrap()
            .as_ref()
            .map(|base| format!("{}/api/mcp/oauth/callback", base.trim_end_matches('/')))
    }

    /// Phoenix's own externally reachable origin (no callback path), the bounce
    /// target for a loopback listener. `None` until the server address is known.
    fn redirect_base(&self) -> Option<String> {
        self.redirect_base.lock().unwrap().clone()
    }
}

/// Failure from a token refresh. The split decides the stored token's fate
/// (REQ-MCP-012): a definitive rejection discards it and re-prompts, while a
/// transport failure is no evidence of staleness — the token is kept and the
/// next 401 retries the refresh.
enum RefreshFailure {
    Rejected(String),
    Transient(String),
}

/// Resolve a usable authorization server from the resource's advertised list
/// (REQ-MCP-009, REQ-MCP-010): the first issuer with both valid metadata and
/// an existing client registration wins; otherwise the first with valid
/// metadata (the DCR fallback). An advertised server that fails discovery
/// (unreachable, no PKCE, malformed metadata) is skipped rather than failing
/// the flow while a usable sibling remains.
async fn resolve_authorization_server(
    oauth_rt: &OAuthRuntime,
    client: &reqwest::Client,
    prm: &oauth::ProtectedResourceMetadata,
) -> Result<(oauth::AuthServerMetadata, Option<OAuthRegistrationRecord>), String> {
    let mut fallback: Option<oauth::AuthServerMetadata> = None;
    let mut errors: Vec<String> = Vec::new();
    for issuer in &prm.authorization_servers {
        let metadata = match oauth::fetch_auth_server_metadata(
            client,
            issuer,
            oauth::IssuerTrust::ResourceAdvertised,
        )
        .await
        {
            Ok(metadata) => metadata,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        match oauth_rt.store().registration(&metadata.issuer).await? {
            Some(registration) => return Ok((metadata, Some(registration))),
            None => {
                if fallback.is_none() {
                    fallback = Some(metadata);
                }
            }
        }
    }
    match fallback {
        Some(metadata) => Ok((metadata, None)),
        None => Err(format!(
            "no usable authorization server among those advertised: {}",
            errors.join("; ")
        )),
    }
}

/// Refresh a server's token: re-discover the authorization server from the
/// challenge (endpoints are not persisted), then exchange the refresh token,
/// persisting the rotated credentials (REQ-MCP-012). Returns the new access
/// token.
async fn oauth_refresh(
    oauth_rt: &OAuthRuntime,
    name: &str,
    url: &str,
    www_authenticate: Option<&str>,
    token: &OAuthTokenRecord,
) -> Result<String, RefreshFailure> {
    let Some(refresh_token) = token.refresh_token.clone() else {
        return Err(RefreshFailure::Rejected(
            "stored token has no refresh token".to_string(),
        ));
    };
    let client = oauth::oauth_http_client().map_err(RefreshFailure::Transient)?;
    let challenge = www_authenticate
        .map(oauth::parse_bearer_challenge)
        .unwrap_or_default();
    let prm = oauth::fetch_protected_resource_metadata(&client, url, &challenge)
        .await
        .map_err(RefreshFailure::Transient)?;
    let (metadata, registration) = resolve_authorization_server(oauth_rt, &client, &prm)
        .await
        .map_err(RefreshFailure::Transient)?;
    let registration = registration.ok_or_else(|| {
        // Without a client identity the grant can never succeed; treat as
        // a rejection so the dead token is discarded and a fresh flow
        // (which registers a client) takes over.
        RefreshFailure::Rejected(format!(
            "no client registration for authorization server '{}'",
            metadata.issuer
        ))
    })?;
    let response = oauth::refresh_grant(
        &client,
        &metadata.token_endpoint,
        &registration,
        &refresh_token,
        &token.resource,
    )
    .await
    .map_err(|e| match e {
        oauth::TokenGrantError::Rejected(detail) => RefreshFailure::Rejected(detail),
        oauth::TokenGrantError::Transport(detail) => RefreshFailure::Transient(detail),
    })?;

    let record = OAuthTokenRecord {
        server_name: name.to_string(),
        resource: token.resource.clone(),
        scopes: response
            .scopes
            .clone()
            .unwrap_or_else(|| token.scopes.clone()),
        access_token: response.access_token.clone(),
        // A rotating server replaces the refresh token; one that does not
        // rotate omits it, keeping the existing one (REQ-MCP-012).
        refresh_token: response.refresh_token.clone().or(Some(refresh_token)),
        expires_at: response.expires_at,
    };
    oauth_rt
        .store()
        .upsert_token(&record)
        .await
        .map_err(RefreshFailure::Transient)?;
    tracing::info!(server = %name, "Refreshed MCP OAuth access token");
    Ok(response.access_token)
}

/// Resolve the OAuth client identity for an authorization server (REQ-MCP-010):
/// reuse a cached client whose registered `redirect_uri` still matches the
/// resolved redirect base (or is unknown), otherwise dynamically register
/// (RFC 7591). A cached client registered with a *different* `redirect_uri` is
/// re-registered so an authorization server that binds clients to their
/// redirect accepts the new callback after the canonical base changed
/// (REQ-MCP-020); when the server advertises no registration endpoint it is
/// reused best-effort with a warning. Phoenix hosts no Client ID Metadata
/// Document, so that step resolves to nothing.
async fn acquire_client_registration(
    oauth_rt: &OAuthRuntime,
    client: &reqwest::Client,
    name: &str,
    metadata: &oauth::AuthServerMetadata,
    cached: Option<OAuthRegistrationRecord>,
    preconfigured: Option<&PreconfiguredClient>,
    redirect_uri: &str,
) -> Result<OAuthRegistrationRecord, String> {
    if let Some(cached) = &cached {
        // Reuse a cached registration only if it still matches the current
        // client identity and redirect. A changed pre-configured client_id
        // (the config was re-keyed) must replace the stale row, not authorize
        // against the old app forever (the registration is keyed by issuer, so
        // the client_id is not in the key).
        let client_matches = preconfigured.is_none_or(|pre| pre.client_id == cached.client_id);
        let redirect_matches = cached
            .redirect_uri
            .as_deref()
            .is_none_or(|registered| registered == redirect_uri);
        if client_matches && redirect_matches {
            return Ok(cached.clone());
        }
    }

    // A pre-configured client identity is seeded — discovery has resolved the
    // issuer the config could not name (REQ-MCP-010, preferred over DCR).
    // Reached when there is no cached row or the cached one no longer matches
    // the configured client_id; the upsert (keyed by issuer) replaces a stale
    // registration. The redirect is registered out of band by the operator, so
    // it is left unknown and not compared (REQ-MCP-020). Public client: no
    // secret, so the token endpoint uses `none` client authentication (PKCE).
    if let Some(pre) = preconfigured {
        let registration = OAuthRegistrationRecord {
            auth_server: metadata.issuer.clone(),
            client_id: pre.client_id.clone(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_string(),
            redirect_uri: None,
        };
        oauth_rt.store().upsert_registration(&registration).await?;
        tracing::info!(
            server = %name,
            auth_server = %metadata.issuer,
            client_id = %registration.client_id,
            "Seeded pre-configured OAuth client (authorization server disables DCR)"
        );
        return Ok(registration);
    }

    let Some(endpoint) = metadata.registration_endpoint.clone() else {
        return match cached {
            Some(cached) => {
                tracing::warn!(
                    server = %name,
                    auth_server = %metadata.issuer,
                    "OAuth redirect base changed but the authorization server advertises no \
                     registration endpoint; reusing the existing client (its registered \
                     redirect_uri may no longer match)"
                );
                Ok(cached)
            }
            None => Err(format!(
                "authorization server '{}' has no client registration for Phoenix and does not \
                 advertise a registration endpoint (RFC 7591); configure a pre-registered OAuth \
                 client for it",
                metadata.issuer
            )),
        };
    };

    let registration = oauth::register_client(client, metadata, &endpoint, redirect_uri).await?;
    oauth_rt.store().upsert_registration(&registration).await?;
    if cached.is_some() {
        tracing::info!(
            server = %name,
            auth_server = %metadata.issuer,
            "Re-registered OAuth client after a redirect base change"
        );
    } else {
        tracing::info!(
            server = %name,
            auth_server = %metadata.issuer,
            client_id = %registration.client_id,
            "Registered OAuth client via dynamic client registration"
        );
    }
    Ok(registration)
}

/// Run discovery → client identity → PKCE for an HTTP server that needs the
/// operator's authorization, park the pending flow, and surface its URL
/// (REQ-MCP-009..011, REQ-MCP-013). `extra_scopes` carries prior grants for a
/// step-up union; `claim` keeps callers parked until re-authorization
/// completes (it is released on every failure path by drop).
/// Extract the query string (including the leading `?`) from an HTTP request's
/// first line — e.g. `GET /callback?code=x&state=y HTTP/1.1` yields
/// `?code=x&state=y`. Empty when the target has no query. The bytes are
/// forwarded verbatim to Phoenix's real callback route, so the percent-encoding
/// the authorization server produced is preserved.
fn callback_request_query(request: &str) -> String {
    let target = request
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("");
    target
        .split_once('?')
        .map(|(_, query)| format!("?{query}"))
        .unwrap_or_default()
}

/// Bind loopback listeners on `port` for both IPv4 (`127.0.0.1`) and IPv6
/// (`::1`), so a browser resolving `localhost` to either family reaches the
/// callback (REQ-MCP-020). Returns every address that bound; errs only if
/// *neither* did (the port is genuinely unavailable). A few short retries
/// absorb the brief window where a just-aborted prior listener is still
/// releasing the port.
async fn bind_loopback_listeners(port: u16) -> std::io::Result<Vec<tokio::net::TcpListener>> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let mut last_err = None;
    for attempt in 0..5 {
        let mut bound = Vec::new();
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ] {
            match tokio::net::TcpListener::bind((ip, port)).await {
                Ok(listener) => bound.push(listener),
                // An IPv6-less host (or vice versa) is fine as long as the
                // other family bound; only a total failure is fatal.
                Err(e) => last_err = Some(e),
            }
        }
        if !bound.is_empty() {
            return Ok(bound);
        }
        if attempt < 4 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    Err(last_err
        .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrInUse, "no loopback bound")))
}

/// Bounce the authorization server's callbacks (arriving on the pre-bound
/// loopback `listeners`) to Phoenix's real callback route under `target_base`
/// (REQ-MCP-020). Accepts in a loop so a retry after a failed token exchange —
/// which leaves the flow pending — is still received, until the flow resolves
/// (the listener is aborted) or the 5-minute flow window elapses.
async fn run_loopback_redirect(
    server: String,
    listeners: Vec<tokio::net::TcpListener>,
    target_base: String,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            tracing::debug!(server = %server, "OAuth loopback listener window elapsed");
            return;
        }
        let accepts = listeners
            .iter()
            .map(|l| Box::pin(l.accept()))
            .collect::<Vec<_>>();
        match tokio::time::timeout(remaining, futures::future::select_all(accepts)).await {
            Ok((Ok((stream, _)), _, _)) => bounce_loopback_callback(stream, &target_base).await,
            Ok((Err(e), _, _)) => {
                tracing::warn!(server = %server, "OAuth loopback accept failed: {e}");
            }
            Err(_) => {
                tracing::debug!(server = %server, "OAuth loopback listener window elapsed");
                return;
            }
        }
    }
}

/// Read one callback request off `stream` and reply `302` to Phoenix's real
/// callback route, forwarding the query verbatim.
async fn bounce_loopback_callback(mut stream: tokio::net::TcpStream, target_base: &str) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let query = callback_request_query(&request);
    let location = format!(
        "{}/api/mcp/oauth/callback{}",
        target_base.trim_end_matches('/'),
        query
    );
    let body = "<!DOCTYPE html><html><body><p>Completing authorization, \
                returning to Phoenix…</p></body></html>";
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
    tracing::debug!("OAuth loopback callback bounced to {location}");
}

#[allow(clippy::too_many_lines)] // One ordered sequence: discover, acquire client, bind callback, publish.
async fn begin_oauth_flow(
    oauth_rt: &OAuthRuntime,
    pending_oauth_urls: &Arc<RwLock<HashMap<String, String>>>,
    name: &str,
    entry: &McpServerConfig,
    www_authenticate: Option<&str>,
    extra_scopes: Vec<String>,
    claim: Option<ServerClaim>,
) -> Result<String, String> {
    let Some(url) = oauth_resource_url(entry) else {
        return Err("server is not OAuth-eligible".to_string());
    };
    let preconfigured = match entry {
        McpServerConfig::Http {
            auth: HttpAuth::OAuth(Some(pre)),
            ..
        } => Some(pre),
        McpServerConfig::Http { .. } | McpServerConfig::Stdio { .. } => None,
    };
    // A pre-registered app whose allowlist pins a fixed loopback port redirects
    // there; Phoenix bounces that callback to its own route (REQ-MCP-020).
    // Otherwise the redirect is Phoenix's own server-route callback.
    let loopback_port = preconfigured.and_then(|pre| pre.callback_port);
    let redirect_uri = match loopback_port {
        Some(port) => format!("http://localhost:{port}/callback"),
        None => oauth_rt
            .redirect_uri()
            .ok_or("OAuth redirect base not configured (server address unknown)")?,
    };
    let client = oauth::oauth_http_client()?;
    let challenge = www_authenticate
        .map(oauth::parse_bearer_challenge)
        .unwrap_or_default();

    let prm = oauth::fetch_protected_resource_metadata(&client, url, &challenge).await?;
    let (metadata, cached_registration) =
        resolve_authorization_server(oauth_rt, &client, &prm).await?;

    let registration = acquire_client_registration(
        oauth_rt,
        &client,
        name,
        &metadata,
        cached_registration,
        preconfigured,
        &redirect_uri,
    )
    .await?;

    // Requested scopes: the challenge's `scope` when present, else the
    // resource's advertised set; unioned with any prior grants (step-up,
    // REQ-MCP-012).
    let mut scopes: Vec<String> = challenge
        .get("scope")
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| prm.scopes_supported.clone());
    for scope in extra_scopes {
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }

    let pkce = oauth::generate_pkce();
    let state_nonce = oauth::generate_state_nonce();
    let resource = oauth::canonical_resource(url);
    let auth_url = oauth::build_authorization_url(
        &metadata,
        &registration,
        &redirect_uri,
        &state_nonce,
        &pkce.challenge,
        &resource,
        &scopes,
    )?;

    let flow = PendingAuthFlow {
        config: entry.clone(),
        state_nonce,
        pkce_verifier: pkce.verifier,
        redirect_uri,
        token_endpoint: metadata.token_endpoint.clone(),
        issuer: metadata.issuer.clone(),
        iss_required: metadata.iss_parameter_supported,
        registration,
        resource,
        scopes,
        claim,
    };
    // Bind the loopback listener BEFORE anything is published, so a bind
    // failure (port in use) fails the flow with a clear error instead of
    // surfacing a sign-in URL whose callback can never be received. The
    // listener bounces the fixed-port callback to Phoenix's real route; the
    // server-route case needs no listener.
    if let Some(port) = loopback_port {
        let Some(base) = oauth_rt.redirect_base() else {
            return Err(format!(
                "MCP server '{name}': loopback OAuth callback needs Phoenix's own address to \
                 bounce to, but none is configured"
            ));
        };
        // Abort and await any prior listener for this server so the port is
        // released before we rebind it (a re-prompt during an active flow).
        let prior = oauth_rt.loopback_listeners.lock().unwrap().remove(name);
        if let Some(prior) = prior {
            prior.abort();
            let _ = prior.await;
        }
        let listeners = bind_loopback_listeners(port).await.map_err(|e| {
            format!(
                "MCP server '{name}': cannot bind OAuth callback port {port} on loopback \
                 (already in use?): {e}"
            )
        })?;
        let handle = tokio::spawn(run_loopback_redirect(name.to_string(), listeners, base));
        oauth_rt
            .loopback_listeners
            .lock()
            .unwrap()
            .insert(name.to_string(), handle);
    }

    // Inserting rotates the nonce: a displaced older flow (and its claim)
    // drops, so its callback no longer matches any pending flow.
    oauth_rt
        .pending
        .lock()
        .unwrap()
        .insert(name.to_string(), flow);
    pending_oauth_urls
        .write()
        .await
        .insert(name.to_string(), auth_url.clone());
    tracing::info!(
        server = %name,
        url = %auth_url,
        "MCP server requires OAuth authorization; waiting for the operator"
    );
    Ok(auth_url)
}

/// An OAuth re-authorization condition on an authorized server, classified
/// from a failed call (REQ-MCP-012).
enum OAuthRecoveryKind {
    /// 401: the access token expired or was revoked (`TokenRefreshNeeded`).
    Refresh { www_authenticate: Option<String> },
    /// 403 with an explicit `error="insufficient_scope"` challenge
    /// (`InsufficientScopeStepUp`). A plain 403 is not a step-up.
    StepUp { www_authenticate: String },
}

fn oauth_recovery_kind(server: &McpServer, error: &McpRequestError) -> Option<OAuthRecoveryKind> {
    let McpRequestError::Transport(transport_error) = error else {
        return None;
    };
    if !server.oauth_active() {
        return None;
    }
    match transport_error {
        TransportError::Unauthorized { www_authenticate } => Some(OAuthRecoveryKind::Refresh {
            www_authenticate: www_authenticate.clone(),
        }),
        TransportError::InsufficientScope {
            www_authenticate: Some(challenge),
        } if oauth::is_insufficient_scope_challenge(challenge) => Some(OAuthRecoveryKind::StepUp {
            www_authenticate: challenge.clone(),
        }),
        TransportError::InsufficientScope { .. }
        | TransportError::SessionExpired
        | TransportError::Disconnected(_)
        | TransportError::Timeout(_)
        | TransportError::Rpc { .. }
        | TransportError::Protocol(_) => None,
    }
}

/// Outcome of refreshing an authorized server's token mid-recovery.
enum RefreshServerOutcome {
    /// The bearer was rotated in place; the server can rejoin the map.
    Refreshed,
    /// The refresh could not be attempted/completed for a reason that says
    /// nothing about the token (network failure to the authorization
    /// server); the token and server are kept.
    Transient(String),
    /// The grant was definitively rejected: the token was discarded and a
    /// fresh authorization flow was surfaced (`TokenRefreshFailed`).
    Reprompt(String),
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
    /// transport failure, mid tool-list refresh, or awaiting an OAuth
    /// step-up): the holder parks a watch sender here via `ServerClaim`;
    /// calls that find the server absent subscribe and wait for the sender
    /// to drop (work finished) instead of failing with "not connected".
    recovering: RecoveringMap,
    /// The current connect attempt per server name. Every spawned connect
    /// (discovery, reload-added, reload-restart) records a ticket here and
    /// publishes its result only while that ticket is still current
    /// (`publish_if_current`), so an attempt outlived by a newer reload
    /// cannot resurrect a removed server or displace its replacement with
    /// stale config.
    connect_tickets: Arc<ConnectTickets>,
    /// Serializes reload reconciliations. Two interleaved reconciliations
    /// can each classify a server against state the other is mutating
    /// (e.g. both seeing the old config of a changed server, one revoking
    /// the other's restart without replacing it); reconciliation is only
    /// meaningful against a stable target, so reloads run one at a time.
    reload_serial: tokio::sync::Mutex<()>,
    /// Servers currently blocked on an OAuth flow: name → auth URL. Written
    /// by the native OAuth flow and by the stdio (`mcp-remote`) stderr drain;
    /// cleared when the server connects or its flow is cancelled. This is the
    /// structured `pending_auth_url` the status API serves (REQ-MCP-013).
    pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
    /// Servers whose connect/handshake/authorization gave up: name → failure,
    /// retained so the status API shows them with their cause rather than
    /// dropping them silently (REQ-MCP-018). Cleared on a successful (re)connect
    /// and on config removal.
    failed_servers: Arc<RwLock<HashMap<String, FailureRecord>>>,
    /// OAuth lifecycle state: the token/registration store, the local
    /// callback's base URL, and the pending authorization flows
    /// (REQ-MCP-009..012).
    oauth: Arc<OAuthRuntime>,
}

/// The claim map shared between the manager and parked `ServerClaim` guards.
type RecoveringMap = Arc<std::sync::Mutex<HashMap<String, tokio::sync::watch::Sender<()>>>>;

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
            recovering: Arc::new(std::sync::Mutex::new(HashMap::new())),
            connect_tickets: Arc::new(std::sync::Mutex::new(HashMap::new())),
            reload_serial: tokio::sync::Mutex::new(()),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
            failed_servers: Arc::new(RwLock::new(HashMap::new())),
            oauth: Arc::new(OAuthRuntime::default()),
        }
    }

    /// Swap in the persistent OAuth store (the default is in-memory). Called
    /// once at startup, before discovery, so stored tokens restore silently
    /// (REQ-MCP-012).
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic is already fatal).
    pub fn set_oauth_store(&self, store: Arc<dyn OAuthStore>) {
        *self.oauth.store.write().unwrap() = store;
    }

    /// Set the externally reachable base URL (scheme://host:port) the OAuth
    /// callback route is served under. Flows refuse to start until this is
    /// known — an authorization URL with an unreachable redirect would strand
    /// the operator in the browser.
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic is already fatal).
    pub fn set_oauth_redirect_base(&self, base: String) {
        *self.oauth.redirect_base.lock().unwrap() = Some(base);
    }

    /// Set (or clear) the redirect-unreachable diagnostic surfaced on
    /// `unauthorized` status entries (REQ-MCP-020).
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic is already fatal).
    pub fn set_oauth_redirect_warning(&self, warning: Option<String>) {
        *self.oauth.redirect_warning.lock().unwrap() = warning;
    }

    /// Drop a pending authorization flow, releasing any held claim and the
    /// surfaced URL (`PendingAuthorizationCancelled`). Returns the cancelled
    /// flow's config when one existed.
    async fn cancel_pending_oauth_flow(&self, name: &str) -> Option<McpServerConfig> {
        let flow = self.oauth.pending.lock().unwrap().remove(name);
        let config = flow.as_ref().map(|flow| flow.config.clone());
        drop(flow);
        if config.is_some() {
            self.pending_oauth_urls.write().await.remove(name);
            tracing::info!(server = %name, "Cancelled pending MCP OAuth authorization");
        }
        config
    }

    /// Discard a stored token the reloaded config can no longer use: the
    /// resource was repointed, auth moved away from OAuth, or the pre-configured
    /// client identity changed — a token minted under the old client must not
    /// restore against the new one (`ReloadInvalidatesOAuth`, REQ-MCP-012).
    async fn invalidate_oauth_on_config_change(
        &self,
        name: &str,
        old_config: &McpServerConfig,
        new_config: &McpServerConfig,
    ) {
        let token = match self.oauth.store().token(name).await {
            Ok(Some(token)) => token,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(server = %name, "OAuth token lookup failed during reload: {e}");
                return;
            }
        };
        let resource_matches = oauth_resource_url(new_config)
            .is_some_and(|url| oauth::canonical_resource(url) == token.resource);
        // The token row records neither its issuer nor its client, so a client
        // identity change is detected from the config delta: a token minted
        // under the old pre-configured client_id must not restore under a new
        // one. A dynamically discovered issuer that changed server-side is
        // caught at use instead: the resource 401s, the refresh fails, and the
        // failure path discards the token and re-prompts.
        let client_id_changed =
            preconfigured_client_id(old_config) != preconfigured_client_id(new_config);
        if !resource_matches || client_id_changed {
            tracing::info!(
                server = %name,
                "Reload repointed, de-OAuthed, or re-keyed this server's client; discarding its stored token"
            );
            if let Err(e) = self.oauth.store().delete_token(name).await {
                tracing::warn!(server = %name, "Failed to delete invalidated OAuth token: {e}");
            }
        }
    }

    /// Handle the OAuth redirect: validate `state` against the pending flow
    /// and `iss` against the discovered authorization server, exchange the
    /// code (with the PKCE verifier and resource indicator), persist the
    /// token, and reconnect the server in the background (REQ-MCP-011,
    /// REQ-MCP-012). Returns the server name on success.
    ///
    /// # Errors
    /// Returns a display string when the callback matches no pending flow,
    /// fails the `iss` check, or the code exchange is rejected. The flow
    /// stays pending on exchange failure so the operator can retry from the
    /// same authorization URL.
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic is already fatal).
    #[allow(clippy::too_many_lines)] // One ordered sequence: validate, exchange, persist, resolve, reconnect.
    pub async fn complete_oauth_authorization(
        self: &Arc<Self>,
        state_nonce: &str,
        code: &str,
        iss: Option<&str>,
    ) -> Result<String, String> {
        // The state nonce binds the callback to exactly one pending flow
        // (callback_state_matches): a code injected by another tab, server,
        // or a cancelled flow matches nothing and is rejected before any
        // exchange (REQ-MCP-011).
        let (name, flow) = {
            let pending = self.oauth.pending.lock().unwrap();
            let Some((name, flow)) = pending
                .iter()
                .find(|(_, flow)| flow.state_nonce == state_nonce)
            else {
                return Err(
                    "no pending MCP authorization matches this callback (state mismatch or \
                     cancelled flow)"
                        .to_string(),
                );
            };
            (name.clone(), flow.snapshot())
        };

        // The iss check defends against authorization-server mix-up
        // (redirect_issuer_valid): a state-valid callback delivered from a
        // different authorization server is rejected before the code reaches
        // the token endpoint (REQ-MCP-011).
        match iss {
            Some(iss) if iss.trim_end_matches('/') != flow.issuer.trim_end_matches('/') => {
                return Err(format!(
                    "callback 'iss' ({iss}) does not match the authorization server ({})",
                    flow.issuer
                ));
            }
            None if flow.iss_required => {
                return Err(format!(
                    "callback omitted the 'iss' parameter that authorization server '{}' \
                     advertises support for",
                    flow.issuer
                ));
            }
            _ => {}
        }

        let client = oauth::oauth_http_client()?;
        let response = oauth::exchange_code(
            &client,
            &flow.token_endpoint,
            &flow.registration,
            &flow.redirect_uri,
            code,
            &flow.pkce_verifier,
            &flow.resource,
        )
        .await
        .map_err(|e| format!("MCP server '{name}': authorization code exchange failed: {e}"))?;

        // Resolve the flow BEFORE persisting -- and only if it is still the
        // one this callback authorized. A reload may have cancelled/replaced
        // it during the exchange (a stale flow's token must not survive under
        // the new config), or a newer flow may already have persisted its own
        // token (which a stale exchange must not overwrite or delete). A dead
        // flow's exchange result is simply discarded.
        let resolved = {
            let mut pending = self.oauth.pending.lock().unwrap();
            match pending.get(&name) {
                Some(current) if current.state_nonce == state_nonce => pending.remove(&name),
                _ => None,
            }
        };
        let Some(resolved) = resolved else {
            return Err(format!(
                "MCP server '{name}': the authorization flow was superseded during the exchange"
            ));
        };

        let record = OAuthTokenRecord {
            server_name: name.clone(),
            resource: flow.resource.clone(),
            scopes: response
                .scopes
                .clone()
                .unwrap_or_else(|| flow.scopes.clone()),
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_at: response.expires_at,
        };
        self.oauth
            .store()
            .upsert_token(&record)
            .await
            .map_err(|e| format!("MCP server '{name}': failed to persist OAuth token: {e}"))?;

        self.pending_oauth_urls.write().await.remove(&name);
        // The flow is resolved; stop its loopback listener (if any) so it does
        // not hold the port for the rest of its window. On exchange *failure*
        // the flow stays pending and this is not reached, so the listener keeps
        // accepting for a retry (REQ-MCP-020).
        if let Some(handle) = self.oauth.loopback_listeners.lock().unwrap().remove(&name) {
            handle.abort();
        }
        let claim = resolved.claim;
        let config = resolved.config;

        // Reconnect in the background: the stored token restores onto the
        // first initialize. Any step-up claim is released only after the
        // publish attempt, so calls parked on it replay against the
        // re-authorized server (deferred ReAuthCallRetry).
        let manager = Arc::clone(self);
        let reconnect_name = name.clone();
        tokio::spawn(async move {
            let ticket = manager.issue_connect_ticket(&reconnect_name, &config);
            let result = Self::connect_one(
                &reconnect_name,
                &config,
                Arc::clone(&manager.pending_oauth_urls),
                Arc::clone(&manager.oauth),
            )
            .await;
            match result {
                Ok(server) => {
                    let tool_count = server.tools.len();
                    if publish_if_current(
                        &manager.servers,
                        &manager.connect_tickets,
                        &reconnect_name,
                        ticket,
                        server,
                    )
                    .await
                    {
                        manager.failed_servers.write().await.remove(&reconnect_name);
                        tracing::info!(
                            server = %reconnect_name,
                            tools = tool_count,
                            "MCP server connected after OAuth authorization"
                        );
                    }
                }
                Err(e) => {
                    // Record the failure only if this attempt still owns the
                    // ticket; a superseded one must not write stale state over
                    // a newer attempt or a removal (REQ-MCP-018).
                    if clear_ticket_if_current(&manager.connect_tickets, &reconnect_name, ticket) {
                        record_connect_failure(
                            &manager.failed_servers,
                            &manager.pending_oauth_urls,
                            &reconnect_name,
                            &config,
                            format!("connect after authorization failed: {e}"),
                        )
                        .await;
                    }
                }
            }
            drop(claim);
        });

        Ok(name)
    }

    /// Handle an error redirect from the authorization server (the operator
    /// denied, or the server rejected the request): cancel the flow it
    /// belongs to (`OAuthFlowFailed`). Returns the server name when a flow
    /// matched.
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic is already fatal).
    pub async fn fail_oauth_authorization(&self, state_nonce: &str, error: &str) -> Option<String> {
        let name = {
            let pending = self.oauth.pending.lock().unwrap();
            pending
                .iter()
                .find(|(_, flow)| flow.state_nonce == state_nonce)
                .map(|(name, _)| name.clone())
        }?;
        let config = self.cancel_pending_oauth_flow(&name).await;
        tracing::warn!(
            server = %name,
            error = %error,
            "MCP OAuth authorization failed at the authorization server"
        );
        // Retain the denial as a failure rather than letting the server vanish
        // from status (REQ-MCP-018). The pending URL is already cleared, so
        // this records `failed`, not `unauthorized`.
        if let Some(config) = config {
            record_connect_failure(
                &self.failed_servers,
                &self.pending_oauth_urls,
                &name,
                &config,
                format!("authorization failed: {error}"),
            )
            .await;
        }
        Some(name)
    }

    /// Refresh the token of an authorized server whose call just 401'd
    /// (`TokenRefreshNeeded` → `TokenRefreshed` / `TokenRefreshFailed`). On a
    /// definitive rejection the token is discarded and a fresh authorization
    /// flow is surfaced before returning `Reprompt`.
    async fn refresh_authorized_server(
        &self,
        server: &mut McpServer,
        www_authenticate: Option<&str>,
    ) -> RefreshServerOutcome {
        let name = server.name.clone();
        let config = server.config();
        let Some(url) = oauth_resource_url(&config).map(str::to_string) else {
            return RefreshServerOutcome::Transient(format!(
                "MCP server '{name}': not OAuth-eligible"
            ));
        };
        let token = match self.oauth.store().token(&name).await {
            Ok(Some(token)) => token,
            Ok(None) => {
                // The bearer cell is set but no row backs it (e.g. deleted by
                // a concurrent reload): nothing to refresh, re-prompt.
                return self
                    .reprompt_after_refresh_failure(
                        server,
                        &config,
                        www_authenticate,
                        "no stored token",
                    )
                    .await;
            }
            Err(e) => {
                return RefreshServerOutcome::Transient(format!(
                    "MCP server '{name}': OAuth token lookup failed: {e}"
                ));
            }
        };
        match oauth_refresh(&self.oauth, &name, &url, www_authenticate, &token).await {
            Ok(access_token) => {
                *server.oauth_bearer.write().unwrap() = Some(access_token);
                RefreshServerOutcome::Refreshed
            }
            Err(RefreshFailure::Transient(e)) => RefreshServerOutcome::Transient(format!(
                "MCP server '{name}': OAuth token refresh failed: {e}"
            )),
            Err(RefreshFailure::Rejected(e)) => {
                self.reprompt_after_refresh_failure(server, &config, www_authenticate, &e)
                    .await
            }
        }
    }

    /// `TokenRefreshFailed`: discard the dead token and surface a fresh
    /// authorization flow so the operator re-authorizes (REQ-MCP-012).
    async fn reprompt_after_refresh_failure(
        &self,
        server: &mut McpServer,
        config: &McpServerConfig,
        www_authenticate: Option<&str>,
        reason: &str,
    ) -> RefreshServerOutcome {
        let name = server.name.clone();
        tracing::warn!(
            server = %name,
            "OAuth refresh rejected ({reason}); discarding token and re-prompting"
        );
        if let Err(e) = self.oauth.store().delete_token(&name).await {
            tracing::warn!(server = %name, "Failed to delete rejected OAuth token: {e}");
        }
        *server.oauth_bearer.write().unwrap() = None;
        match begin_oauth_flow(
            &self.oauth,
            &self.pending_oauth_urls,
            &name,
            config,
            www_authenticate,
            Vec::new(),
            None,
        )
        .await
        {
            Ok(auth_url) => RefreshServerOutcome::Reprompt(format!(
                "MCP server '{name}': authorization expired; re-authorize at {auth_url}"
            )),
            Err(flow_error) => RefreshServerOutcome::Reprompt(format!(
                "MCP server '{name}': OAuth refresh rejected ({reason}) and re-authorization \
                 could not start: {flow_error}"
            )),
        }
    }

    /// `InsufficientScopeStepUp`: read the prior grants, discard the narrow
    /// token, and surface a re-authorization requesting the union of prior
    /// and challenged scopes. The claim moves into the pending flow so the
    /// triggering call stays parked until the re-authorized server is
    /// republished (REQ-MCP-012, deferred `ReAuthCallRetry`).
    async fn step_up_authorization(
        &self,
        mut server: Box<McpServer>,
        www_authenticate: &str,
        claim: ServerClaim,
    ) -> Result<(), String> {
        let name = server.name.clone();
        let config = server.config();
        // Prior grants are read BEFORE the token is discarded; persisting
        // scopes on the token makes them available even across a restart.
        let prior_scopes = match self.oauth.store().token(&name).await {
            Ok(Some(token)) => token.scopes,
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!(server = %name, "OAuth token lookup failed during step-up: {e}");
                Vec::new()
            }
        };
        if let Err(e) = self.oauth.store().delete_token(&name).await {
            tracing::warn!(server = %name, "Failed to delete narrow OAuth token: {e}");
        }
        // Best-effort session teardown while the old bearer is still
        // attached; the replacement connection starts fresh.
        server.terminate().await;

        match begin_oauth_flow(
            &self.oauth,
            &self.pending_oauth_urls,
            &name,
            &config,
            Some(www_authenticate),
            prior_scopes,
            Some(claim),
        )
        .await
        {
            Ok(auth_url) => {
                tracing::info!(
                    server = %name,
                    url = %auth_url,
                    "Tool call needs additional scopes; awaiting re-authorization"
                );
                Ok(())
            }
            Err(e) => Err(format!(
                "MCP server '{name}': insufficient scope and re-authorization could not \
                 start: {e}"
            )),
        }
    }

    /// Register a new connect attempt for `server_name` toward `config`,
    /// superseding any earlier attempt still in flight.
    fn issue_connect_ticket(&self, server_name: &str, config: &McpServerConfig) -> u64 {
        let ticket = next_generation();
        self.connect_tickets
            .lock()
            .unwrap()
            .insert(server_name.to_string(), (ticket, config.clone()));
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
    fn claim_server(&self, server_name: &str) -> ServerClaim {
        let (sender, _) = tokio::sync::watch::channel(());
        self.recovering_map()
            .insert(server_name.to_string(), sender);
        ServerClaim {
            recovering: Arc::clone(&self.recovering),
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
                    let oauth_rt = Arc::clone(&manager.oauth);
                    let ticket = manager.issue_connect_ticket(&name, &entry);
                    tokio::spawn(async move {
                        let result =
                            Self::connect_one(&name, &entry, Arc::clone(&oauth), oauth_rt).await;
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
                                // Clear the failure only once this attempt is
                                // the published one; a superseded success must
                                // not erase a failure the winning attempt
                                // recorded (REQ-MCP-018).
                                mgr.failed_servers.write().await.remove(&name);
                                tracing::info!(
                                    server = %name,
                                    tools = tool_count,
                                    "MCP server connected"
                                );
                                Some((name, tool_count))
                            }
                            Err(e) => {
                                // Only record if this attempt still owns the
                                // ticket; a superseded/removed connect failing
                                // late must not resurrect a `failed` entry for a
                                // server no longer configured (REQ-MCP-018).
                                if clear_ticket_if_current(&mgr.connect_tickets, &name, ticket) {
                                    record_connect_failure(
                                        &mgr.failed_servers,
                                        &mgr.pending_oauth_urls,
                                        &name,
                                        &entry,
                                        e,
                                    )
                                    .await;
                                }
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
        let failed = self.failed_servers.read().await;
        // Surfaced only on `unauthorized` entries: the authorize link those
        // carry is the one a loopback-on-remote redirect breaks (REQ-MCP-020).
        let redirect_warning = self.oauth.redirect_warning();

        // Connected servers are ready.
        let mut result: Vec<McpServerStatus> = servers
            .iter()
            .map(|(name, server)| McpServerStatus {
                name: name.clone(),
                state: McpConnState::Ready,
                transport: server.config.transport_kind(),
                auth: server.config.auth_kind(),
                tool_count: server.tools.len(),
                tools: server.tools.iter().map(|t| t.name.clone()).collect(),
                enabled: !disabled.contains(name),
                pending_oauth_url: None,
                last_error: None,
                auth_redirect_warning: None,
            })
            .collect();

        // Servers blocked on OAuth haven't entered the connected map yet
        // (REQ-MCP-013). The pending map carries only the URL; the awaiting
        // state is OAuth by construction, so the native HTTP transport is
        // assumed (the legacy stdio `mcp-remote` bridge mislabels here).
        for (name, url) in pending.iter() {
            if !servers.contains_key(name) {
                result.push(McpServerStatus {
                    name: name.clone(),
                    state: McpConnState::Unauthorized,
                    transport: McpTransportKind::Http,
                    auth: McpAuthKind::Oauth,
                    tool_count: 0,
                    tools: vec![],
                    enabled: !disabled.contains(name),
                    pending_oauth_url: Some(url.clone()),
                    last_error: None,
                    auth_redirect_warning: redirect_warning.clone(),
                });
            }
        }

        // Failed servers are retained with their cause (REQ-MCP-018). A server
        // that has since reconnected (in `servers`) or is awaiting auth (in
        // `pending`) takes precedence over a stale failure record.
        for (name, failure) in failed.iter() {
            if !servers.contains_key(name) && !pending.contains_key(name) {
                result.push(McpServerStatus {
                    name: name.clone(),
                    state: McpConnState::Failed,
                    transport: failure.transport,
                    auth: failure.auth,
                    tool_count: 0,
                    tools: vec![],
                    enabled: !disabled.contains(name),
                    pending_oauth_url: None,
                    last_error: Some(failure.error.clone()),
                    auth_redirect_warning: None,
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
                                // Retain the dropped server as failed rather
                                // than letting it vanish (REQ-MCP-018).
                                record_connect_failure(
                                    &self.failed_servers,
                                    &self.pending_oauth_urls,
                                    &name,
                                    &server.config,
                                    reestablish_err.to_string(),
                                )
                                .await;
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
    /// the server is held out of the map joins the parked claim instead of
    /// failing with "not connected", and keeps re-joining if a new claim is
    /// parked between a release and the re-lookup (back-to-back recoveries);
    /// absence with no claim parked is settled.
    async fn attempt_call(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<CallAttempt, String> {
        loop {
            {
                let servers = self.servers.read().await;
                if let Some(server) = servers.get(server_name) {
                    return Ok(CallAttempt::run(server, tool_name, arguments).await);
                }
            }
            let receiver = self
                .recovering_map()
                .get(server_name)
                .map(tokio::sync::watch::Sender::subscribe);
            match receiver {
                Some(mut receiver) => {
                    let _ = receiver.changed().await;
                }
                None => return Err(format!("MCP server '{server_name}' is not connected")),
            }
        }
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
    #[allow(clippy::too_many_lines)] // One failure-classification lifecycle: attempt, classify, recover (transport or OAuth), retry.
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
                enum Recovery {
                    Lead {
                        server: Box<McpServer>,
                        action: &'static str,
                        claim: ServerClaim,
                    },
                    /// A 401 on an OAuth-authorized server: silently refresh
                    /// the token, then retry (`TokenRefreshNeeded`).
                    OAuthRefresh {
                        server: Box<McpServer>,
                        claim: ServerClaim,
                        www_authenticate: Option<String>,
                    },
                    /// A 403 `insufficient_scope` on an OAuth-authorized
                    /// server: re-authorize with the scope union, holding the
                    /// claim so this call replays once the operator acts
                    /// (`InsufficientScopeStepUp`).
                    OAuthStepUp {
                        server: Box<McpServer>,
                        claim: ServerClaim,
                        www_authenticate: String,
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
                        // side-effecting call -- unless it is an OAuth
                        // re-auth condition, which claims the slot like a
                        // transport recovery (REQ-MCP-012).
                        if server.generation == attempt.generation
                            && server.is_alive()
                            && !attempt.recoverable
                        {
                            match oauth_recovery_kind(&server, &e) {
                                Some(OAuthRecoveryKind::Refresh { www_authenticate }) => {
                                    Recovery::OAuthRefresh {
                                        server: Box::new(server),
                                        claim: self.claim_server(server_name),
                                        www_authenticate,
                                    }
                                }
                                Some(OAuthRecoveryKind::StepUp { www_authenticate }) => {
                                    Recovery::OAuthStepUp {
                                        server: Box::new(server),
                                        claim: self.claim_server(server_name),
                                        www_authenticate,
                                    }
                                }
                                None => {
                                    servers.insert(server_name.to_string(), server);
                                    return Err(e.into_message(server_name));
                                }
                            }
                        } else if server.generation == attempt.generation {
                            // A recoverable transport failure from the
                            // serving instance: lead the recovery.
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
                            // A failure from a transport that has already
                            // been replaced (another task finished a
                            // recovery, or reload swapped the server) is
                            // stale: the fresh instance's policy must not
                            // re-judge it. Surface it if the serving instance
                            // deemed it non-recoverable; otherwise retry on
                            // the fresh instance instead of tearing it down.
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
                        // Captured before a successful reestablish moves the
                        // server back into the map, so the failure path can
                        // still retain its transport/auth (REQ-MCP-018).
                        let config = server.config.clone();
                        let result = server.reestablish().await;
                        if result.is_ok() {
                            insert_server(&self.servers, server_name, *server).await;
                        }
                        drop(claim);
                        if let Err(reestablish_err) = result {
                            record_connect_failure(
                                &self.failed_servers,
                                &self.pending_oauth_urls,
                                server_name,
                                &config,
                                reestablish_err.to_string(),
                            )
                            .await;
                            return Err(format!(
                                "MCP server '{server_name}' connection lost and {action} failed: {reestablish_err}"
                            ));
                        }
                    }
                    // Silent token refresh (REQ-MCP-012): on success the
                    // server rejoins the map with the rotated bearer and the
                    // call retries below, so a routine expiry never surfaces
                    // as a tool failure.
                    Recovery::OAuthRefresh {
                        mut server,
                        claim,
                        www_authenticate,
                    } => {
                        match self
                            .refresh_authorized_server(&mut server, www_authenticate.as_deref())
                            .await
                        {
                            RefreshServerOutcome::Refreshed => {
                                insert_server(&self.servers, server_name, *server).await;
                                drop(claim);
                            }
                            // Not evidence of a stale token (network blip to
                            // the authorization server): keep the server and
                            // its token; the next 401 retries the refresh.
                            RefreshServerOutcome::Transient(message) => {
                                insert_server(&self.servers, server_name, *server).await;
                                drop(claim);
                                return Err(message);
                            }
                            // TokenRefreshFailed: the token was discarded and
                            // a fresh authorization flow surfaced its URL;
                            // the server leaves the map until the operator
                            // completes it.
                            RefreshServerOutcome::Reprompt(message) => {
                                server.terminate().await;
                                drop(claim);
                                return Err(message);
                            }
                        }
                    }
                    // Scope step-up (REQ-MCP-012): the claim moves into the
                    // pending flow, so the retry below parks until the
                    // operator re-authorizes and the server is republished --
                    // then the call replays with the upgraded token.
                    Recovery::OAuthStepUp {
                        server,
                        claim,
                        www_authenticate,
                    } => {
                        self.step_up_authorization(server, &www_authenticate, claim)
                            .await?;
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

        // Retry once via the same claim-joining lookup. The "not connected"
        // error here means a followed (or this call's own) recovery failed
        // and dropped the server.
        let retry = self
            .attempt_call(server_name, tool_name, &arguments)
            .await?;
        retry.result.map_err(|e| e.into_message(server_name))
    }

    /// Connect and initialize a single MCP server. A failed handshake shuts
    /// the transport down (ending any session `initialize` created) before
    /// the error is returned.
    ///
    /// For OAuth-eligible HTTP servers this is also where the token
    /// lifecycle's connect-side rules live: a stored, resource-matched token
    /// is restored onto the very first `initialize` (REQ-MCP-012,
    /// `ServerDiscoveredWithStoredToken`); a 401 against a refreshable token
    /// takes the silent refresh path; and a 401 with no usable token starts
    /// the authorization flow, failing the connect with the surfaced URL
    /// (`OAuthRequired`). A 401 against static config credentials stays a hard
    /// failure (`StaticAuthRejected`, REQ-MCP-008).
    #[allow(clippy::too_many_lines)] // One ordered connect sequence: seed, restore, handshake, refresh-or-prompt.
    async fn connect_one(
        name: &str,
        entry: &McpServerConfig,
        pending_oauth_urls: Arc<RwLock<HashMap<String, String>>>,
        oauth_rt: Arc<OAuthRuntime>,
    ) -> Result<McpServer, String> {
        // A pre-configured client (Claude Code's `oauth` shape) is seeded only
        // once discovery resolves the authorization server's issuer, since the
        // config does not name it — see `acquire_client_registration`.
        let bearer: SharedBearer = Arc::default();
        if oauth_resource_url(entry).is_none() {
            // The config no longer selects OAuth (static credentials or
            // stdio): a token left from an OAuth-era config — e.g. the auth
            // mode changed while Phoenix was stopped, so no reload rule saw
            // the transition — must not linger to be restored if the config
            // later flips back (ReloadInvalidatesOAuth's invariant, applied
            // at connect time).
            match oauth_rt.store().token(name).await {
                Ok(Some(_)) => {
                    tracing::info!(
                        server = %name,
                        "Config no longer selects OAuth; discarding the stored token"
                    );
                    if let Err(e) = oauth_rt.store().delete_token(name).await {
                        tracing::warn!(server = %name, "Failed to delete stale OAuth token: {e}");
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(server = %name, "OAuth token lookup failed: {e}"),
            }
        }
        if let Some(url) = oauth_resource_url(entry) {
            match oauth_rt.store().token(name).await {
                Ok(Some(token)) => {
                    let resource = oauth::canonical_resource(url);
                    if token.resource != resource {
                        // The config was repointed; the token's audience is
                        // the old endpoint and must not be sent to the new
                        // one (REQ-MCP-012).
                        tracing::info!(
                            server = %name,
                            "Stored OAuth token is bound to a different resource; discarding"
                        );
                        let _ = oauth_rt.store().delete_token(name).await;
                    } else if token.is_expired() && token.refresh_token.is_none() {
                        tracing::info!(
                            server = %name,
                            "Stored OAuth token is expired with no refresh token; discarding"
                        );
                        let _ = oauth_rt.store().delete_token(name).await;
                    } else {
                        // Silent restore: the bearer rides the first
                        // initialize. An expired-but-refreshable token rides
                        // the 401 → refresh path below, still with no
                        // re-prompt (REQ-MCP-012).
                        *bearer.write().unwrap() = Some(token.access_token.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(server = %name, "OAuth token lookup failed: {e}"),
            }
        }

        let mut server = McpServer::connect(
            name,
            entry.clone(),
            Arc::clone(&pending_oauth_urls),
            Arc::clone(&bearer),
        )
        .await?;
        let Err(failure) = server.handshake().await else {
            return Ok(server);
        };
        let HandshakeFailure::Unauthorized {
            www_authenticate,
            message,
        } = failure
        else {
            return Err(failure.to_string());
        };
        // StaticAuthRejected: there is no interactive flow to recover a
        // rejected config credential into (REQ-MCP-008). Stdio cannot 401.
        let Some(url) = oauth_resource_url(entry) else {
            return Err(message);
        };

        // Silent refresh before any re-prompt: a restored token whose access
        // half expired offline refreshes on this first 401 (REQ-MCP-012).
        let stored = oauth_rt.store().token(name).await.unwrap_or_default();
        if let Some(token) = stored {
            if token.refresh_token.is_some() {
                match oauth_refresh(&oauth_rt, name, url, www_authenticate.as_deref(), &token).await
                {
                    Ok(access_token) => {
                        *bearer.write().unwrap() = Some(access_token);
                        match server.reestablish().await {
                            Ok(()) => return Ok(server),
                            Err(HandshakeFailure::Unauthorized { .. }) => {
                                // The freshly refreshed token was still
                                // rejected; the grant chain is dead.
                                let _ = oauth_rt.store().delete_token(name).await;
                            }
                            Err(other) => return Err(other.to_string()),
                        }
                    }
                    Err(RefreshFailure::Transient(e)) => {
                        return Err(format!(
                            "MCP server '{name}': OAuth token refresh failed: {e}"
                        ));
                    }
                    Err(RefreshFailure::Rejected(e)) => {
                        tracing::warn!(
                            server = %name,
                            "OAuth refresh rejected ({e}); discarding token and re-prompting"
                        );
                        let _ = oauth_rt.store().delete_token(name).await;
                    }
                }
            } else {
                // An unexpired stored token was rejected and cannot be
                // refreshed: discard it before re-prompting so a stale
                // credential never coexists with the fresh one.
                let _ = oauth_rt.store().delete_token(name).await;
            }
        }

        // Fresh authorization: surface the URL and fail the connect — the
        // callback completing the flow reconnects (REQ-MCP-009..011).
        let auth_url = begin_oauth_flow(
            &oauth_rt,
            &pending_oauth_urls,
            name,
            entry,
            www_authenticate.as_deref(),
            Vec::new(),
            None,
        )
        .await
        .map_err(|e| format!("MCP server '{name}': OAuth authorization failed: {e}"))?;
        Err(format!(
            "MCP server '{name}' requires OAuth authorization; open {auth_url}"
        ))
    }

    /// Read all MCP config files in priority order, merging by server name
    /// (first-seen wins).
    fn read_all_configs() -> Vec<(String, McpServerConfig)> {
        let home = phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()
            .home()
            .to_path_buf();
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
            // An explicit static credential under `auth` wins; otherwise the
            // top-level `oauth` object (Claude Code's shape) selects OAuth;
            // otherwise no credential, and a 401 still drives OAuth discovery.
            let auth = match cfg.get("auth") {
                Some(auth) => Self::classify_http_auth(name, auth)?,
                None => match cfg.get("oauth") {
                    Some(oauth) => Self::classify_oauth_auth(name, oauth)?,
                    None => HttpAuth::None,
                },
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
    /// `{"headers": {...}}` is an explicit static credential (REQ-MCP-008). An
    /// unrecognized or malformed shape skips the server -- silently
    /// downgrading an intended credential to no-auth would change which
    /// authorization path a 401 takes. OAuth is declared by the sibling
    /// top-level `oauth` field (Claude Code's shape), not under `auth`.
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
        if auth.get("oauth").is_some() {
            tracing::warn!(
                server = %name,
                "'auth.oauth' is no longer read; move the OAuth config to the top-level \
                 'oauth' field (Claude Code's shape). Skipping this server until then."
            );
            return None;
        }
        tracing::debug!(server = %name, "HTTP MCP server with unrecognized 'auth' shape");
        None
    }

    /// Classify the top-level `oauth` value (Claude Code's shape): `true` or
    /// an object without a `clientId` selects OAuth with a dynamically
    /// registered client; an object carrying `clientId` pre-configures the
    /// (public) client identity for an authorization server that disables DCR
    /// (REQ-MCP-010). The authorization server is *not* named in config — it
    /// is discovered from the resource's metadata. `callbackPort` is read but
    /// ignored: Phoenix receives the redirect on its own server route, not a
    /// throwaway localhost port. No client secret is read: the flow is
    /// authorization-code + PKCE, a public client.
    fn classify_oauth_auth(name: &str, oauth_value: &Value) -> Option<HttpAuth> {
        match oauth_value {
            Value::Bool(true) => Some(HttpAuth::OAuth(None)),
            Value::Object(fields) => match fields.get("clientId").and_then(Value::as_str) {
                Some(client_id) => {
                    // A present-but-malformed callbackPort (non-integer, 0, or
                    // out of range) would silently fall back to the server-route
                    // redirect, which a fixed-allowlist app rejects — a confusing
                    // failure. Treat it as an unusable config and skip the server.
                    let callback_port = match fields.get("callbackPort") {
                        None => None,
                        Some(value) => match value.as_u64().and_then(|p| u16::try_from(p).ok()) {
                            Some(port) if port != 0 => Some(port),
                            _ => {
                                tracing::debug!(
                                    server = %name,
                                    "'oauth.callbackPort' must be an integer 1-65535; skipping server"
                                );
                                return None;
                            }
                        },
                    };
                    Some(HttpAuth::OAuth(Some(PreconfiguredClient {
                        client_id: client_id.to_string(),
                        callback_port,
                    })))
                }
                // An object with no clientId (e.g. only callbackPort/scopes):
                // OAuth via dynamic client registration, where DCR registers
                // Phoenix's own callback, so a fixed loopback port is moot.
                None => Some(HttpAuth::OAuth(None)),
            },
            Value::Null
            | Value::Bool(false)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_) => {
                tracing::debug!(server = %name, "'oauth' must be true or an object");
                None
            }
        }
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
        // One reconciliation at a time (see `reload_serial`). Recovery and
        // refresh holds are not serialized by this -- they are settled via
        // claims below -- only sibling reloads are.
        let _serial = self.reload_serial.lock().await;

        let config_names: std::collections::HashSet<String> =
            configs.iter().map(|(n, _)| n.clone()).collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut restarted = Vec::new();
        let mut unchanged = Vec::new();
        let mut failed = Vec::new();
        let mut restart_pending = std::collections::HashSet::new();
        // The target config per in-flight restart, so a restart that times out
        // before its background connect returns is still retained as a failure
        // with the right transport/auth (REQ-MCP-018).
        let mut restart_configs: HashMap<String, McpServerConfig> = HashMap::new();
        let mut restart_futures: futures::stream::FuturesUnordered<McpRestartFuture> =
            futures::stream::FuturesUnordered::new();

        // Removal sweep. A held server (claim parked, entry absent) whose
        // name left the config must also be removed -- the holder would
        // otherwise reinsert it later as a zombie -- so claimed names are
        // settled and swept alongside the map keys.
        let mut removed_servers = Vec::new();
        let claimed: Vec<String> = {
            let mut servers = self.servers.write().await;
            // Revoke connect tickets for names no longer configured while
            // still holding the servers lock: publish_if_current checks the
            // ticket under this same lock, so an in-flight connect either
            // publishes before this sweep (and is removed by it, below) or
            // observes the revocation -- there is no window in which a late
            // publish can land after the sweep and resurrect a removed
            // server.
            self.connect_tickets
                .lock()
                .unwrap()
                .retain(|name, _| config_names.contains(name));

            let existing_names: Vec<String> = servers.keys().cloned().collect();
            for name in existing_names {
                if !config_names.contains(&name) {
                    if let Some(server) = servers.remove(&name) {
                        removed_servers.push((name.clone(), server));
                    }
                    removed.push(name);
                }
            }

            // Snapshot active holds under the same lock as the sweep. A
            // holder releases its claim only after reinserting through this
            // lock, so every held server is either already back in the map
            // (swept above) or still claimed (in this snapshot) -- none can
            // slip between the sweep and the snapshot.
            let claimed: Vec<String> = self.recovering_map().keys().cloned().collect();
            claimed
        };
        // Sweep pending OAuth flows whose server left the config -- BEFORE
        // settling claims, because a step-up flow holds its server's claim
        // until the operator acts, and waiting on it here would block the
        // reload on a browser round trip. Cancelling drops the flow (and its
        // claim), so the stale callback is rejected (ReloadCancelsPendingAuth).
        let pending_flow_names: Vec<String> =
            self.oauth.pending.lock().unwrap().keys().cloned().collect();
        for name in pending_flow_names {
            if !config_names.contains(&name) {
                self.cancel_pending_oauth_flow(&name).await;
                if !removed.contains(&name) {
                    removed.push(name);
                }
            }
        }
        for name in claimed {
            if !config_names.contains(&name) && !removed.contains(&name) {
                self.await_claim_release(&name).await;
                if let Some(server) = self.servers.write().await.remove(&name) {
                    removed_servers.push((name.clone(), server));
                    removed.push(name);
                }
            }
        }
        // A server whose only remaining state is a failure record (it never
        // connected, so it is absent from the connected/claimed/pending sets
        // above) is folded into `removed` when dropped from config, so its
        // orphaned OAuth token is deleted and the removal is reported -- not
        // merely swept from status (REQ-MCP-018).
        let failed_only_removed: Vec<String> = {
            let failed = self.failed_servers.read().await;
            failed
                .keys()
                .filter(|name| !config_names.contains(*name) && !removed.contains(*name))
                .cloned()
                .collect()
        };
        removed.extend(failed_only_removed);
        // A removed server's stored token would orphan with no owning server
        // (ReloadRemovesServer / TokenImpliesOAuthServer); the shared
        // per-authorization-server registration is deliberately retained.
        for name in &removed {
            if let Err(e) = self.oauth.store().delete_token(name).await {
                tracing::warn!(server = %name, "Failed to delete OAuth token for removed server: {e}");
            }
        }
        // Drop failure records for any server no longer configured (REQ-MCP-018).
        self.failed_servers
            .write()
            .await
            .retain(|name, _| config_names.contains(name));
        // Sweep pending-auth URLs the same way: a removed server may carry one
        // without an active flow (a failed-only server, or the stdio
        // `mcp-remote` stderr drain writing a URL), which `cancel_pending_oauth_flow`
        // and the connected-server cleanup below would both miss, leaving an
        // `unauthorized` entry for a server no longer configured.
        self.pending_oauth_urls
            .write()
            .await
            .retain(|name, _| config_names.contains(name));
        for (name, mut server) in removed_servers {
            self.pending_oauth_urls.write().await.remove(&name);
            server.terminate().await;
            tracing::info!(server = %name, "MCP server removed during reload");
        }

        for (name, entry) in configs {
            let existing_config = self.settled_config(&name).await;

            match existing_config {
                None => {
                    // A pending native OAuth flow for this exact config keeps
                    // waiting on the operator: superseding it would rotate
                    // the nonce and invalidate the URL they may already have
                    // open in a browser. Only a *changed* config cancels a
                    // pending flow (ReloadCancelsPendingAuth).
                    let pending_same_config = self
                        .oauth
                        .pending
                        .lock()
                        .unwrap()
                        .get(&name)
                        .is_some_and(|flow| flow.config == entry);
                    if pending_same_config {
                        unchanged.push(name);
                        continue;
                    }
                    if let Some(old_flow_config) = self.cancel_pending_oauth_flow(&name).await {
                        self.invalidate_oauth_on_config_change(&name, &old_flow_config, &entry)
                            .await;
                    }

                    let oauth = Arc::clone(&self.pending_oauth_urls);
                    oauth.write().await.remove(&name);
                    added.push(name.clone());

                    // An earlier attempt may still be handshaking toward this
                    // exact config (added servers park no claim to settle
                    // on). Superseding it would gamble both attempts -- the
                    // old one discarded as stale, the new one possibly
                    // failing -- so a pending same-config attempt is left to
                    // finish instead.
                    let same_config_in_flight = self
                        .connect_tickets
                        .lock()
                        .unwrap()
                        .get(&name)
                        .is_some_and(|(_, pending)| *pending == entry);
                    if same_config_in_flight {
                        tracing::debug!(
                            server = %name,
                            "Connect already in flight for this config; leaving it to finish"
                        );
                        continue;
                    }

                    let servers = Arc::clone(&self.servers);
                    let tickets = Arc::clone(&self.connect_tickets);
                    let oauth_rt = Arc::clone(&self.oauth);
                    let failed = Arc::clone(&self.failed_servers);
                    // Supersede any in-flight connect for this name BEFORE
                    // acting on the observed absence: with the new ticket
                    // issued, a stale publish landing from here on is
                    // discarded by the ticket check. One landing earlier --
                    // between the absence observation and this issue -- is
                    // evicted below, so an old attempt's server can neither
                    // race the new connect nor outlive a failed one.
                    let ticket = self.issue_connect_ticket(&name, &entry);
                    if let Some(mut stale) = self.servers.write().await.remove(&name) {
                        tracing::warn!(
                            server = %name,
                            "Evicting a stale connect that landed before reload superseded it"
                        );
                        stale.terminate().await;
                    }
                    tokio::spawn(async move {
                        let result =
                            Self::connect_one(&name, &entry, Arc::clone(&oauth), oauth_rt).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
                                let tool_count = server.tools.len();
                                if publish_if_current(&servers, &tickets, &name, ticket, server)
                                    .await
                                {
                                    // Clear only once published, so a superseded
                                    // success cannot erase the winning attempt's
                                    // failure (REQ-MCP-018).
                                    failed.write().await.remove(&name);
                                    tracing::info!(
                                        server = %name,
                                        tools = tool_count,
                                        "MCP server connected during reload"
                                    );
                                }
                            }
                            Err(e) => {
                                if clear_ticket_if_current(&tickets, &name, ticket) {
                                    record_connect_failure(&failed, &oauth, &name, &entry, e).await;
                                }
                            }
                        }
                    });
                }
                Some(current) if current == entry => {
                    unchanged.push(name);
                }
                Some(old_config) => {
                    // A changed config cancels any pending authorization
                    // (ReloadCancelsPendingAuth) -- BEFORE the slot loop
                    // below, which would otherwise wait on a claim the
                    // pending flow holds until the operator acts -- and
                    // discards a stored token the new config can no longer
                    // use (ReloadInvalidatesOAuth).
                    self.cancel_pending_oauth_flow(&name).await;
                    self.invalidate_oauth_on_config_change(&name, &old_config, &entry)
                        .await;

                    // Supersede any in-flight connect BEFORE removing the old
                    // server: with the new ticket issued, a stale publish can
                    // no longer slip into the window between the removal and
                    // the new connect (the removal below sweeps anything that
                    // landed earlier).
                    let ticket = self.issue_connect_ticket(&name, &entry);

                    // Take the slot. The old-config server may be momentarily
                    // held out by a refresh/recovery claim; settle the hold
                    // and re-take rather than misreading it as nothing-to-
                    // restart -- the holder would reinsert the OLD config and
                    // this reload would silently fail to apply the new one.
                    let slot = loop {
                        {
                            let mut servers = self.servers.write().await;
                            match servers.get(&name) {
                                Some(server) if server.config() == entry => break Slot::Desired,
                                Some(_) => match servers.remove(&name) {
                                    Some(server) => break Slot::Old(Box::new(server)),
                                    None => break Slot::Vacant,
                                },
                                None => {}
                            }
                        }
                        let receiver = self
                            .recovering_map()
                            .get(&name)
                            .map(tokio::sync::watch::Sender::subscribe);
                        match receiver {
                            Some(mut receiver) => {
                                let _ = receiver.changed().await;
                            }
                            // Absent with no hold: a recovery dropped the
                            // server in the gap. The new connect below fills
                            // the vacancy with the desired config.
                            None => break Slot::Vacant,
                        }
                    };

                    match slot {
                        Slot::Desired => {
                            unchanged.push(name);
                            continue;
                        }
                        Slot::Old(mut old_server) => {
                            self.pending_oauth_urls.write().await.remove(&name);
                            old_server.terminate().await;
                        }
                        Slot::Vacant => {
                            self.pending_oauth_urls.write().await.remove(&name);
                        }
                    }

                    let oauth = Arc::clone(&self.pending_oauth_urls);
                    let servers = Arc::clone(&self.servers);
                    let tickets = Arc::clone(&self.connect_tickets);
                    let oauth_rt = Arc::clone(&self.oauth);
                    let failed = Arc::clone(&self.failed_servers);
                    restart_pending.insert(name.clone());
                    restart_configs.insert(name.clone(), entry.clone());
                    // The connect runs as a detached task: when the reload
                    // deadline drops the awaiting future below, the task is
                    // abandoned, not cancelled, so a partially established
                    // connection still finishes -- publishing (late, ticket
                    // permitting) on success, or terminating the transport on
                    // a handshake failure so a created HTTP session is
                    // DELETEd rather than leaked by a cancelled future.
                    let task_name = name.clone();
                    let task = tokio::spawn(async move {
                        let result =
                            Self::connect_one(&name, &entry, Arc::clone(&oauth), oauth_rt).await;
                        match result {
                            Ok(server) => {
                                oauth.write().await.remove(&name);
                                let tool_count = server.tools.len();
                                if publish_if_current(&servers, &tickets, &name, ticket, server)
                                    .await
                                {
                                    // Clear only once published, so a superseded
                                    // success cannot erase the winning attempt's
                                    // failure (REQ-MCP-018).
                                    failed.write().await.remove(&name);
                                    (name, Ok(tool_count))
                                } else {
                                    // Nothing was published; reporting this
                                    // as restarted would describe a server
                                    // that does not exist.
                                    (name, Err("superseded by a newer reload".to_string()))
                                }
                            }
                            Err(error) => {
                                if clear_ticket_if_current(&tickets, &name, ticket) {
                                    record_connect_failure(
                                        &failed,
                                        &oauth,
                                        &name,
                                        &entry,
                                        error.clone(),
                                    )
                                    .await;
                                }
                                (name, Err(error))
                            }
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
                        let error = format!(
                            "timed out after {}s restarting changed MCP server",
                            RELOAD_RESTART_TIMEOUT.as_secs()
                        );
                        // Retain the timed-out restart as failed so status shows
                        // it instead of an empty gap while the background connect
                        // runs on (REQ-MCP-018). The still-current background task
                        // reconciles this: it clears on a late publish, or
                        // overwrites with the real error on a late failure.
                        if let Some(config) = restart_configs.get(&name) {
                            record_connect_failure(
                                &self.failed_servers,
                                &self.pending_oauth_urls,
                                &name,
                                config,
                                error.clone(),
                            )
                            .await;
                        }
                        failed.push(McpReloadFailure {
                            server: name,
                            action: "restart".to_string(),
                            error,
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
/// (re-establishing after a transport failure, refreshing its tool list, or
/// awaiting an OAuth step-up re-authorization). Dropping it releases the
/// claim and wakes waiters on every exit path -- success, error, panic
/// unwind, or a dropped future -- so a dead holder can never strand the
/// callers waiting on it. Owns its handle on the claim map (rather than
/// borrowing the manager) so a pending OAuth flow can hold it.
struct ServerClaim {
    recovering: RecoveringMap,
    name: String,
}

impl Drop for ServerClaim {
    fn drop(&mut self) {
        self.recovering.lock().unwrap().remove(&self.name);
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

impl McpServerConfig {
    fn transport_kind(&self) -> McpTransportKind {
        match self {
            Self::Stdio { .. } => McpTransportKind::Stdio,
            Self::Http { .. } => McpTransportKind::Http,
        }
    }

    /// The declared auth scheme. Stdio has none; an HTTP server reflects its
    /// `auth` field (a `none` HTTP server may still reach OAuth via a 401).
    fn auth_kind(&self) -> McpAuthKind {
        match self {
            Self::Stdio { .. }
            | Self::Http {
                auth: HttpAuth::None,
                ..
            } => McpAuthKind::None,
            Self::Http {
                auth: HttpAuth::Static(_),
                ..
            } => McpAuthKind::Static,
            Self::Http {
                auth: HttpAuth::OAuth(_),
                ..
            } => McpAuthKind::Oauth,
        }
    }
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

/// A pre-configured public OAuth client for an authorization server that
/// disables dynamic client registration (Claude Code's `oauth` config shape).
/// The identity, not a credential: once discovery resolves the authorization
/// server's issuer, this seeds the persisted registration under that issuer so
/// the flow reuses it instead of attempting DCR. It does not pre-authorize the
/// server (REQ-MCP-010). The authorization server is *not* named here — it is
/// learned from the resource's advertised metadata at discovery time.
///
/// Public client only: the flow is OAuth 2.1 authorization-code + PKCE, which
/// needs no client secret. Phoenix neither accepts nor stores a pre-configured
/// client secret — keeping a long-lived app credential out of the app is the
/// point. A server that mandates confidential client authentication is out of
/// scope here.
///
/// `callback_port` (Claude Code's `oauth.callbackPort`) is set when the
/// pre-registered app's redirect allowlist only contains a fixed
/// `http://localhost:<port>/callback` loopback URI — it cannot register
/// Phoenix's own server-route callback. Phoenix then uses that loopback
/// redirect and bounces the browser to its real callback via an ephemeral
/// listener on that port (REQ-MCP-020). Absent, Phoenix uses its server route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreconfiguredClient {
    pub client_id: String,
    pub callback_port: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn classify_entry_parses_oauth_shapes() {
        // Bare OAuth (Claude Code's shape): client identity acquired
        // dynamically. An object without `clientId` (here, only callbackPort)
        // is still dynamic — the extra fields are Claude-Code-only.
        for oauth_value in [
            serde_json::json!(true),
            serde_json::json!({}),
            serde_json::json!({"callbackPort": 3118}),
        ] {
            let cfg = serde_json::json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "oauth": oauth_value,
            });
            assert_eq!(
                McpClientManager::classify_config_entry("s", &cfg),
                Some(McpServerConfig::Http {
                    url: "https://example.com/mcp".to_string(),
                    headers: HashMap::new(),
                    auth: HttpAuth::OAuth(None),
                }),
                "oauth = {oauth_value} must select dynamic-client OAuth"
            );
        }

        // Pre-configured client for a DCR-less authorization server: `clientId`
        // names the pre-registered app; the authorization server and the
        // client secret are not in config (discovered / from the environment).
        let preconfigured = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "oauth": {"clientId": "cid-1", "callbackPort": 3118},
        });
        assert_eq!(
            McpClientManager::classify_config_entry("slack", &preconfigured),
            Some(McpServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::OAuth(Some(PreconfiguredClient {
                    client_id: "cid-1".to_string(),
                    callback_port: Some(3118),
                })),
            })
        );

        // An explicit static `auth` credential wins over a sibling `oauth`.
        let static_wins = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "auth": {"bearer": "tok"},
            "oauth": {"clientId": "cid-1"},
        });
        assert_eq!(
            McpClientManager::classify_config_entry("s", &static_wins),
            Some(McpServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::Static(StaticCred::Bearer("tok".to_string())),
            })
        );
    }

    #[test]
    fn classify_oauth_rejects_malformed_callback_port() {
        // A present-but-unusable callbackPort skips the server rather than
        // silently falling back to a redirect the fixed-allowlist app rejects.
        for bad in [
            serde_json::json!("3118"),
            serde_json::json!(0),
            serde_json::json!(70000),
            serde_json::json!(-1),
            serde_json::json!(3.5),
        ] {
            let cfg = serde_json::json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "oauth": {"clientId": "cid", "callbackPort": bad},
            });
            assert_eq!(
                McpClientManager::classify_config_entry("s", &cfg),
                None,
                "callbackPort {bad} must skip the server"
            );
        }
    }

    #[test]
    fn classify_legacy_auth_oauth_shape_is_skipped() {
        // The removed `auth.oauth` shape is no longer read; the server is
        // skipped (with a migration warning) rather than silently downgraded
        // to no-auth. OAuth now lives under the top-level `oauth` field.
        let cfg = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "auth": {"oauth": true},
        });
        assert_eq!(McpClientManager::classify_config_entry("s", &cfg), None);
    }

    #[test]
    fn callback_request_query_extracts_query_verbatim() {
        assert_eq!(
            callback_request_query("GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n"),
            "?code=abc&state=xyz"
        );
        // An error callback's query is forwarded just the same.
        assert_eq!(
            callback_request_query("GET /callback?error=access_denied&state=xyz HTTP/1.1"),
            "?error=access_denied&state=xyz"
        );
        // No query → empty (the real callback then reports a missing code).
        assert_eq!(callback_request_query("GET /callback HTTP/1.1"), "");
        assert_eq!(callback_request_query(""), "");
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
            tools_changed: Arc::new(AtomicBool::new(false)),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
            oauth_bearer: Arc::default(),
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

    fn http_none_config(url: &str) -> McpServerConfig {
        McpServerConfig::Http {
            url: url.to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
        }
    }

    #[tokio::test]
    async fn status_retains_failed_server_with_cause_and_clears_on_reconnect() {
        let manager = McpClientManager::new();
        let config = http_none_config("https://remote.example/mcp");
        record_connect_failure(
            &manager.failed_servers,
            &manager.pending_oauth_urls,
            "remote",
            &config,
            "connection refused".to_string(),
        )
        .await;

        let status = manager.status().await;
        assert_eq!(status.len(), 1, "a failed server is retained, not dropped");
        let s = &status[0];
        assert_eq!(s.name, "remote");
        assert!(matches!(s.state, McpConnState::Failed));
        assert!(matches!(s.transport, McpTransportKind::Http));
        assert_eq!(s.last_error.as_deref(), Some("connection refused"));

        // A successful reconnect clears the failure (mirrored by the connect
        // Ok arms); the server then vanishes from the failed set.
        manager.failed_servers.write().await.remove("remote");
        assert!(manager.status().await.is_empty());
    }

    #[tokio::test]
    async fn reload_sweeps_failed_only_servers_dropped_from_config() {
        let manager = McpClientManager::new();
        manager.failed_servers.write().await.insert(
            "gone".to_string(),
            FailureRecord::from_config(
                &http_none_config("https://gone.example/mcp"),
                "boom".into(),
            ),
        );
        // A failed-only server (never connected, so absent from the connected
        // map) dropped from config must be swept, not linger in status.
        manager.reload_from_configs(vec![]).await;
        assert!(manager.failed_servers.read().await.is_empty());
        assert!(manager.status().await.is_empty());
    }

    #[tokio::test]
    async fn failed_entry_reflects_disabled_state() {
        let manager = McpClientManager::new();
        manager.failed_servers.write().await.insert(
            "remote".to_string(),
            FailureRecord::from_config(&http_none_config("https://remote.example/mcp"), "x".into()),
        );
        manager
            .disabled_servers
            .write()
            .await
            .insert("remote".to_string());

        let status = manager.status().await;
        assert_eq!(status.len(), 1);
        assert!(matches!(status[0].state, McpConnState::Failed));
        assert!(
            !status[0].enabled,
            "a disabled failed server reports disabled"
        );
    }

    #[tokio::test]
    async fn unauthorized_entry_carries_the_redirect_warning() {
        let manager = McpClientManager::new();
        manager.set_oauth_redirect_warning(Some("callback unreachable".to_string()));
        manager.pending_oauth_urls.write().await.insert(
            "remote".to_string(),
            "https://auth.example/authorize".to_string(),
        );

        let status = manager.status().await;
        assert_eq!(status.len(), 1);
        assert!(matches!(status[0].state, McpConnState::Unauthorized));
        assert_eq!(
            status[0].auth_redirect_warning.as_deref(),
            Some("callback unreachable"),
            "the redirect diagnostic rides the unauthorized entry (REQ-MCP-020)"
        );
    }

    #[tokio::test]
    async fn awaiting_authorization_is_unauthorized_not_failed() {
        let manager = McpClientManager::new();
        manager.pending_oauth_urls.write().await.insert(
            "remote".to_string(),
            "https://auth.example/authorize".to_string(),
        );

        // A connect that returned Err while the OAuth URL is pending must not
        // be recorded as failed -- it is awaiting the operator.
        record_connect_failure(
            &manager.failed_servers,
            &manager.pending_oauth_urls,
            "remote",
            &http_none_config("https://remote.example/mcp"),
            "HTTP 401".to_string(),
        )
        .await;
        assert!(
            manager.failed_servers.read().await.is_empty(),
            "an awaiting-auth server is not a failure"
        );

        let status = manager.status().await;
        assert_eq!(status.len(), 1);
        assert!(matches!(status[0].state, McpConnState::Unauthorized));
        assert_eq!(
            status[0].pending_oauth_url.as_deref(),
            Some("https://auth.example/authorize")
        );
        assert!(status[0].last_error.is_none());
    }

    #[tokio::test]
    async fn pending_authorization_takes_precedence_over_a_stale_failure() {
        // A server can hold a stale failure and then enter an OAuth flow on
        // retry; the status shows it as unauthorized, not failed.
        let manager = McpClientManager::new();
        manager.failed_servers.write().await.insert(
            "remote".to_string(),
            FailureRecord::from_config(
                &http_none_config("https://remote.example/mcp"),
                "earlier failure".to_string(),
            ),
        );
        manager.pending_oauth_urls.write().await.insert(
            "remote".to_string(),
            "https://auth.example/authorize".to_string(),
        );

        let status = manager.status().await;
        assert_eq!(status.len(), 1, "no duplicate entry across the two maps");
        assert!(matches!(status[0].state, McpConnState::Unauthorized));
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
            Arc::clone(&manager.oauth),
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
        // The failed restart is retained with its cause rather than vanishing
        // (REQ-MCP-018).
        let status = manager.status().await;
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].name, "fixture");
        assert!(matches!(status[0].state, McpConnState::Failed));
        assert!(status[0].last_error.is_some());
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
