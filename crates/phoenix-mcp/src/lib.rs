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
mod supervisor;

pub use http::HttpTransport;
pub use stdio::StdioTransport;

use async_trait::async_trait;
use oauth::{OAuthRegistrationRecord, OAuthStore, OAuthTokenRecord};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use supervisor::{CallOutcome, CallRecovery, RecoveryClaim, SupervisorHandle, SupervisorState};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// The OAuth bearer for one HTTP server, shared between the manager (which
/// seeds it from the token store and rotates it on refresh) and the server's
/// transports (which attach it to every request, REQ-MCP-012). `None` until a
/// token exists.
pub type SharedBearer = Arc<std::sync::RwLock<Option<String>>>;

/// Timeout for a single MCP tool call request-response round trip.
const DEFAULT_TOOL_CALL_TIMEOUT: Duration = Duration::from_mins(5);

/// Longer timeout for initialize + tools/list during server connection.
/// Five minutes gives OAuth flows (mcp-remote prompts, browser redirect) time to complete.
const CONNECT_TIMEOUT: Duration = Duration::from_mins(5);

/// Upper bound for a reload request waiting for a changed server to reconnect.
const RELOAD_RESTART_TIMEOUT: Duration = Duration::from_mins(1);

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
    fn is_alive(&self) -> bool;

    /// Mark this transport unsafe for another request after an abandoned
    /// exchange. Stdio uses this to fence already-queued writers before the
    /// manager can acquire exclusive ownership and rebuild the process.
    fn invalidate(&self) {}

    /// Whether abandoning a request makes the connection unsafe to reuse.
    fn requires_reestablish_after_cancel(&self) -> bool {
        false
    }

    /// Tear down the transport (stdio: kill the child process).
    async fn shutdown(&self);
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
        McpServerConfig::Stdio {
            command, args, env, ..
        } => Ok(Box::new(
            StdioTransport::spawn(name, command, args, env, pending_oauth_urls).await?,
        )),
        McpServerConfig::Http {
            url, headers, auth, ..
        } => Ok(Box::new(HttpTransport::connect(
            name,
            url,
            headers,
            auth,
            Arc::clone(oauth_bearer),
            sink,
        )?)),
    }
}

type ServerMap = HashMap<String, SupervisorHandle>;

#[cfg(test)]
fn server_handle(server: McpServer) -> SupervisorHandle {
    SupervisorHandle::connected(server)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    /// The caller cancelled the request. The serving transport must be
    /// re-established before this server accepts another request because a
    /// stdio response may still arrive on the abandoned stream.
    Cancelled,
    /// Tool-level failure (`isError` result) or malformed response; the
    /// string is the complete display message.
    Other(String),
}

impl McpRequestError {
    fn into_message(self, server_name: &str) -> String {
        match self {
            Self::Transport(e) => format!("MCP server '{server_name}': {e}"),
            Self::Cancelled => format!("MCP server '{server_name}': tool call cancelled"),
            Self::Other(message) => message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpToolCallError {
    Cancelled,
    Failed(String),
}

impl From<String> for McpToolCallError {
    fn from(message: String) -> Self {
        Self::Failed(message)
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
            other @ (McpRequestError::Transport(_)
            | McpRequestError::Cancelled
            | McpRequestError::Other(_)) => Self::Other(other.into_message(server_name)),
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
    transport: Arc<dyn McpTransport>,
    tools: std::sync::RwLock<Vec<McpToolDef>>,
    /// Config retained for reload comparison and for rebuilding the
    /// transport on respawn.
    config: McpServerConfig,
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
            transport: Arc::from(transport),
            tools: std::sync::RwLock::new(Vec::new()),
            config,
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
    ///
    /// # Panics
    /// Panics if the internal tool cache lock is poisoned.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpRequestError> {
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

        self.tools.write().unwrap().clone_from(&all_defs);
        Ok(all_defs)
    }

    fn tools(&self) -> Vec<McpToolDef> {
        self.tools.read().unwrap().clone()
    }

    /// Call a tool on this server via `tools/call`.
    ///
    /// # Errors
    /// Returns a `McpRequestError` when the `tools/call` request fails or the
    /// server reports a tool error.
    #[cfg(test)]
    async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<String, McpRequestError> {
        self.call_context()
            .call_tool(tool_name, arguments, cancel)
            .await
    }

    fn call_context(&self) -> CallContext {
        CallContext {
            name: self.name.clone(),
            transport: Arc::clone(&self.transport),
            config: self.config.clone(),
            tools_changed: Arc::clone(&self.tools_changed),
            oauth_bearer: Arc::clone(&self.oauth_bearer),
        }
    }

    #[cfg(test)]
    pub(crate) fn config(&self) -> McpServerConfig {
        self.config.clone()
    }

    async fn terminate(&self) {
        self.transport.shutdown().await;
    }

    /// Check whether the underlying transport is still usable.
    pub fn is_alive(&self) -> bool {
        self.transport.is_alive()
    }
    fn should_reestablish(&self, error: &McpRequestError) -> bool {
        should_reestablish(&self.config, error)
    }

    /// Run the post-connect handshake: `initialize` then the first
    /// `tools/list`.
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

        tracing::warn!(
            server = %self.name,
            error = %failure,
            "Handshake hit a recoverable transport failure; retrying once on a fresh connection"
        );
        self.transport = Arc::from(
            connect_transport(
                &self.name,
                &self.config,
                Arc::clone(&self.pending_oauth_urls),
                &self.oauth_bearer,
                notification_sink(&self.name, &self.tools_changed),
            )
            .await
            .map_err(HandshakeFailure::Other)?,
        );
        match self.handshake_attempt().await {
            Ok(()) => Ok(()),
            Err(error) => {
                let failure = HandshakeFailure::classify(error, &self.name);
                self.terminate().await;
                Err(failure)
            }
        }
    }

    async fn handshake_attempt(&mut self) -> Result<(), McpRequestError> {
        self.initialize().await?;
        self.list_tools().await?;
        Ok(())
    }

    async fn fresh_recovery(self) -> Result<Self, HandshakeFailure> {
        let name = self.name.clone();
        let config = self.config.clone();
        let pending_oauth_urls = Arc::clone(&self.pending_oauth_urls);
        let oauth_bearer = Arc::clone(&self.oauth_bearer);
        self.terminate().await;
        let mut replacement = Self::connect(&name, config, pending_oauth_urls, oauth_bearer)
            .await
            .map_err(HandshakeFailure::Other)?;
        replacement.handshake().await?;
        Ok(replacement)
    }
}

#[derive(Clone)]
pub(crate) struct CallContext {
    name: String,
    transport: Arc<dyn McpTransport>,
    config: McpServerConfig,
    tools_changed: Arc<AtomicBool>,
    oauth_bearer: SharedBearer,
}

impl CallContext {
    pub(crate) fn should_reestablish(&self, error: &McpRequestError) -> bool {
        should_reestablish(&self.config, error)
    }

    pub(crate) fn is_http(&self) -> bool {
        matches!(self.config, McpServerConfig::Http { .. })
    }

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

    pub(crate) async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<String, McpRequestError> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                self.transport.invalidate();
                return Err(McpRequestError::Cancelled);
            },
            result = self.request("tools/call", params, self.config.tool_call_timeout()) => result,
        }
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

    pub(crate) fn outcome(
        &self,
        epoch: u64,
        result: Result<String, McpRequestError>,
    ) -> CallOutcome {
        let recovery = match &result {
            Err(McpRequestError::Cancelled)
                if self.transport.requires_reestablish_after_cancel() =>
            {
                CallRecovery::CancelledTransport
            }
            Err(error) if should_reestablish(&self.config, error) => CallRecovery::Transport,
            Err(error) => oauth_recovery_kind_parts(&self.config, &self.oauth_bearer, error)
                .map(CallRecovery::OAuth)
                .unwrap_or(CallRecovery::None),
            Ok(_) => CallRecovery::None,
        };
        CallOutcome {
            epoch,
            result,
            recovery,
        }
    }
}

fn should_reestablish(config: &McpServerConfig, error: &McpRequestError) -> bool {
    let McpRequestError::Transport(transport_error) = error else {
        return false;
    };
    match config {
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

fn oauth_active(config: &McpServerConfig, bearer: &SharedBearer) -> bool {
    oauth_resource_url(config).is_some() && bearer.read().unwrap().is_some()
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
            auth: HttpAuth::OAuth(oauth),
            ..
        } => oauth
            .client
            .as_ref()
            .map(|client| client.client_id.as_str()),
        McpServerConfig::Http { .. } | McpServerConfig::Stdio { .. } => None,
    }
}

fn oauth_config(config: &McpServerConfig) -> Option<&OAuthConfig> {
    match config {
        McpServerConfig::Http {
            auth: HttpAuth::OAuth(oauth),
            ..
        } => Some(oauth),
        McpServerConfig::Http { .. } | McpServerConfig::Stdio { .. } => None,
    }
}

fn configured_oauth_scopes(config: &McpServerConfig) -> &[String] {
    oauth_config(config)
        .map(|oauth| oauth.scopes.as_slice())
        .unwrap_or_default()
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
/// exactly this flow.
struct PendingAuthFlow {
    owner: Option<(SupervisorHandle, u64)>,
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
    #[cfg(test)]
    pending_publications: tokio::sync::watch::Sender<u64>,
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
            #[cfg(test)]
            pending_publications: tokio::sync::watch::channel(0).0,
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
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(5);
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
) -> Result<String, String> {
    let Some(url) = oauth_resource_url(entry) else {
        return Err("server is not OAuth-eligible".to_string());
    };
    let configured_oauth = oauth_config(entry);
    let preconfigured = configured_oauth.and_then(|oauth| oauth.client.as_ref());
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

    // Configured and challenge-required scopes form the explicit request. The
    // resource's advertised set is the fallback only when neither is present;
    // prior grants are always unioned for step-up (REQ-MCP-011..012).
    let mut scopes = configured_oauth
        .map(|oauth| oauth.scopes.clone())
        .unwrap_or_default();
    let challenge_scopes = challenge
        .get("scope")
        .map(|scope| scope.split_whitespace())
        .into_iter()
        .flatten();
    extend_unique(&mut scopes, challenge_scopes);
    if scopes.is_empty() {
        scopes = prm.scopes_supported.clone();
    }
    extend_unique(&mut scopes, extra_scopes.iter().map(String::as_str));

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
        owner: None,
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
    #[cfg(test)]
    oauth_rt
        .pending_publications
        .send_modify(|version| *version += 1);
    tracing::info!(
        server = %name,
        url = %auth_url,
        "MCP server requires OAuth authorization; waiting for the operator"
    );
    Ok(auth_url)
}

/// An OAuth re-authorization condition on an authorized server, classified
/// from a failed call (REQ-MCP-012).
#[derive(Debug, Clone)]
pub(crate) enum OAuthRecoveryKind {
    /// 401: the access token expired or was revoked (`TokenRefreshNeeded`).
    Refresh { www_authenticate: Option<String> },
    /// 403 with an explicit `error="insufficient_scope"` challenge
    /// (`InsufficientScopeStepUp`). A plain 403 is not a step-up.
    StepUp { www_authenticate: String },
}

fn oauth_recovery_kind_parts(
    config: &McpServerConfig,
    bearer: &SharedBearer,
    error: &McpRequestError,
) -> Option<OAuthRecoveryKind> {
    let McpRequestError::Transport(transport_error) = error else {
        return None;
    };
    if !oauth_active(config, bearer) {
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
pub struct McpClientManager {
    servers: Arc<RwLock<ServerMap>>,
    /// Server names whose tools should be excluded from conversations.
    /// The servers remain connected for instant re-enable.
    disabled_servers: RwLock<std::collections::HashSet<String>>,
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
    #[cfg(test)]
    background_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// OAuth lifecycle state: the token/registration store, the local
    /// callback's base URL, and the pending authorization flows
    /// (REQ-MCP-009..012).
    oauth: Arc<OAuthRuntime>,
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
            reload_serial: tokio::sync::Mutex::new(()),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(test)]
            background_tasks: std::sync::Mutex::new(Vec::new()),
            oauth: Arc::new(OAuthRuntime::default()),
        }
    }

    #[cfg(test)]
    fn spawn_background(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        self.background_tasks
            .lock()
            .unwrap()
            .push(tokio::spawn(future));
    }

    #[allow(clippy::unused_self)]
    #[cfg(not(test))]
    fn spawn_background(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        std::mem::drop(tokio::spawn(future));
    }

    #[cfg(test)]
    async fn await_background_tasks(&self) {
        loop {
            let tasks = std::mem::take(&mut *self.background_tasks.lock().unwrap());
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                task.await.expect("background MCP task");
            }
        }
    }

    #[cfg(test)]
    fn track_background_task(&self, task: tokio::task::JoinHandle<()>) {
        self.background_tasks.lock().unwrap().push(task);
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    fn track_background_task(&self, task: tokio::task::JoinHandle<()>) {
        drop(task);
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
        let configured_scopes_changed =
            configured_oauth_scopes(old_config) != configured_oauth_scopes(new_config);
        if !resource_matches || client_id_changed || configured_scopes_changed {
            tracing::info!(
                server = %name,
                "Reload repointed, de-OAuthed, re-keyed, or changed configured scopes; discarding its stored token"
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
        let owner = if let Some((handle, epoch)) = resolved.owner.as_ref() {
            if handle.snapshot().epoch == *epoch {
                handle
                    .reconfigure(resolved.config.clone())
                    .await
                    .ok()
                    .map(|new_epoch| (handle.clone(), new_epoch))
            } else {
                None
            }
        } else {
            None
        };
        // The flow is resolved; stop its loopback listener (if any) so it does
        // not hold the port for the rest of its window. On exchange failure the
        // flow stays pending, so the listener keeps accepting (REQ-MCP-020).
        if let Some(handle) = self.oauth.loopback_listeners.lock().unwrap().remove(&name) {
            handle.abort();
        }
        let config = resolved.config;
        let owner = owner.or(resolved.owner);

        let manager = Arc::clone(self);
        let reconnect_name = name.clone();
        self.spawn_background(async move {
            if let Some((handle, epoch)) = owner {
                if handle.snapshot().epoch != epoch {
                    return;
                }
                match Self::connect_one(
                    &reconnect_name,
                    &config,
                    Arc::clone(&manager.pending_oauth_urls),
                    Arc::clone(&manager.oauth),
                )
                .await
                {
                    Ok(server) => {
                        if handle.publish(epoch, server).await {
                            manager
                                .pending_oauth_urls
                                .write()
                                .await
                                .remove(&reconnect_name);
                        }
                    }
                    Err(error) => {
                        handle.fail(epoch, error).await;
                    }
                }
            } else {
                let handle = {
                    let mut servers = manager.servers.write().await;
                    servers
                        .entry(reconnect_name.clone())
                        .or_insert_with(|| SupervisorHandle::connecting(config.clone()))
                        .clone()
                };
                manager
                    .configure_actor(reconnect_name, config, handle)
                    .await;
            }
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
        let flow = {
            let mut pending = self.oauth.pending.lock().unwrap();
            pending.remove(&name)
        };
        if let Some(handle) = self.oauth.loopback_listeners.lock().unwrap().remove(&name) {
            handle.abort();
        }
        self.pending_oauth_urls.write().await.remove(&name);
        tracing::warn!(
            server = %name,
            error = %error,
            "MCP OAuth authorization failed at the authorization server"
        );
        // Retain the denial as a failure rather than letting the server vanish
        // from status (REQ-MCP-018). The pending URL is already cleared, so
        // this records `failed`, not `unauthorized`.
        if let Some((handle, epoch)) = flow.and_then(|flow| flow.owner) {
            handle
                .fail(epoch, format!("authorization failed: {error}"))
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
        name: &str,
        config: &McpServerConfig,
        www_authenticate: Option<&str>,
    ) -> RefreshServerOutcome {
        let Some(url) = oauth_resource_url(config).map(str::to_string) else {
            return RefreshServerOutcome::Transient(format!(
                "MCP server '{name}': not OAuth-eligible"
            ));
        };
        let token = match self.oauth.store().token(name).await {
            Ok(Some(token)) => token,
            Ok(None) => {
                // The bearer cell is set but no row backs it (e.g. deleted by
                // a concurrent reload): nothing to refresh, re-prompt.
                return self
                    .reprompt_after_refresh_failure(
                        name,
                        config,
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
        match oauth_refresh(&self.oauth, name, &url, www_authenticate, &token).await {
            Ok(_) => RefreshServerOutcome::Refreshed,
            Err(RefreshFailure::Transient(e)) => RefreshServerOutcome::Transient(format!(
                "MCP server '{name}': OAuth token refresh failed: {e}"
            )),
            Err(RefreshFailure::Rejected(e)) => {
                self.reprompt_after_refresh_failure(name, config, www_authenticate, &e)
                    .await
            }
        }
    }

    fn bind_pending_flow_owner(
        &self,
        name: &str,
        handle: &SupervisorHandle,
        epoch: u64,
    ) -> Result<(), String> {
        if handle.snapshot().epoch != epoch {
            return Err("MCP OAuth flow was superseded before publication".to_string());
        }
        let mut pending = self.oauth.pending.lock().unwrap();
        let Some(flow) = pending.get_mut(name) else {
            return Err("MCP OAuth flow disappeared before publication".to_string());
        };
        flow.owner = Some((handle.clone(), epoch));
        Ok(())
    }

    /// `TokenRefreshFailed`: discard the dead token and surface a fresh
    /// authorization flow so the operator re-authorizes (REQ-MCP-012).
    async fn reprompt_after_refresh_failure(
        &self,
        name: &str,
        config: &McpServerConfig,
        www_authenticate: Option<&str>,
        reason: &str,
    ) -> RefreshServerOutcome {
        tracing::warn!(
            server = %name,
            "OAuth refresh rejected ({reason}); discarding token and re-prompting"
        );
        if let Err(e) = self.oauth.store().delete_token(name).await {
            tracing::warn!(server = %name, "Failed to delete rejected OAuth token: {e}");
        }
        match begin_oauth_flow(
            &self.oauth,
            &self.pending_oauth_urls,
            name,
            config,
            www_authenticate,
            Vec::new(),
        )
        .await
        {
            Ok(auth_url) => RefreshServerOutcome::Reprompt(format!(
                "MCP server '{name}': authorization expired; re-authorize at {auth_url}"
            )),
            Err(flow_error) => RefreshServerOutcome::Transient(format!(
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
        name: &str,
        config: &McpServerConfig,
        www_authenticate: &str,
    ) -> Result<(), String> {
        // Prior grants are read BEFORE the token is discarded; persisting
        // scopes on the token makes them available even across a restart.
        let prior_scopes = match self.oauth.store().token(name).await {
            Ok(Some(token)) => token.scopes,
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!(server = %name, "OAuth token lookup failed during step-up: {e}");
                Vec::new()
            }
        };
        if let Err(e) = self.oauth.store().delete_token(name).await {
            tracing::warn!(server = %name, "Failed to delete narrow OAuth token: {e}");
        }
        match begin_oauth_flow(
            &self.oauth,
            &self.pending_oauth_urls,
            name,
            config,
            Some(www_authenticate),
            prior_scopes,
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

    fn begin_actor_connect_at_epoch(
        &self,
        name: String,
        config: &McpServerConfig,
        handle: SupervisorHandle,
        epoch: u64,
    ) -> tokio::task::JoinHandle<()> {
        let connect_name = name.clone();
        let connect_config = config.clone();
        let pending = Arc::clone(&self.pending_oauth_urls);
        let oauth = Arc::clone(&self.oauth);
        tokio::spawn(async move {
            match Self::connect_one(
                &connect_name,
                &connect_config,
                Arc::clone(&pending),
                Arc::clone(&oauth),
            )
            .await
            {
                Ok(server) => {
                    if handle.publish(epoch, server).await {
                        pending.write().await.remove(&connect_name);
                    }
                }
                Err(error) => {
                    if let Some(url) = pending.read().await.get(&name).cloned() {
                        {
                            let mut flows = oauth.pending.lock().unwrap();
                            if let Some(flow) = flows.get_mut(&name) {
                                flow.owner = Some((handle.clone(), epoch));
                            }
                        }
                        handle.unauthorized(epoch, url, error).await;
                    } else {
                        handle.fail(epoch, error).await;
                    }
                }
            }
        })
    }

    async fn begin_actor_connect(
        &self,
        name: String,
        config: McpServerConfig,
        handle: SupervisorHandle,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let epoch = handle.reconfigure(config.clone()).await.ok()?;
        Some(self.begin_actor_connect_at_epoch(name, &config, handle, epoch))
    }

    async fn configure_actor(
        &self,
        name: String,
        config: McpServerConfig,
        handle: SupervisorHandle,
    ) {
        if let Some(task) = self.begin_actor_connect(name, config, handle).await {
            if let Err(error) = task.await {
                tracing::warn!(error = %error, "MCP connect task failed");
            }
        }
    }

    #[must_use = "await the handle when MCP tools must be ready before continuing"]
    pub fn start_background_discovery(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let _ = manager
                .reload_from_actor_configs(Self::read_all_configs())
                .await;
            let handles: Vec<SupervisorHandle> =
                manager.servers.read().await.values().cloned().collect();
            let settles =
                futures::future::join_all(handles.iter().map(SupervisorHandle::wait_for_settled));
            let _ = tokio::time::timeout(CONNECT_TIMEOUT, settles).await;
        })
    }

    pub async fn status(&self) -> Vec<McpServerStatus> {
        let handles: Vec<(String, SupervisorHandle)> = self
            .servers
            .read()
            .await
            .iter()
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect();
        let disabled = self.disabled_servers.read().await;
        let redirect_warning = self.oauth.redirect_warning();
        let pending_urls = self.pending_oauth_urls.read().await.clone();
        let mut statuses = Vec::new();
        for (name, handle) in handles {
            let Some(snapshot) = handle.status().await else {
                continue;
            };
            let pending_url = snapshot
                .pending_oauth_url
                .clone()
                .or_else(|| pending_urls.get(&name).cloned());
            let tools = match &snapshot.state {
                SupervisorState::Ready(server) => server.tools(),
                SupervisorState::Connecting
                | SupervisorState::Recovering
                | SupervisorState::Failed
                | SupervisorState::Removed => Vec::new(),
            };
            let (state, last_error) = match &snapshot.state {
                SupervisorState::Ready(_) if pending_url.is_some() => {
                    (McpConnState::Unauthorized, None)
                }
                SupervisorState::Ready(_) => (McpConnState::Ready, None),
                SupervisorState::Recovering if snapshot.pending_oauth_url.is_some() => {
                    (McpConnState::Unauthorized, None)
                }
                SupervisorState::Failed => (McpConnState::Failed, snapshot.last_error.clone()),
                SupervisorState::Connecting | SupervisorState::Recovering
                    if pending_url.is_some() =>
                {
                    (McpConnState::Unauthorized, None)
                }
                SupervisorState::Connecting
                | SupervisorState::Recovering
                | SupervisorState::Removed => continue,
            };
            statuses.push(McpServerStatus {
                name: name.clone(),
                state,
                transport: snapshot.config.transport_kind(),
                auth: if pending_url.is_some() {
                    McpAuthKind::Oauth
                } else {
                    snapshot.config.auth_kind()
                },
                tool_count: tools.len(),
                tools: tools.iter().map(|tool| tool.name.clone()).collect(),
                enabled: !disabled.contains(&name),
                pending_oauth_url: pending_url,
                last_error,
                auth_redirect_warning: matches!(state, McpConnState::Unauthorized)
                    .then(|| redirect_warning.clone())
                    .flatten(),
            });
        }
        statuses
    }

    pub async fn tool_definitions(&self) -> Vec<(String, McpToolDef)> {
        let handles: Vec<(String, SupervisorHandle)> = self
            .servers
            .read()
            .await
            .iter()
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect();
        let mut definitions = Vec::new();
        for (name, handle) in handles {
            if self.disabled_servers.read().await.contains(&name) {
                continue;
            }
            let Ok(mut outcome) = handle.inspect().await else {
                continue;
            };
            if outcome.epoch != handle.snapshot().epoch {
                let Ok(current) = handle.inspect().await else {
                    continue;
                };
                outcome = current;
            }
            if outcome.result.is_err()
                && outcome.recoverable
                && self
                    .recover_actor(&name, &handle, outcome.epoch, None)
                    .await
                    .is_ok()
            {
                let Ok(retry) = handle.inspect().await else {
                    continue;
                };
                outcome = retry;
            }
            if self.disabled_servers.read().await.contains(&name) {
                continue;
            }
            if let Ok(tools) = outcome.result {
                definitions.extend(
                    tools
                        .into_iter()
                        .map(|definition| (name.clone(), definition)),
                );
            }
        }
        definitions
    }

    /// # Errors
    /// Returns an error when the server is unavailable or the tool call fails.
    pub async fn call_tool(
        self: &Arc<Self>,
        server_name: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String> {
        self.call_tool_cancellable(server_name, tool_name, arguments, CancellationToken::new())
            .await
            .map_err(|error| match error {
                McpToolCallError::Cancelled => {
                    format!("MCP server '{server_name}': tool call cancelled")
                }
                McpToolCallError::Failed(message) => message,
            })
    }

    /// # Errors
    /// Returns cancellation or the serving/recovery failure.
    #[allow(clippy::too_many_lines)] // One ordered lifecycle: call, classify, recover, retry.
    pub async fn call_tool_cancellable(
        self: &Arc<Self>,
        server_name: &str,
        tool_name: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> Result<String, McpToolCallError> {
        if self.disabled_servers.read().await.contains(server_name) {
            return Err(McpToolCallError::Failed(format!(
                "MCP server '{server_name}' is disabled"
            )));
        }
        let handle = self
            .servers
            .read()
            .await
            .get(server_name)
            .cloned()
            .ok_or_else(|| {
                McpToolCallError::Failed(format!("MCP server '{server_name}' is not connected"))
            })?;
        let first = self
            .call_actor_when_ready(&handle, tool_name, arguments.clone(), cancel.clone())
            .await?;
        let recovery = first.recovery;
        match first.result {
            Ok(value) => return Ok(value),
            Err(McpRequestError::Cancelled) if matches!(recovery, CallRecovery::None) => {
                return Err(McpToolCallError::Cancelled);
            }
            Err(error) if matches!(recovery, CallRecovery::None) => {
                return Err(McpToolCallError::Failed(error.into_message(server_name)));
            }
            Err(McpRequestError::Cancelled) => {
                let manager = Arc::clone(self);
                let name = server_name.to_string();
                let recovery_handle = handle.clone();
                self.spawn_background(async move {
                    let _ = manager
                        .recover_actor(&name, &recovery_handle, first.epoch, None)
                        .await;
                });
                return Err(McpToolCallError::Cancelled);
            }
            Err(_) => match recovery {
                CallRecovery::OAuth(kind) => {
                    let manager = Arc::clone(self);
                    let name = server_name.to_string();
                    let recovery_handle = handle.clone();
                    let mut recovery = tokio::spawn(async move {
                        manager
                            .recover_oauth(&name, &recovery_handle, first.epoch, kind)
                            .await
                    });
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => {
                            self.track_background_task(tokio::spawn(async move {
                                let _ = recovery.await;
                            }));
                            return Err(McpToolCallError::Cancelled);
                        }
                        result = &mut recovery => {
                            result
                                .map_err(|error| McpToolCallError::Failed(format!("MCP OAuth recovery task failed: {error}")))??;
                        }
                    }
                }
                CallRecovery::Transport | CallRecovery::CancelledTransport => {
                    if let Err(error) = self
                        .recover_actor(server_name, &handle, first.epoch, Some(&cancel))
                        .await
                    {
                        return Err(if cancel.is_cancelled() {
                            McpToolCallError::Cancelled
                        } else {
                            McpToolCallError::Failed(error)
                        });
                    }
                }
                CallRecovery::None => unreachable!("handled above"),
            },
        }
        let retry_handle = self
            .servers
            .read()
            .await
            .get(server_name)
            .cloned()
            .ok_or_else(|| {
                McpToolCallError::Failed(format!("MCP server '{server_name}' is not connected"))
            })?;
        let retry = self
            .call_actor_when_ready(&retry_handle, tool_name, arguments, cancel)
            .await?;
        let retry_recovery = retry.recovery;
        match retry.result {
            Ok(value) => Ok(value),
            Err(McpRequestError::Cancelled) => {
                if matches!(retry_recovery, CallRecovery::CancelledTransport) {
                    self.recover_actor(server_name, &retry_handle, retry.epoch, None)
                        .await
                        .map_err(McpToolCallError::Failed)?;
                }
                Err(McpToolCallError::Cancelled)
            }
            Err(error) => {
                if matches!(retry_recovery, CallRecovery::Transport) {
                    let manager = Arc::clone(self);
                    let name = server_name.to_string();
                    let recovery_handle = retry_handle.clone();
                    self.spawn_background(async move {
                        let _ = manager
                            .recover_actor(&name, &recovery_handle, retry.epoch, None)
                            .await;
                    });
                }
                Err(McpToolCallError::Failed(error.into_message(server_name)))
            }
        }
    }

    async fn call_actor_when_ready(
        &self,
        handle: &SupervisorHandle,
        tool_name: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> Result<supervisor::CallOutcome, McpToolCallError> {
        loop {
            let snapshot = handle.snapshot();
            match snapshot.state {
                SupervisorState::Ready(_) => {
                    match handle
                        .call(tool_name.to_string(), arguments.clone(), cancel.clone())
                        .await
                    {
                        Ok(outcome) => return Ok(outcome),
                        Err(_)
                            if matches!(
                                handle.snapshot().state,
                                SupervisorState::Connecting | SupervisorState::Recovering
                            ) =>
                        {
                            self.wait_for_ready(handle, &cancel).await?;
                        }
                        Err(error) if error == "MCP tool call cancelled" => {
                            return Err(McpToolCallError::Cancelled);
                        }
                        Err(error) => return Err(McpToolCallError::Failed(error)),
                    }
                }
                SupervisorState::Connecting | SupervisorState::Recovering => {
                    self.wait_for_ready(handle, &cancel).await?;
                }
                SupervisorState::Failed | SupervisorState::Removed => {
                    return Err(McpToolCallError::Failed(
                        snapshot.last_error.unwrap_or_else(|| {
                            format!("MCP server is {:?}", snapshot.state).to_lowercase()
                        }),
                    ));
                }
            }
        }
    }

    async fn recover_actor(
        &self,
        server_name: &str,
        handle: &SupervisorHandle,
        observed_epoch: u64,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), String> {
        if self
            .servers
            .read()
            .await
            .get(server_name)
            .is_some_and(|current| !current.same_actor(handle))
        {
            return Ok(());
        }
        match handle.claim_recovery(observed_epoch).await {
            RecoveryClaim::Leader(permit) => {
                let pending = Arc::clone(&self.pending_oauth_urls);
                let oauth = Arc::clone(&self.oauth);
                let name = server_name.to_string();
                let leader = handle.clone();
                let mut task = tokio::spawn(async move {
                    let result =
                        Self::connect_one(&name, &permit.config, Arc::clone(&pending), oauth).await;
                    match result {
                        Ok(server) => {
                            if leader.publish(permit.epoch, server).await {
                                pending.write().await.remove(&name);
                            }
                        }
                        Err(error) => {
                            if let Some(url) = pending.read().await.get(&name).cloned() {
                                leader.unauthorized(permit.epoch, url, error).await;
                            } else {
                                leader.fail(permit.epoch, error).await;
                            }
                        }
                    }
                });
                if let Some(cancel) = cancel {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => {
                            self.track_background_task(task);
                            Err(format!("MCP server '{server_name}': tool call cancelled"))
                        }
                        result = &mut task => result
                            .map_err(|error| format!("MCP recovery task failed: {error}")),
                    }
                } else {
                    task.await
                        .map_err(|error| format!("MCP recovery task failed: {error}"))
                }
            }
            RecoveryClaim::Follow(mut snapshots) => loop {
                let snapshot = snapshots.borrow().clone();
                if !matches!(
                    snapshot.state,
                    SupervisorState::Recovering | SupervisorState::Connecting
                ) {
                    return if snapshot.is_ready() {
                        Ok(())
                    } else {
                        Err(snapshot
                            .last_error
                            .unwrap_or_else(|| "MCP recovery failed".to_string()))
                    };
                }
                if let Some(cancel) = cancel {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Err(
                            format!("MCP server '{server_name}': tool call cancelled")
                        ),
                        changed = snapshots.changed() => {
                            changed.map_err(|_| "MCP supervisor stopped".to_string())?;
                        }
                    }
                } else {
                    snapshots
                        .changed()
                        .await
                        .map_err(|_| "MCP supervisor stopped".to_string())?;
                }
            },
            RecoveryClaim::Stale => Ok(()),
            RecoveryClaim::Unavailable(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_lines)] // One ordered lifecycle: claim, refresh or step-up, publish/fail.
    async fn recover_oauth(
        self: &Arc<Self>,
        server_name: &str,
        handle: &SupervisorHandle,
        observed_epoch: u64,
        kind: OAuthRecoveryKind,
    ) -> Result<(), McpToolCallError> {
        let permit = match handle.claim_recovery(observed_epoch).await {
            RecoveryClaim::Leader(permit) => permit,
            RecoveryClaim::Follow(_) => {
                return self.wait_for_ready(handle, &CancellationToken::new()).await;
            }
            RecoveryClaim::Stale => return Ok(()),
            RecoveryClaim::Unavailable(error) => return Err(McpToolCallError::Failed(error)),
        };

        match kind {
            OAuthRecoveryKind::Refresh { www_authenticate } => {
                match self
                    .refresh_authorized_server(
                        server_name,
                        &permit.config,
                        www_authenticate.as_deref(),
                    )
                    .await
                {
                    RefreshServerOutcome::Refreshed => {
                        let result = Self::connect_one(
                            server_name,
                            &permit.config,
                            Arc::clone(&self.pending_oauth_urls),
                            Arc::clone(&self.oauth),
                        )
                        .await;
                        match result {
                            Ok(server) => {
                                if handle.publish(permit.epoch, server).await {
                                    Ok(())
                                } else {
                                    Err(McpToolCallError::Failed(
                                        "MCP OAuth recovery was superseded".to_string(),
                                    ))
                                }
                            }
                            Err(error) => {
                                handle.fail(permit.epoch, error.clone()).await;
                                Err(McpToolCallError::Failed(error))
                            }
                        }
                    }
                    RefreshServerOutcome::Transient(error) => {
                        let reconnect_epoch = handle
                            .reconfigure(permit.config.clone())
                            .await
                            .map_err(McpToolCallError::Failed)?;
                        let manager = Arc::clone(self);
                        let name = server_name.to_string();
                        let config = permit.config.clone();
                        let retry_handle = handle.clone();
                        self.spawn_background(async move {
                            loop {
                                if retry_handle.snapshot().epoch != reconnect_epoch {
                                    return;
                                }
                                match Self::connect_one(
                                    &name,
                                    &config,
                                    Arc::clone(&manager.pending_oauth_urls),
                                    Arc::clone(&manager.oauth),
                                )
                                .await
                                {
                                    Ok(server) => {
                                        retry_handle.publish(reconnect_epoch, server).await;
                                        return;
                                    }
                                    Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
                                }
                            }
                        });
                        Err(McpToolCallError::Failed(error))
                    }
                    RefreshServerOutcome::Reprompt(error) => {
                        self.bind_pending_flow_owner(server_name, handle, permit.epoch)
                            .map_err(McpToolCallError::Failed)?;
                        let url = self
                            .pending_oauth_urls
                            .read()
                            .await
                            .get(server_name)
                            .cloned()
                            .unwrap_or_default();
                        handle.unauthorized(permit.epoch, url, error.clone()).await;
                        Err(McpToolCallError::Failed(error))
                    }
                }
            }
            OAuthRecoveryKind::StepUp { www_authenticate } => {
                if let Err(error) = self
                    .step_up_authorization(server_name, &permit.config, &www_authenticate)
                    .await
                {
                    handle.fail(permit.epoch, error.clone()).await;
                    return Err(McpToolCallError::Failed(error));
                }
                self.bind_pending_flow_owner(server_name, handle, permit.epoch)
                    .map_err(McpToolCallError::Failed)?;
                let url = self
                    .pending_oauth_urls
                    .read()
                    .await
                    .get(server_name)
                    .cloned()
                    .unwrap_or_default();
                handle
                    .unauthorized(
                        permit.epoch,
                        url,
                        "additional OAuth scopes required".to_string(),
                    )
                    .await;
                self.wait_for_ready(handle, &CancellationToken::new()).await
            }
        }
    }

    async fn wait_for_ready(
        &self,
        handle: &SupervisorHandle,
        cancel: &CancellationToken,
    ) -> Result<(), McpToolCallError> {
        let mut snapshots = handle.subscribe();
        loop {
            let snapshot = snapshots.borrow().clone();
            match snapshot.state {
                SupervisorState::Ready(_) => return Ok(()),
                SupervisorState::Failed | SupervisorState::Removed => {
                    return Err(McpToolCallError::Failed(
                        snapshot
                            .last_error
                            .unwrap_or_else(|| "MCP recovery did not reach ready".to_string()),
                    ));
                }
                SupervisorState::Connecting | SupervisorState::Recovering => {}
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(McpToolCallError::Cancelled),
                changed = snapshots.changed() => {
                    changed.map_err(|_| McpToolCallError::Failed(
                        "MCP supervisor stopped".to_string()
                    ))?;
                }
            }
        }
    }

    pub async fn reload(&self) -> McpReloadResult {
        self.reload_from_actor_configs(Self::read_all_configs())
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn reload_from_actor_configs(
        &self,
        configs: Vec<(String, McpServerConfig)>,
    ) -> McpReloadResult {
        let _serial = self.reload_serial.lock().await;
        let desired: HashMap<String, McpServerConfig> = configs.into_iter().collect();
        let removed_names: Vec<String> = self
            .servers
            .read()
            .await
            .keys()
            .filter(|name| !desired.contains_key(*name))
            .cloned()
            .collect();
        let mut failed = Vec::new();
        let mut removed = Vec::new();
        for name in removed_names {
            if let Some(handle) = self.servers.write().await.remove(&name) {
                let pending_flow = self.oauth.pending.lock().unwrap().remove(&name);
                match self.oauth.store().delete_token(&name).await {
                    Ok(()) => {
                        handle.remove().await;
                        if let Some(listener) =
                            self.oauth.loopback_listeners.lock().unwrap().remove(&name)
                        {
                            listener.abort();
                        }
                        self.pending_oauth_urls.write().await.remove(&name);
                        removed.push(name);
                    }
                    Err(error) => {
                        self.servers.write().await.insert(name.clone(), handle);
                        if let Some(flow) = pending_flow {
                            self.oauth
                                .pending
                                .lock()
                                .unwrap()
                                .insert(name.clone(), flow);
                        }
                        tracing::warn!(server = %name, error = %error, "MCP token deletion failed; server removal aborted");
                        failed.push(McpReloadFailure {
                            server: name,
                            action: "remove".to_string(),
                            error: error.clone(),
                        });
                    }
                }
            }
        }

        let mut added = Vec::new();
        let mut restarted = Vec::new();
        let mut unchanged = Vec::new();
        for (name, config) in desired {
            let existing = self.servers.read().await.get(&name).cloned();
            if existing.is_none() {
                let stale_flow = self.oauth.pending.lock().unwrap().remove(&name);
                if stale_flow.is_some() {
                    self.pending_oauth_urls.write().await.remove(&name);
                }
            }
            let (handle, is_restart) = match existing {
                Some(handle) if handle.snapshot().config == config => {
                    match handle.snapshot().state {
                        SupervisorState::Failed => (handle, true),
                        SupervisorState::Ready(_)
                        | SupervisorState::Connecting
                        | SupervisorState::Recovering
                        | SupervisorState::Removed => {
                            unchanged.push(name);
                            continue;
                        }
                    }
                }
                Some(handle) => {
                    let old = handle.snapshot().config;
                    self.cancel_pending_oauth_flow(&name).await;
                    self.pending_oauth_urls.write().await.remove(&name);
                    self.invalidate_oauth_on_config_change(&name, &old, &config)
                        .await;
                    (handle, true)
                }
                None => {
                    added.push(name.clone());
                    let handle = SupervisorHandle::connecting(config.clone());
                    self.servers
                        .write()
                        .await
                        .insert(name.clone(), handle.clone());
                    (handle, false)
                }
            };
            let task = self
                .begin_actor_connect(name.clone(), config, handle.clone())
                .await;
            if is_restart {
                if let Some(mut task) = task {
                    if tokio::time::timeout(RELOAD_RESTART_TIMEOUT, &mut task)
                        .await
                        .is_err()
                    {
                        let error = format!(
                            "timed out after {}s restarting changed MCP server",
                            RELOAD_RESTART_TIMEOUT.as_secs()
                        );
                        handle.fail(handle.snapshot().epoch, error.clone()).await;
                        failed.push(McpReloadFailure {
                            server: name,
                            action: "restart".to_string(),
                            error,
                        });
                        continue;
                    }
                }
                let snapshot = handle.snapshot();
                match snapshot.state {
                    SupervisorState::Ready(_) => restarted.push(name),
                    SupervisorState::Connecting
                    | SupervisorState::Recovering
                    | SupervisorState::Failed
                    | SupervisorState::Removed => failed.push(McpReloadFailure {
                        server: name,
                        action: "restart".to_string(),
                        error: snapshot
                            .last_error
                            .unwrap_or_else(|| "restart did not reach ready".to_string()),
                    }),
                }
            } else if let Some(task) = task {
                self.track_background_task(task);
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

    #[cfg(test)]
    async fn reload_from_configs(
        &self,
        configs: Vec<(String, McpServerConfig)>,
    ) -> McpReloadResult {
        self.reload_from_actor_configs(configs).await
    }

    pub async fn shutdown(&self) {
        let handles = std::mem::take(&mut *self.servers.write().await);
        for (_, handle) in handles {
            handle.shutdown().await;
        }
    }
}

impl McpClientManager {
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
                        match server.fresh_recovery().await {
                            Ok(recovered) => return Ok(recovered),
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
        let tool_call_timeout = match cfg.get("timeoutSeconds") {
            None => DEFAULT_TOOL_CALL_TIMEOUT,
            Some(value) => {
                let Some(seconds) = value.as_u64().filter(|seconds| *seconds > 0) else {
                    tracing::debug!(
                        server = %name,
                        "'timeoutSeconds' must be a positive integer; skipping server"
                    );
                    return None;
                };
                Duration::from_secs(seconds)
            }
        };
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
                tool_call_timeout,
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
            tool_call_timeout,
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
    /// (REQ-MCP-010). A whitespace-delimited `scopes` string supplies the
    /// initial authorization scope set (REQ-MCP-011). No client secret is read:
    /// the flow is authorization-code + PKCE, a public client.
    fn classify_oauth_auth(name: &str, oauth_value: &Value) -> Option<HttpAuth> {
        match oauth_value {
            Value::Bool(true) => Some(HttpAuth::OAuth(OAuthConfig::default())),
            Value::Object(fields) => {
                let scopes = match fields.get("scopes") {
                    None => Vec::new(),
                    Some(Value::String(scopes)) => {
                        let mut parsed = Vec::new();
                        extend_unique(&mut parsed, scopes.split_whitespace());
                        if parsed.is_empty() {
                            tracing::debug!(
                                server = %name,
                                "'oauth.scopes' must contain at least one scope; skipping server"
                            );
                            return None;
                        }
                        parsed
                    }
                    Some(_) => {
                        tracing::debug!(
                            server = %name,
                            "'oauth.scopes' must be a whitespace-delimited string; skipping server"
                        );
                        return None;
                    }
                };

                let client = match fields.get("clientId") {
                    Some(Value::String(client_id)) if !client_id.is_empty() => {
                        let callback_port = match fields.get("callbackPort") {
                            None => None,
                            Some(value) => {
                                match value.as_u64().and_then(|p| u16::try_from(p).ok()) {
                                    Some(port) if port != 0 => Some(port),
                                    _ => {
                                        tracing::debug!(
                                            server = %name,
                                            "'oauth.callbackPort' must be an integer 1-65535; skipping server"
                                        );
                                        return None;
                                    }
                                }
                            }
                        };
                        Some(PreconfiguredClient {
                            client_id: client_id.clone(),
                            callback_port,
                        })
                    }
                    Some(_) => {
                        tracing::debug!(
                            server = %name,
                            "'oauth.clientId' must be a non-empty string; skipping server"
                        );
                        return None;
                    }
                    None => None,
                };

                Some(HttpAuth::OAuth(OAuthConfig { client, scopes }))
            }
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
}

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
        tool_call_timeout: Duration,
    },
    Http {
        url: String,
        /// Generic per-request headers (org id, beta flag, ...) attached
        /// under ANY auth scheme; they do not imply auth and must not
        /// preempt OAuth (REQ-MCP-008).
        headers: HashMap<String, String>,
        auth: HttpAuth,
        tool_call_timeout: Duration,
    },
}

impl McpServerConfig {
    fn tool_call_timeout(&self) -> Duration {
        match self {
            Self::Stdio {
                tool_call_timeout, ..
            }
            | Self::Http {
                tool_call_timeout, ..
            } => *tool_call_timeout,
        }
    }
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

fn extend_unique<'a>(target: &mut Vec<String>, scopes: impl IntoIterator<Item = &'a str>) {
    for scope in scopes {
        if !target.iter().any(|existing| existing == scope) {
            target.push(scope.to_string());
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
    /// OAuth 2.1 configuration, including an optional pre-configured client
    /// identity and the initial scopes requested from the operator.
    OAuth(OAuthConfig),
}

/// An explicit, config-supplied auth credential (REQ-MCP-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticCred {
    Bearer(String),
    /// Designated auth headers (e.g. an API-key header), NOT the generic
    /// per-request `headers`.
    Headers(HashMap<String, String>),
}

/// Operator-supplied OAuth behavior from the top-level `oauth` config object.
/// Keeping client identity and initial scopes in one typed value makes dropping
/// either field during config discovery impossible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client: Option<PreconfiguredClient>,
    pub scopes: Vec<String>,
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
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
            }
        );
    }

    #[test]
    fn classify_entry_parses_and_validates_timeout_seconds() {
        let configured = serde_json::json!({
            "command": "uvx",
            "timeoutSeconds": 900,
        });
        let config =
            McpClientManager::classify_config_entry("s", &configured).expect("configured timeout");
        assert_eq!(config.tool_call_timeout(), Duration::from_secs(900));

        for invalid in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("300"),
        ] {
            let cfg = serde_json::json!({
                "command": "uvx",
                "timeoutSeconds": invalid,
            });
            assert_eq!(
                McpClientManager::classify_config_entry("s", &cfg),
                None,
                "invalid timeout must skip server: {cfg}"
            );
        }
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
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
            })
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One table-driven assertion over all supported OAuth config shapes.
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
                    auth: HttpAuth::OAuth(OAuthConfig::default()),
                    tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
                }),
                "oauth = {oauth_value} must select dynamic-client OAuth"
            );
        }

        // Pre-configured client for a DCR-less authorization server: `clientId`
        // names the pre-registered public app; the authorization server is
        // discovered and no client secret is accepted.
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
                auth: HttpAuth::OAuth(OAuthConfig {
                    client: Some(PreconfiguredClient {
                        client_id: "cid-1".to_string(),
                        callback_port: Some(3118),
                    }),
                    scopes: Vec::new(),
                }),
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
            })
        );

        let configured_scopes = serde_json::json!({
            "type": "http",
            "url": "https://mcp.slack.com/mcp",
            "oauth": {
                "clientId": "1601185624273.8899143856786",
                "callbackPort": 3118,
                "scopes": "channels:history  groups:history channels:history chat:write"
            },
        });
        assert_eq!(
            McpClientManager::classify_config_entry("slack", &configured_scopes),
            Some(McpServerConfig::Http {
                url: "https://mcp.slack.com/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::OAuth(OAuthConfig {
                    client: Some(PreconfiguredClient {
                        client_id: "1601185624273.8899143856786".to_string(),
                        callback_port: Some(3118),
                    }),
                    scopes: vec![
                        "channels:history".to_string(),
                        "groups:history".to_string(),
                        "chat:write".to_string(),
                    ],
                }),
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
            })
        );

        let dynamic_with_scopes = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "oauth": {"scopes": "read write"},
        });
        assert_eq!(
            McpClientManager::classify_config_entry("dynamic", &dynamic_with_scopes),
            Some(McpServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::OAuth(OAuthConfig {
                    client: None,
                    scopes: vec!["read".to_string(), "write".to_string()],
                }),
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
            })
        );
    }

    #[test]
    fn classify_oauth_rejects_malformed_scopes() {
        for bad in [
            serde_json::json!(""),
            serde_json::json!([]),
            serde_json::json!(42),
        ] {
            let cfg = serde_json::json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "oauth": {"scopes": bad},
            });
            assert_eq!(
                McpClientManager::classify_config_entry("s", &cfg),
                None,
                "scopes {bad} must skip the server"
            );
        }
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
        started: Option<tokio::sync::oneshot::Sender<String>>,
    }

    fn exchange(result: Result<Value, TransportError>) -> ScriptedExchange {
        ScriptedExchange {
            server_messages: Vec::new(),
            result,
            delay: Duration::ZERO,
            started: None,
        }
    }

    fn delayed_exchange(result: Result<Value, TransportError>, delay_ms: u64) -> ScriptedExchange {
        ScriptedExchange {
            server_messages: Vec::new(),
            result,
            delay: Duration::from_millis(delay_ms),
            started: None,
        }
    }

    fn witnessed_delayed_exchange(
        result: Result<Value, TransportError>,
        delay_ms: u64,
    ) -> (ScriptedExchange, tokio::sync::oneshot::Receiver<String>) {
        let (started, receiver) = tokio::sync::oneshot::channel();
        let mut exchange = delayed_exchange(result, delay_ms);
        exchange.started = Some(started);
        (exchange, receiver)
    }

    struct FakeTransport {
        script: std::sync::Mutex<std::collections::VecDeque<ScriptedExchange>>,
        requests: Arc<std::sync::Mutex<Vec<(String, Value, Duration)>>>,
        notifications: Arc<std::sync::Mutex<Vec<Value>>>,
    }

    #[async_trait]
    impl McpTransport for FakeTransport {
        async fn request(
            &self,
            method: &str,
            params: Value,
            timeout: Duration,
            sink: &dyn ServerMessageSink,
        ) -> Result<Value, TransportError> {
            self.requests
                .lock()
                .unwrap()
                .push((method.to_string(), params, timeout));
            let mut exchange = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("unscripted request");
            if let Some(started) = exchange.started.take() {
                let _ = started.send(method.to_string());
            }
            if exchange.delay > Duration::ZERO {
                // test-timing-allow: scripted transport latency is the behavior exercised by timeout tests
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

        fn is_alive(&self) -> bool {
            true
        }

        async fn shutdown(&self) {}
    }

    type RequestLog = Arc<std::sync::Mutex<Vec<(String, Value, Duration)>>>;
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
            transport: Arc::new(transport),
            tools: std::sync::RwLock::new(Vec::new()),
            config,
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
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
        let (server, requests, _) = fake_server(vec![
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
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
            .await
            .expect_err("isError result must be an error");

        assert!(!server.should_reestablish(&err));
        assert_eq!(err.into_message("fake"), "boom");
    }

    #[tokio::test]
    async fn call_tool_allows_five_minutes_for_response() {
        let (server, requests, _) = fake_server(vec![exchange(Ok(serde_json::json!({
            "content": [],
        })))]);

        server
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
            .await
            .expect("call_tool");

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "tools/call");
        assert_eq!(requests[0].2, Duration::from_mins(5));
    }

    #[tokio::test]
    async fn call_tool_uses_per_server_timeout_override() {
        let (server, requests, _) = fake_server_with_config(
            vec![exchange(Ok(serde_json::json!({"content": []})))],
            McpServerConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                tool_call_timeout: Duration::from_secs(900),
            },
        );

        server
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
            .await
            .expect("call_tool");

        assert_eq!(requests.lock().unwrap()[0].2, Duration::from_secs(900));
    }

    #[test]
    fn timeout_change_participates_in_reload_comparison() {
        let base = McpServerConfig::Stdio {
            command: "unused".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        };
        let mut changed = base.clone();
        if let McpServerConfig::Stdio {
            tool_call_timeout, ..
        } = &mut changed
        {
            *tool_call_timeout = Duration::from_secs(900);
        }
        assert_ne!(base, changed);
    }

    #[tokio::test]
    async fn long_tool_call_does_not_hold_global_server_map_lock() {
        let (exchange, started) =
            witnessed_delayed_exchange(Ok(serde_json::json!({"content": []})), 100);
        let (server, _, _) = fake_server(vec![exchange]);
        let manager = Arc::new(McpClientManager::new());
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), server_handle(server));

        let caller = Arc::clone(&manager);
        let call = tokio::spawn(async move {
            caller
                .call_tool("fake", "report", serde_json::json!({}))
                .await
        });
        started.await.expect("tool call started");

        let map_guard = tokio::time::timeout(Duration::from_secs(1), manager.servers.write())
            .await
            .expect("global server map remains writable during tools/call");
        drop(map_guard);
        call.await.expect("call task").expect("tool call");
    }

    #[tokio::test]
    async fn call_started_during_recovery_waits_for_the_fresh_server() {
        let (serving, _, _) = fake_server(Vec::new());
        let handle = server_handle(serving);
        let manager = Arc::new(McpClientManager::new());
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), handle.clone());
        let RecoveryClaim::Leader(permit) = handle.claim_recovery(0).await else {
            panic!("recovery claim");
        };

        let caller = Arc::clone(&manager);
        let call = tokio::spawn(async move {
            caller
                .call_tool("fake", "report", serde_json::json!({}))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!call.is_finished(), "call follows the recovery snapshot");

        let (replacement, _, _) = fake_server(vec![exchange(Ok(serde_json::json!({
            "content": [{"type": "text", "text": "fresh"}],
        })))]);
        assert!(handle.publish(permit.epoch, replacement).await);
        assert_eq!(call.await.expect("call task").expect("tool call"), "fresh");
    }

    #[tokio::test]
    async fn reload_removal_waits_without_global_map_lock() {
        let (exchange, started) =
            witnessed_delayed_exchange(Ok(serde_json::json!({"content": []})), 200);
        let (server, _, _) = fake_server(vec![exchange]);
        let manager = Arc::new(McpClientManager::new());
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), server_handle(server));

        let caller = Arc::clone(&manager);
        let call = tokio::spawn(async move {
            caller
                .call_tool("fake", "report", serde_json::json!({}))
                .await
        });
        started.await.expect("call started");
        let reloader = Arc::clone(&manager);
        let reload = tokio::spawn(async move { reloader.reload_from_configs(Vec::new()).await });
        tokio::task::yield_now().await;

        let map_guard = tokio::time::timeout(Duration::from_secs(1), manager.servers.write())
            .await
            .expect("reload waits for removed server without global map lock");
        drop(map_guard);
        let call_result = call.await.expect("call task");
        assert!(
            call_result.is_ok()
                || call_result
                    .as_ref()
                    .is_err_and(|error| error.contains("supervisor stopped")),
            "a removal may complete or supersede the already-started call: {call_result:?}"
        );
        let result = reload.await.expect("reload task");
        assert_eq!(result.removed, vec!["fake"]);
    }

    #[tokio::test]
    async fn server_message_on_sink_sets_tools_changed() {
        let (server, _, _) = fake_server(vec![ScriptedExchange {
            server_messages: vec![serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed",
            })],
            result: Ok(serde_json::json!({"content": []})),
            started: None,
            delay: Duration::ZERO,
        }]);

        assert!(!server.tools_changed.load(Ordering::Acquire));
        server
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
            .await
            .expect("call_tool");
        assert!(server.tools_changed.load(Ordering::Acquire));
    }

    fn http_none_config(url: &str) -> McpServerConfig {
        McpServerConfig::Http {
            url: url.to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        }
    }

    #[tokio::test]
    async fn status_retains_failed_server_with_cause_and_clears_on_reconnect() {
        let manager = Arc::new(McpClientManager::new());
        let config = http_none_config("https://remote.example/mcp");
        let handle = SupervisorHandle::connecting(config.clone());
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), handle.clone());
        handle.fail(0, "connection refused".to_string()).await;

        let status = manager.status().await;
        assert_eq!(status.len(), 1, "a failed server is retained, not dropped");
        let s = &status[0];
        assert_eq!(s.name, "remote");
        assert!(matches!(s.state, McpConnState::Failed));
        assert!(matches!(s.transport, McpTransportKind::Http));
        assert_eq!(s.last_error.as_deref(), Some("connection refused"));

        let epoch = handle
            .reconfigure(config.clone())
            .await
            .expect("reconfigure");
        let (server, _, _) = fake_server_with_config(Vec::new(), config);
        assert!(handle.publish(epoch, server).await);
        assert!(matches!(
            manager.status().await[0].state,
            McpConnState::Ready
        ));
    }

    #[tokio::test]
    async fn reload_sweeps_failed_only_servers_dropped_from_config() {
        let manager = Arc::new(McpClientManager::new());
        let handle = SupervisorHandle::connecting(http_none_config("https://gone.example/mcp"));
        manager
            .servers
            .write()
            .await
            .insert("gone".to_string(), handle.clone());
        handle.fail(0, "boom".into()).await;
        // A failed-only server (never connected, so absent from the connected
        // map) dropped from config must be swept, not linger in status.
        manager.reload_from_configs(vec![]).await;
        assert!(manager.status().await.is_empty());
    }

    #[tokio::test]
    async fn failed_entry_reflects_disabled_state() {
        let manager = Arc::new(McpClientManager::new());
        let handle = SupervisorHandle::connecting(http_none_config("https://remote.example/mcp"));
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), handle.clone());
        handle.fail(0, "x".into()).await;
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
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_warning(Some("callback unreachable".to_string()));
        let handle = SupervisorHandle::connecting(http_none_config("https://remote.example/mcp"));
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), handle.clone());
        manager.pending_oauth_urls.write().await.insert(
            "remote".to_string(),
            "https://auth.example/authorize".to_string(),
        );
        handle
            .unauthorized(
                0,
                "https://auth.example/authorize".to_string(),
                "HTTP 401".to_string(),
            )
            .await;

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
        let manager = Arc::new(McpClientManager::new());
        let handle = SupervisorHandle::connecting(http_none_config("https://remote.example/mcp"));
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), handle.clone());
        manager.pending_oauth_urls.write().await.insert(
            "remote".to_string(),
            "https://auth.example/authorize".to_string(),
        );
        handle
            .unauthorized(
                0,
                "https://auth.example/authorize".to_string(),
                "HTTP 401".to_string(),
            )
            .await;

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
        let manager = Arc::new(McpClientManager::new());
        let handle = SupervisorHandle::connecting(http_none_config("https://remote.example/mcp"));
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), handle.clone());
        handle.fail(0, "earlier failure".to_string()).await;
        let epoch = handle
            .reconfigure(http_none_config("https://remote.example/mcp"))
            .await
            .expect("retry");
        manager.pending_oauth_urls.write().await.insert(
            "remote".to_string(),
            "https://auth.example/authorize".to_string(),
        );
        handle
            .unauthorized(
                epoch,
                "https://auth.example/authorize".to_string(),
                "HTTP 401".to_string(),
            )
            .await;

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
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
            .await
            .expect_err("disconnected must fail");
        assert!(server.should_reestablish(&crash));

        let timeout = server
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
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
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
        let (serving_exchange, serving_started) =
            witnessed_delayed_exchange(Err(TransportError::SessionExpired), 200);
        let (serving, _, _) = fake_server_with_config(
            vec![serving_exchange],
            McpServerConfig::Http {
                url: "http://127.0.0.1:1/mcp".to_string(),
                headers: HashMap::new(),
                auth: HttpAuth::None,
                tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
            },
        );
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), server_handle(serving));

        let (replacement, replacement_requests, _) = fake_server_with_config(
            vec![exchange(Ok(serde_json::json!({
                "content": [{"type": "text", "text": "fresh"}]
            })))],
            stdio_test_config("replacement"),
        );
        let swapper = Arc::clone(&manager);
        let swap = tokio::spawn(async move {
            let method = tokio::time::timeout(Duration::from_secs(5), serving_started)
                .await
                .expect("serving request started in time")
                .expect("serving transport retained its witness");
            assert_eq!(method, "tools/call");
            swapper
                .servers
                .write()
                .await
                .insert("fake".to_string(), server_handle(replacement));
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
        let (serving_exchange, serving_started) = witnessed_delayed_exchange(
            Err(TransportError::Timeout(
                "timed out reading response for 'tools/call'".to_string(),
            )),
            200,
        );
        let (serving, _, _) =
            fake_server_with_config(vec![serving_exchange], stdio_test_config("serving"));
        manager
            .servers
            .write()
            .await
            .insert("fake".to_string(), server_handle(serving));

        let (replacement, replacement_requests, _) =
            fake_server_with_config(Vec::new(), stdio_test_config("replacement"));
        let swapper = Arc::clone(&manager);
        let swap = tokio::spawn(async move {
            let method = tokio::time::timeout(Duration::from_secs(5), serving_started)
                .await
                .expect("serving request started in time")
                .expect("serving transport retained its witness");
            assert_eq!(method, "tools/call");
            swapper
                .servers
                .write()
                .await
                .insert("fake".to_string(), server_handle(replacement));
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
        append_marker("call")
        block_once = os.environ.get("MCP_BLOCK_ONCE_FILE")
        if block_once and os.path.exists(block_once):
            os.remove(block_once)
            append_marker("block-start")
            import time
            time.sleep(60)
        crash_then_block = os.environ.get("MCP_CRASH_THEN_BLOCK_FILE")
        if crash_then_block:
            if os.path.exists(crash_then_block):
                os.remove(crash_then_block)
                os._exit(2)
            append_marker("retry-start")
            import time
            time.sleep(60)
        if os.environ.get("MCP_BLOCK_CALL"):
            append_marker("call-start")
            import time
            time.sleep(60)
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
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        }
    }

    /// Mutable access to a config's stdio fields; panics on an Http config.
    fn as_stdio_mut(
        config: &mut McpServerConfig,
    ) -> (&mut String, &mut Vec<String>, &mut HashMap<String, String>) {
        match config {
            McpServerConfig::Stdio {
                command, args, env, ..
            } => (command, args, env),
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
            .insert("fixture".to_string(), server_handle(server));
    }

    fn marker_lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn queued_stdio_call_is_fenced_from_cancelled_transport() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let block_once = tmp.path().join("block-once");
        std::fs::write(&block_once, "block").expect("write block marker");
        let manager = Arc::new(McpClientManager::new());
        let mut config = fixture_config(&script, &marker, "v1", "env1");
        as_stdio_mut(&mut config).2.insert(
            "MCP_BLOCK_ONCE_FILE".to_string(),
            block_once.display().to_string(),
        );
        connect_fixture(&manager, &config).await;

        let cancel = CancellationToken::new();
        let first_manager = Arc::clone(&manager);
        let first_cancel = cancel.clone();
        let first = tokio::spawn(async move {
            first_manager
                .call_tool_cancellable("fixture", "report", serde_json::json!({}), first_cancel)
                .await
        });
        while !marker_lines(&marker)
            .iter()
            .any(|line| line.starts_with("block-start|"))
        {
            tokio::task::yield_now().await;
        }
        let second_manager = Arc::clone(&manager);
        let second = tokio::spawn(async move {
            second_manager
                .call_tool("fixture", "report", serde_json::json!({}))
                .await
        });
        tokio::task::yield_now().await;
        cancel.cancel();

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), first)
                .await
                .expect("cancelled call settles")
                .expect("first task"),
            Err(McpToolCallError::Cancelled)
        );
        let output = tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .expect("queued call settles after respawn")
            .expect("second task")
            .expect("queued call succeeds");
        assert_eq!(output, "label=v1;env=env1");

        let lines = marker_lines(&marker);
        let first_pid = lines
            .iter()
            .find(|line| line.starts_with("block-start|"))
            .and_then(|line| line.split('|').find(|part| part.starts_with("pid=")))
            .expect("blocked process pid");
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("call|") && line.contains(first_pid))
                .count(),
            1,
            "queued call never writes to the invalidated process"
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn cancelled_retry_reestablishes_before_returning() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let crash_then_block = tmp.path().join("crash-then-block");
        std::fs::write(&crash_then_block, "crash").expect("write crash marker");
        let manager = Arc::new(McpClientManager::new());
        let mut config = fixture_config(&script, &marker, "v1", "env1");
        as_stdio_mut(&mut config).2.insert(
            "MCP_CRASH_THEN_BLOCK_FILE".to_string(),
            crash_then_block.display().to_string(),
        );
        connect_fixture(&manager, &config).await;

        let cancel = CancellationToken::new();
        let caller = Arc::clone(&manager);
        let call_cancel = cancel.clone();
        let call = tokio::spawn(async move {
            caller
                .call_tool_cancellable("fixture", "report", serde_json::json!({}), call_cancel)
                .await
        });
        while !marker_lines(&marker)
            .iter()
            .any(|line| line.starts_with("retry-start|"))
        {
            tokio::task::yield_now().await;
        }

        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .expect("cancelled retry settles promptly")
            .expect("call task")
            .expect_err("retry is cancelled");
        assert_eq!(error, McpToolCallError::Cancelled);
        assert_eq!(
            marker_lines(&marker)
                .iter()
                .filter(|line| line.starts_with("start|"))
                .count(),
            3,
            "initial crash recovery plus retry cancellation each respawn"
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn cancelled_stdio_call_reestablishes_before_next_call() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = Arc::new(McpClientManager::new());
        let mut config = fixture_config(&script, &marker, "v1", "env1");
        as_stdio_mut(&mut config)
            .2
            .insert("MCP_BLOCK_CALL".to_string(), "1".to_string());
        connect_fixture(&manager, &config).await;

        let cancel = CancellationToken::new();
        let caller = Arc::clone(&manager);
        let call_cancel = cancel.clone();
        let call = tokio::spawn(async move {
            caller
                .call_tool_cancellable("fixture", "report", serde_json::json!({}), call_cancel)
                .await
        });
        while !marker_lines(&marker)
            .iter()
            .any(|line| line.starts_with("call-start|"))
        {
            tokio::task::yield_now().await;
        }

        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .expect("cancelled call settles promptly")
            .expect("call task")
            .expect_err("call is cancelled");
        assert_eq!(error, McpToolCallError::Cancelled);
        let mut snapshots = manager
            .servers
            .read()
            .await
            .get("fixture")
            .expect("fixture supervisor")
            .subscribe();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !snapshots.borrow().is_ready() {
                snapshots.changed().await.expect("supervisor remains alive");
            }
        })
        .await
        .expect("cancellation recovery settles");

        let changed = fixture_config(&script, &marker, "v2", "env2");
        let reload = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;
        assert_eq!(reload.restarted, vec!["fixture"]);
        let output = manager
            .call_tool("fixture", "report", serde_json::json!({}))
            .await
            .expect("next call uses a clean transport");
        assert_eq!(output, "label=v2;env=env2");
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn reload_same_config_is_unchanged_without_respawn() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = Arc::new(McpClientManager::new());
        let config = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &config).await;

        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), config)])
            .await;

        assert_eq!(result.unchanged, vec!["fixture"]);
        assert!(result.restarted.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(
            marker_lines(&marker)
                .iter()
                .filter(|line| line.starts_with("start|"))
                .count(),
            1
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn reload_changed_args_restarts_and_uses_new_args() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = Arc::new(McpClientManager::new());
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        let changed = fixture_config(&script, &marker, "v2", "env1");
        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), changed)])
            .await;

        assert_eq!(result.restarted, vec!["fixture"]);
        assert!(result.unchanged.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(
            marker_lines(&marker)
                .iter()
                .filter(|line| line.starts_with("start|"))
                .count(),
            2
        );
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
        let manager = Arc::new(McpClientManager::new());
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
        let manager = Arc::new(McpClientManager::new());
        let initial = fixture_config(&script, &marker, "v1", "env1");
        connect_fixture(&manager, &initial).await;

        // Port 1 refuses connections, so the HTTP handshake fails fast.
        let changed = McpServerConfig::Http {
            url: "http://127.0.0.1:1/mcp".to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
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
        let manager = Arc::new(McpClientManager::new());
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
        let manager = Arc::new(McpClientManager::new());
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
        let manager = Arc::new(McpClientManager::new());
        let config = fixture_config(&script, &marker, "v1", "env1");

        let result = manager
            .reload_from_configs(vec![("fixture".to_string(), config)])
            .await;

        assert_eq!(result.added, vec!["fixture"]);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            manager.await_background_tasks(),
        )
        .await
        .expect("fixture connected in background");
        assert_eq!(manager.status().await.len(), 1);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn respawn_after_changed_config_reload_uses_new_config() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let crash_once = tmp.path().join("crash-once");
        std::fs::write(&crash_once, "crash").expect("write crash marker");
        let manager = Arc::new(McpClientManager::new());
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
        assert_eq!(
            marker_lines(&marker)
                .iter()
                .filter(|line| line.starts_with("start|"))
                .count(),
            3
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn stdio_server_request_with_colliding_id_is_not_mistaken_for_reply() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let script = write_fixture_server(&tmp);
        let marker = tmp.path().join("marker.log");
        let manager = Arc::new(McpClientManager::new());
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
