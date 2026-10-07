//! Streamable HTTP MCP transport (REQ-MCP-004, REQ-MCP-005, REQ-MCP-008).
//!
//! A remote MCP server exposes a single endpoint URL: JSON-RPC requests go
//! out as POSTs, and a response arrives either as `application/json` (one
//! JSON-RPC reply) or as `text/event-stream` (a sequence of JSON-RPC
//! messages, ending with the reply). Server-initiated messages on a stream
//! are forwarded to the `ServerMessageSink`; the protocol layer interprets
//! them. Unlike stdio, requests are not serialized: each POST is an
//! independent HTTP exchange correlated by the JSON-RPC id.

use crate::{HttpAuth, McpTransport, ServerMessageSink, SharedBearer, StaticCred, TransportError};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

const MCP_SESSION_ID: HeaderName = HeaderName::from_static("mcp-session-id");
const MCP_PROTOCOL_VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");

/// Both response framings a Streamable HTTP server may choose (REQ-MCP-004).
const ACCEPT_BOTH: &str = "application/json, text/event-stream";

/// The lone framing the server-initiated GET stream may return (REQ-MCP-006).
const ACCEPT_EVENT_STREAM: &str = "text/event-stream";

/// Streamable HTTP transport for one MCP server.
pub struct HttpTransport {
    name: String,
    client: reqwest::Client,
    url: String,
    /// Generic per-request headers plus any static auth credential, resolved
    /// once from config and attached to every request (REQ-MCP-008).
    base_headers: HeaderMap,
    /// `Mcp-Session-Id` captured from the `initialize` response and echoed
    /// on every subsequent request (REQ-MCP-005). None for stateless servers.
    session_id: std::sync::Mutex<Option<String>>,
    /// Negotiated protocol version from the `initialize` result, sent as the
    /// `MCP-Protocol-Version` header on every later request (REQ-MCP-004).
    protocol_version: std::sync::Mutex<Option<String>>,
    /// The server's shared OAuth bearer, attached as `Authorization: Bearer`
    /// on every request — initialize, tools/*, the GET stream, and the session
    /// DELETE (REQ-MCP-012). `None` for static-credential servers, whose config
    /// authorization is already in `base_headers` and must not be shadowed.
    oauth_bearer: Option<SharedBearer>,
    /// Protocol-layer handler for messages the server pushes on its
    /// server-initiated GET stream (REQ-MCP-006).
    sink: Arc<dyn ServerMessageSink>,
    /// The detached task reading the server-initiated GET stream, spawned once
    /// the `initialize` handshake has negotiated the session and protocol
    /// version. Aborted on shutdown and on drop so a torn-down transport never
    /// leaves a stream task reconnecting against a dead connection.
    stream_task: std::sync::Mutex<Option<JoinHandle<()>>>,
    next_id: AtomicU64,
}

impl HttpTransport {
    /// Build the transport. Performs no I/O; the connection is exercised by
    /// the `initialize` handshake.
    ///
    /// # Errors
    /// Returns a display string when a configured header or credential
    /// cannot be encoded as an HTTP header.
    pub fn connect(
        name: &str,
        url: &str,
        headers: &HashMap<String, String>,
        auth: &HttpAuth,
        oauth_bearer: SharedBearer,
        sink: Arc<dyn ServerMessageSink>,
    ) -> Result<Self, String> {
        let mut base_headers = HeaderMap::new();
        for (key, value) in headers {
            // An Authorization header is a credential, and credentials are
            // classified through `auth` (REQ-MCP-008). Smuggled in as a
            // generic header it would both dodge the StaticAuthRejected
            // semantics and ride alongside an OAuth bearer as a second
            // Authorization value once a flow completes. Reject loudly
            // rather than silently misclassify.
            if key.eq_ignore_ascii_case("authorization") {
                return Err(format!(
                    "MCP server '{name}': 'Authorization' is a credential and cannot be a \
                     generic header; configure it as auth.bearer or auth.headers"
                ));
            }
            insert_header(&mut base_headers, name, key, value)?;
        }
        match auth {
            // OAuth credentials come from the authorization flow, not config;
            // until a token exists the handshake goes out unauthenticated and
            // the server's 401 drives the flow.
            HttpAuth::None | HttpAuth::OAuth(_) => {}
            HttpAuth::Static(StaticCred::Bearer(token)) => {
                let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                    format!("MCP server '{name}': bearer token is not a valid header value")
                })?;
                base_headers.insert(AUTHORIZATION, value);
            }
            HttpAuth::Static(StaticCred::Headers(auth_headers)) => {
                for (key, value) in auth_headers {
                    insert_header(&mut base_headers, name, key, value)?;
                }
            }
        }

        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("MCP server '{name}': failed to build HTTP client: {e}"))?;

        Ok(Self {
            name: name.to_string(),
            client,
            url: url.to_string(),
            base_headers,
            session_id: std::sync::Mutex::new(None),
            protocol_version: std::sync::Mutex::new(None),
            // A static credential owns the Authorization header; the OAuth
            // bearer applies only when no config credential does.
            oauth_bearer: match auth {
                HttpAuth::None | HttpAuth::OAuth(_) => Some(oauth_bearer),
                HttpAuth::Static(_) => None,
            },
            sink,
            stream_task: std::sync::Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    /// The current OAuth bearer header value, when one applies.
    fn bearer_header(&self) -> Option<HeaderValue> {
        bearer_header(self.oauth_bearer.as_ref())
    }

    /// Open the server-initiated GET stream once `initialize` has negotiated
    /// the session and protocol version (REQ-MCP-006). Idempotent: a transport
    /// runs `initialize` once, but a second call is a no-op rather than a
    /// second stream. The task is detached and survives until the transport is
    /// quiesced or torn down, so `tools/list_changed` keeps arriving
    /// between requests instead of only on a POST reply.
    fn start_server_stream(&self) {
        let mut slot = self.stream_task.lock().unwrap();
        if slot.is_some() {
            return;
        }
        let stream = ServerStream {
            client: self.client.clone(),
            url: self.url.clone(),
            base_headers: self.base_headers.clone(),
            oauth_bearer: self.oauth_bearer.clone(),
            session_id: self.session_id.lock().unwrap().clone(),
            protocol_version: self.protocol_version.lock().unwrap().clone(),
            name: self.name.clone(),
            sink: Arc::clone(&self.sink),
        };
        *slot = Some(tokio::spawn(stream.run()));
    }

    /// A POST to the MCP endpoint carrying the base headers, the Accept pair,
    /// and the session/protocol-version headers when negotiated. Also returns
    /// the session id this request carries: concurrent requests classify a
    /// later 404 against what *they* sent, not the shared state, which
    /// another request's recovery may have changed meanwhile.
    fn post(&self, timeout: Duration) -> (reqwest::RequestBuilder, Option<String>) {
        let session_id = self.session_id.lock().unwrap().clone();
        let mut builder = self
            .client
            .post(&self.url)
            .timeout(timeout)
            .headers(self.base_headers.clone())
            .header(ACCEPT, ACCEPT_BOTH);
        if let Some(bearer) = self.bearer_header() {
            builder = builder.header(AUTHORIZATION, bearer);
        }
        if let Some(session_id) = &session_id {
            builder = builder.header(MCP_SESSION_ID, session_id.as_str());
        }
        if let Some(version) = self.protocol_version.lock().unwrap().clone() {
            builder = builder.header(MCP_PROTOCOL_VERSION, version);
        }
        (builder, session_id)
    }

    /// Classify an HTTP status into a `TransportError`, or pass a success
    /// status through. `Ok` carries the response back for body handling.
    /// `sent_session_id` is the session id this particular request carried.
    fn classify_status(
        response: reqwest::Response,
        sent_session_id: Option<&str>,
    ) -> Result<reqwest::Response, TransportError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        // A response may carry several challenges across repeated
        // WWW-Authenticate headers (e.g. Basic first, Bearer second); the
        // Bearer one carries the OAuth discovery and step-up parameters
        // (resource_metadata, error, scope), so it is selected explicitly
        // rather than taking whichever header happens to be first.
        let www_authenticate = crate::oauth::select_bearer_challenge(
            response
                .headers()
                .get_all("www-authenticate")
                .iter()
                .filter_map(|v| v.to_str().ok()),
        );
        match status.as_u16() {
            401 => Err(TransportError::Unauthorized { www_authenticate }),
            403 => Err(TransportError::InsufficientScope { www_authenticate }),
            404 if sent_session_id.is_some() => {
                // The server-side session is gone (REQ-MCP-005). The stored
                // id is deliberately NOT cleared: recovery replaces this
                // whole transport (the fresh one re-initializes with no
                // session), and clearing here would let a concurrent call
                // race in session-less -- failing as a generic protocol
                // error instead of classifying as expired and joining the
                // recovery.
                Err(TransportError::SessionExpired)
            }
            _ => Err(TransportError::Protocol(format!(
                "HTTP {status} from MCP endpoint"
            ))),
        }
    }

    /// Dispatch one JSON-RPC message from a response body: the correlated
    /// reply is returned, server-initiated messages go to the sink, and a
    /// mismatched reply is logged and dropped.
    fn dispatch_message(
        &self,
        message: Value,
        id: u64,
        sink: &dyn ServerMessageSink,
    ) -> Option<Result<Value, TransportError>> {
        // A message carrying a `method` is server-initiated (a request or a
        // notification); responses never carry one. Its id space is
        // independent of ours -- a server `ping` whose id collides with our
        // request id is not our reply -- so this check must come before id
        // correlation.
        if message.get("method").is_some() {
            sink.on_message(message);
            return None;
        }

        if message.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                return Some(Err(TransportError::Rpc {
                    code,
                    message: text,
                }));
            }
            return Some(message.get("result").cloned().ok_or_else(|| {
                TransportError::Protocol("response missing both 'result' and 'error'".to_string())
            }));
        }

        tracing::warn!(
            server = %self.name,
            expected_id = id,
            got = ?message.get("id"),
            "Mismatched response id, skipping"
        );
        None
    }

    fn classify_request_error(error: &reqwest::Error, method: &str) -> TransportError {
        if error.is_timeout() {
            TransportError::Timeout(format!("request timed out for '{method}'"))
        } else if error.is_decode() {
            TransportError::Protocol(format!("failed to decode response for '{method}': {error}"))
        } else {
            TransportError::Disconnected(format!("request failed for '{method}': {error}"))
        }
    }
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        // Safety net for any teardown path that drops the transport without
        // `shutdown` (e.g. a connect failure unwinding): the detached GET
        // stream task must not outlive the transport it belongs to.
        if let Ok(slot) = self.stream_task.get_mut() {
            if let Some(task) = slot.take() {
                task.abort();
            }
        }
    }
}

/// The `Authorization: Bearer` header value for an OAuth bearer cell, when one
/// applies and currently holds a token. Shared by the request path and the
/// server-initiated GET stream so both attach the live, rotated token.
fn bearer_header(oauth_bearer: Option<&SharedBearer>) -> Option<HeaderValue> {
    let token = oauth_bearer?.read().unwrap().clone()?;
    HeaderValue::from_str(&format!("Bearer {token}")).ok()
}

/// Whether a response is framed as a server-sent-event stream.
fn is_event_stream(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| {
            ct.trim_start()
                .to_ascii_lowercase()
                .starts_with("text/event-stream")
        })
}

fn insert_header(map: &mut HeaderMap, server: &str, key: &str, value: &str) -> Result<(), String> {
    // The session, protocol-version, and resume headers are transport state; a
    // config-supplied copy would ride alongside the real value (reqwest's
    // `.header()` appends rather than replaces) and could bind a request to
    // the wrong session or replay from a stale event id. Reject loudly rather
    // than silently dropping what the user wrote.
    if key.eq_ignore_ascii_case("mcp-session-id")
        || key.eq_ignore_ascii_case("mcp-protocol-version")
        || key.eq_ignore_ascii_case("last-event-id")
    {
        return Err(format!(
            "MCP server '{server}': header '{key}' is transport-managed and cannot be set in config"
        ));
    }
    let header_name = HeaderName::from_bytes(key.as_bytes())
        .map_err(|_| format!("MCP server '{server}': invalid header name '{key}'"))?;
    let header_value = HeaderValue::from_str(value)
        .map_err(|_| format!("MCP server '{server}': invalid value for header '{key}'"))?;
    map.insert(header_name, header_value);
    Ok(())
}

#[async_trait]
impl McpTransport for HttpTransport {
    /// POST one JSON-RPC request. Requests are concurrent by design: HTTP
    /// correlates each POST with its own response, so no per-server
    /// round-trip lock exists here (unlike stdio).
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        sink: &dyn ServerMessageSink,
    ) -> Result<Value, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (builder, sent_session_id) = self.post(timeout);
        let response = builder
            .json(&body)
            .send()
            .await
            .map_err(|e| Self::classify_request_error(&e, method))?;

        // The session id is issued on the initialize response (REQ-MCP-005).
        if method == "initialize" {
            if let Some(session_id) = response
                .headers()
                .get(&MCP_SESSION_ID)
                .and_then(|v| v.to_str().ok())
            {
                *self.session_id.lock().unwrap() = Some(session_id.to_string());
            }
        }

        let response = Self::classify_status(response, sent_session_id.as_deref())?;

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        let result = if content_type.starts_with("text/event-stream") {
            // A stream of JSON-RPC messages delivered as SSE events; the
            // correlated reply ends the wait (REQ-MCP-004).
            let mut framer = SseFramer::default();
            let mut stream = response.bytes_stream();
            let mut outcome = None;
            'read: while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| Self::classify_request_error(&e, method))?;
                for data in framer.push(&chunk) {
                    if let Some(found) = self.dispatch_sse_data(&data, id, method, sink) {
                        outcome = Some(found);
                        break 'read;
                    }
                }
            }
            if outcome.is_none() {
                if let Some(data) = framer.finish() {
                    outcome = self.dispatch_sse_data(&data, id, method, sink);
                }
            }
            outcome.unwrap_or_else(|| {
                Err(TransportError::Disconnected(format!(
                    "SSE stream ended without a response to '{method}'"
                )))
            })?
        } else {
            // A single JSON-RPC reply (REQ-MCP-004).
            let message: Value = response
                .json()
                .await
                .map_err(|e| Self::classify_request_error(&e, method))?;
            self.dispatch_message(message, id, sink)
                .unwrap_or_else(|| {
                    Err(TransportError::Protocol(format!(
                        "response body did not answer request '{method}'"
                    )))
                })?
        };

        // The protocol version negotiated at initialize rides every later
        // request (REQ-MCP-004); once it is known the server-initiated GET
        // stream can open (REQ-MCP-006).
        if method == "initialize" {
            if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
                *self.protocol_version.lock().unwrap() = Some(version.to_string());
            }
            self.start_server_stream();
        }

        Ok(result)
    }

    async fn notify(&self, notification: &Value) -> Result<(), TransportError> {
        let (builder, sent_session_id) = self.post(crate::NOTIFY_TIMEOUT);
        let response = builder
            .json(notification)
            .send()
            .await
            .map_err(|e| Self::classify_request_error(&e, "notification"))?;

        // A conforming server acknowledges an accepted notification with
        // 202 Accepted and no body (REQ-MCP-004); any 2xx is success and the
        // body, if present, is ignored.
        Self::classify_status(response, sent_session_id.as_deref()).map(|_| ())
    }

    fn requested_protocol_version(&self) -> &'static str {
        // The revision that introduced the Streamable HTTP transport;
        // earlier revisions speak the deprecated HTTP+SSE transport
        // (REQ-MCP-019).
        "2025-03-26"
    }

    fn is_alive(&self) -> bool {
        // No process to probe; failures are classified per request and
        // recovery is reconnection (REQ-MCP-007).
        true
    }

    async fn quiesce(&self) -> Result<(), TransportError> {
        let stream_task = self.stream_task.lock().unwrap().take();
        if let Some(task) = stream_task {
            task.abort();
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    return Err(TransportError::Disconnected(format!(
                        "MCP server '{}': stream task failed during quiescence: {error}",
                        self.name,
                    )));
                }
            }
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), TransportError> {
        let background_error = self.quiesce().await.err();
        // End the server-side session explicitly so it does not linger until
        // expiry (REQ-MCP-005). Stateless servers have nothing to delete.
        let session_id = self.session_id.lock().unwrap().clone();
        if let Some(session_id) = session_id {
            let mut builder = self
                .client
                .delete(&self.url)
                .timeout(Duration::from_secs(5))
                .headers(self.base_headers.clone())
                .header(MCP_SESSION_ID, &session_id);
            // The bearer rides the session DELETE too (REQ-MCP-012).
            if let Some(bearer) = self.bearer_header() {
                builder = builder.header(AUTHORIZATION, bearer);
            }
            // The negotiated protocol version rides every post-initialize
            // request, the session DELETE included (REQ-MCP-004).
            if let Some(version) = self.protocol_version.lock().unwrap().clone() {
                builder = builder.header(MCP_PROTOCOL_VERSION, version);
            }
            let response = builder.send().await.map_err(|error| {
                TransportError::Disconnected(format!(
                    "MCP server '{}': session DELETE failed during shutdown: {error}",
                    self.name
                ))
            })?;
            if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND
            {
                self.session_id.lock().unwrap().take();
            } else {
                return Err(TransportError::Disconnected(format!(
                    "MCP server '{}': session DELETE failed during shutdown with HTTP {}",
                    self.name,
                    response.status()
                )));
            }
        }
        match background_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl HttpTransport {
    fn dispatch_sse_data(
        &self,
        data: &str,
        id: u64,
        method: &str,
        sink: &dyn ServerMessageSink,
    ) -> Option<Result<Value, TransportError>> {
        match serde_json::from_str::<Value>(data) {
            Ok(message) => self.dispatch_message(message, id, sink),
            Err(e) => Some(Err(TransportError::Protocol(format!(
                "invalid JSON in SSE event for '{method}': {e}"
            )))),
        }
    }
}

// ---------------------------------------------------------------------------
// Server-initiated GET stream (REQ-MCP-006)
// ---------------------------------------------------------------------------

/// What ended one attempt at the server-initiated GET stream.
enum StreamOutcome {
    /// The server does not offer a stream at this endpoint (HTTP 405). The MCP
    /// spec's signal for "no server-initiated stream"; do not reconnect.
    Unsupported,
    /// A condition no reconnect can recover (a non-OAuth auth rejection, or a
    /// 404 whose recovery is a fresh transport, not a re-GET). Stop the task.
    Fatal(String),
    /// The stream connected and then dropped, or failed to connect. `productive`
    /// is true when it delivered at least one event or stayed open long enough
    /// to count as healthy, which resets the reconnect backoff. `retry_after`
    /// carries a server-sent SSE `retry:` hint, when one arrived, to time the
    /// reconnect in place of local backoff.
    Disconnected {
        productive: bool,
        reason: String,
        retry_after: Option<Duration>,
    },
}

/// The owned slice of an `HttpTransport` a detached GET-stream task needs. It
/// captures the session/protocol version negotiated at `initialize` (fixed for
/// the transport's life) and reads the OAuth bearer cell live, so a token
/// rotated by a concurrent refresh is picked up on the next reconnect.
struct ServerStream {
    client: reqwest::Client,
    url: String,
    base_headers: HeaderMap,
    oauth_bearer: Option<SharedBearer>,
    session_id: Option<String>,
    protocol_version: Option<String>,
    name: String,
    sink: Arc<dyn ServerMessageSink>,
}

impl ServerStream {
    /// Reconnect backoff bounds. A productive connection resets to the base; an
    /// immediately-closing one ramps to the cap, bounding a broken server to one
    /// reconnect per `BACKOFF_MAX` rather than a hot loop.
    const BACKOFF_BASE: Duration = Duration::from_secs(1);
    const BACKOFF_MAX: Duration = Duration::from_secs(30);
    /// Floor for a server-sent `retry:` hint, so a `retry: 0` cannot turn the
    /// reconnect into a hot loop while still honoring sub-second requests.
    const RETRY_MIN: Duration = Duration::from_millis(250);
    /// A connection open at least this long counts as healthy even if it
    /// delivered no events (an idle but live stream).
    const HEALTHY_AFTER: Duration = Duration::from_secs(5);

    async fn run(self) {
        let mut last_event_id: Option<String> = None;
        let mut backoff = Self::BACKOFF_BASE;
        // A server-sent `retry:` sets the reconnection time until the server
        // updates it (SSE processing model), so it persists across reconnects
        // rather than applying only to the stream that carried it.
        let mut retry_hint: Option<Duration> = None;
        loop {
            match self.connect_once(&mut last_event_id).await {
                StreamOutcome::Unsupported => {
                    tracing::debug!(
                        server = %self.name,
                        "MCP server offers no server-initiated SSE stream (HTTP 405); not reconnecting"
                    );
                    return;
                }
                StreamOutcome::Fatal(reason) => {
                    tracing::debug!(
                        server = %self.name,
                        reason,
                        "MCP server-initiated stream stopped"
                    );
                    return;
                }
                StreamOutcome::Disconnected {
                    productive,
                    reason,
                    retry_after,
                } => {
                    if productive {
                        backoff = Self::BACKOFF_BASE;
                    }
                    if let Some(hint) = retry_after {
                        retry_hint = Some(hint.clamp(Self::RETRY_MIN, Self::BACKOFF_MAX));
                    }
                    // A remembered server `retry:` governs the delay; otherwise
                    // local backoff applies and ramps when unproductive.
                    let delay = retry_hint.unwrap_or(backoff);
                    tracing::debug!(
                        server = %self.name,
                        reason,
                        resume_from = ?last_event_id,
                        delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "MCP server-initiated stream dropped; reconnecting"
                    );
                    tokio::time::sleep(delay).await;
                    if !productive && retry_hint.is_none() {
                        backoff = (backoff * 2).min(Self::BACKOFF_MAX);
                    }
                }
            }
        }
    }

    /// Open the GET stream once and pump its events into the sink until it
    /// drops. Updates `last_event_id` as `id:` fields arrive so the caller
    /// resumes from the right point (REQ-MCP-006).
    async fn connect_once(&self, last_event_id: &mut Option<String>) -> StreamOutcome {
        let response = match self.open(last_event_id.as_deref()).await {
            Ok(response) => response,
            Err(reason) => {
                return StreamOutcome::Disconnected {
                    productive: false,
                    reason,
                    retry_after: None,
                }
            }
        };
        if let Some(outcome) = self.classify_stream_status(response.status()) {
            return outcome;
        }
        // A 2xx that is not `text/event-stream` (a JSON/HTML health page, a
        // 204) carries no stream: framing it yields no events and `pump` would
        // return at once, spinning a reconnect loop. The endpoint offers no
        // server-initiated stream, so stop like a 405 (REQ-MCP-006).
        if !is_event_stream(&response) {
            return StreamOutcome::Unsupported;
        }
        self.pump(response, last_event_id).await
    }

    /// Build and send the GET, attaching auth, session, protocol-version, and
    /// the resume cursor. Returns the open response or a display reason.
    async fn open(&self, resume_from: Option<&str>) -> Result<reqwest::Response, String> {
        let mut builder = self
            .client
            .get(&self.url)
            .headers(self.base_headers.clone())
            .header(ACCEPT, ACCEPT_EVENT_STREAM);
        if let Some(bearer) = bearer_header(self.oauth_bearer.as_ref()) {
            builder = builder.header(AUTHORIZATION, bearer);
        }
        if let Some(session_id) = &self.session_id {
            builder = builder.header(MCP_SESSION_ID, session_id.as_str());
        }
        if let Some(version) = &self.protocol_version {
            builder = builder.header(MCP_PROTOCOL_VERSION, version.as_str());
        }
        // Resume past the last delivered event so the server can replay what
        // was missed across the drop (REQ-MCP-006).
        if let Some(resume) = resume_from {
            if let Ok(value) = HeaderValue::from_str(resume) {
                builder = builder.header(LAST_EVENT_ID, value);
            }
        }
        builder
            .send()
            .await
            .map_err(|e| format!("GET stream request failed: {e}"))
    }

    /// Map a GET-stream response status to a terminal outcome, or `None` to
    /// proceed reading the stream body.
    fn classify_stream_status(&self, status: reqwest::StatusCode) -> Option<StreamOutcome> {
        match status.as_u16() {
            // A session-bearing 404 means the session is gone: signal the
            // protocol layer so the next definitions read re-establishes (new
            // session + fresh GET stream) rather than leaving the server
            // `ready` on a dead session (REQ-MCP-005); a re-GET on the dead
            // session would only 404 again.
            404 if self.session_id.is_some() => {
                self.sink.on_session_reset();
                Some(StreamOutcome::Fatal(
                    "GET stream session expired (HTTP 404)".to_string(),
                ))
            }
            // No stream offered here: 405 (method not allowed) or a 404 on a
            // stateless server. Stop without reconnecting.
            404 | 405 => Some(StreamOutcome::Unsupported),
            // A current token may be rotated by a concurrent refresh, so
            // reconnect and re-read the shared cell. With no token in hand
            // (a no-auth server, or one that never authorized) there is nothing
            // to recover, so stop rather than loop on a permanent rejection --
            // the presence of the shared cell alone (`HttpAuth::None` carries
            // one) is not authorization.
            401 | 403 if self.has_bearer() => Some(StreamOutcome::Disconnected {
                productive: false,
                reason: format!("GET stream rejected with HTTP {status}"),
                retry_after: None,
            }),
            401 | 403 => Some(StreamOutcome::Fatal(format!(
                "GET stream rejected with HTTP {status}"
            ))),
            _ if !status.is_success() => Some(StreamOutcome::Disconnected {
                productive: false,
                reason: format!("GET stream returned HTTP {status}"),
                retry_after: None,
            }),
            _ => None,
        }
    }

    /// Read the open stream's SSE body, forwarding each message to the sink,
    /// until the body ends or errors. The resume cursor is taken from the
    /// framer's last-event-id buffer at the drop, so an empty/bare `id` reset
    /// clears it rather than leaving a stale cursor (REQ-MCP-006).
    async fn pump(
        &self,
        response: reqwest::Response,
        last_event_id: &mut Option<String>,
    ) -> StreamOutcome {
        let started = Instant::now();
        let mut delivered = false;
        let mut framer = SseFramer::default();
        let mut stream = response.bytes_stream();
        let reason = loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    for data in framer.push(&chunk) {
                        self.dispatch(&data);
                        delivered = true;
                    }
                }
                Some(Err(e)) => break format!("GET stream read error: {e}"),
                None => {
                    if let Some(data) = framer.finish() {
                        self.dispatch(&data);
                        delivered = true;
                    }
                    break "GET stream closed by server".to_string();
                }
            }
        };
        last_event_id.clone_from(&framer.last_id);
        StreamOutcome::Disconnected {
            productive: delivered || started.elapsed() >= Self::HEALTHY_AFTER,
            reason,
            retry_after: framer.retry,
        }
    }

    /// Whether a usable OAuth bearer token is currently held. Distinct from the
    /// shared cell merely existing: a `HttpAuth::None` server carries the cell
    /// but holds no token, and a GET-stream 401/403 there is unrecoverable.
    fn has_bearer(&self) -> bool {
        bearer_header(self.oauth_bearer.as_ref()).is_some()
    }

    /// Forward server-initiated messages in one SSE event to the protocol
    /// layer. The MCP Streamable HTTP transport allows a JSON-RPC batch (an
    /// array) here, so an array is unpacked and each member dispatched; a bare
    /// object is dispatched directly. Only messages carrying a `method`
    /// (notifications, or server-initiated requests) belong on this stream; a
    /// stray response has no request of ours to answer.
    fn dispatch(&self, data: &str) {
        match serde_json::from_str::<Value>(data) {
            Ok(Value::Array(batch)) => batch.into_iter().for_each(|m| self.dispatch_message(m)),
            Ok(message) => self.dispatch_message(message),
            Err(e) => tracing::debug!(
                server = %self.name,
                "Invalid JSON on server-initiated stream: {e}"
            ),
        }
    }

    fn dispatch_message(&self, message: Value) {
        if message.get("method").is_some() {
            self.sink.on_message(message);
        } else {
            tracing::debug!(
                server = %self.name,
                "Ignoring non-method message on server-initiated stream"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SSE framing
// ---------------------------------------------------------------------------

/// Incremental SSE event framer over a byte stream: yields the joined `data:`
/// payload of each event. `event:` fields and comments are ignored. Per the
/// SSE processing model the `id:` field sets a "last event id" buffer that
/// persists across events (an empty or bare `id` resets it) and the `retry:`
/// field sets the reconnection time; the server-initiated GET stream reads both
/// off the framer to resume and pace reconnects (REQ-MCP-006). The POST reply
/// path consumes only the data payloads.
#[derive(Default)]
struct SseFramer {
    buf: Vec<u8>,
    data_lines: Vec<String>,
    last_id: Option<String>,
    retry: Option<Duration>,
}

impl SseFramer {
    /// Feed a chunk; returns the data payloads of the events completed by it.
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        self.buf.extend_from_slice(chunk);
        while let Some(newline) = self.buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.buf.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(self.data_lines.join("\n"));
                    self.data_lines.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data_lines
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            } else if line == "id" || line.starts_with("id:") {
                // The `id` field sets the persistent last event id. An empty
                // value (a bare `id` or `id:`) resets it so the next reconnect
                // omits Last-Event-ID; a NUL value is ignored per the SSE spec.
                let value = line
                    .strip_prefix("id:")
                    .map_or("", |rest| rest.strip_prefix(' ').unwrap_or(rest));
                if !value.contains('\0') {
                    self.last_id = (!value.is_empty()).then(|| value.to_string());
                }
            } else if let Some(rest) = line.strip_prefix("retry:") {
                // Per the SSE spec a `retry:` value is the reconnection time in
                // integer milliseconds; a non-integer value is ignored.
                let value = rest.strip_prefix(' ').unwrap_or(rest);
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(ms) = value.parse::<u64>() {
                        self.retry = Some(Duration::from_millis(ms));
                    }
                }
            }
        }
        events
    }

    /// Flush a final event not terminated by a blank line before EOF. EOF
    /// acts as the missing line terminator too: a server may close the
    /// response immediately after the last `data:` line, without a trailing
    /// newline, and that line must not be lost.
    fn finish(&mut self) -> Option<String> {
        self.push(b"\n\n").into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        server_handle, McpClientManager, McpConnState, McpServerConfig, DEFAULT_TOOL_CALL_TIMEOUT,
    };
    use std::collections::VecDeque;
    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{watch, RwLock};
    use tokio_util::sync::CancellationToken;

    // -----------------------------------------------------------------------
    // Minimal scripted HTTP/1.1 server: records requests, replays canned
    // responses in order. Hand-rolled so the tests exercise reqwest against
    // real sockets without an HTTP-server dependency in this crate.
    // -----------------------------------------------------------------------

    #[derive(Debug)]
    struct RecordedRequest {
        request_line: String,
        headers: HashMap<String, String>,
        body: String,
    }

    impl RecordedRequest {
        fn http_method(&self) -> &str {
            self.request_line.split(' ').next().unwrap_or("")
        }

        fn path(&self) -> &str {
            let target = self.request_line.split(' ').nth(1).unwrap_or("");
            target.split('?').next().unwrap_or("")
        }

        fn rpc_method(&self) -> String {
            serde_json::from_str::<Value>(&self.body)
                .ok()
                .and_then(|v| v.get("method").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_default()
        }

        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).map(String::as_str)
        }
    }

    #[derive(Clone)]
    struct CannedResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
        /// Sleep before responding -- lets a test order concurrent exchanges.
        delay_ms: u64,
        /// When set, the body is built by echoing the request's JSON-RPC id,
        /// for exchanges whose request id depends on scheduling order.
        echo_result: Option<Value>,
    }

    fn json_response(id: u64, result: &Value, headers: &[(&str, &str)]) -> CannedResponse {
        let mut all = vec![("content-type".to_string(), "application/json".to_string())];
        all.extend(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
        );
        CannedResponse {
            status: 200,
            headers: all,
            body: serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
            delay_ms: 0,
            echo_result: None,
        }
    }

    fn echo_id_response(result: &Value) -> CannedResponse {
        CannedResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: String::new(),
            delay_ms: 0,
            echo_result: Some(result.clone()),
        }
    }

    fn accepted() -> CannedResponse {
        CannedResponse {
            status: 202,
            headers: Vec::new(),
            body: String::new(),
            delay_ms: 0,
            echo_result: None,
        }
    }

    fn sse_response(body: &str) -> CannedResponse {
        CannedResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
            body: body.to_string(),
            delay_ms: 0,
            echo_result: None,
        }
    }

    fn status_response(status: u16, headers: &[(&str, &str)]) -> CannedResponse {
        CannedResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            body: String::new(),
            delay_ms: 0,
            echo_result: None,
        }
    }

    /// Ack for the session DELETE that re-establish sends while tearing
    /// down a session-bearing transport.
    fn delete_ack() -> CannedResponse {
        status_response(200, &[])
    }

    fn delayed(mut response: CannedResponse, delay_ms: u64) -> CannedResponse {
        response.delay_ms = delay_ms;
        response
    }

    /// Path-routed responses: a queue per path, consumed in order with the
    /// final entry replayed indefinitely. Lets one scripted server carry the
    /// OAuth endpoints (metadata, registration, token) alongside the
    /// in-order /mcp queue, which keeps serving any unrouted path.
    #[derive(Default)]
    struct ResponseRoutes {
        paths: Mutex<HashMap<String, VecDeque<CannedResponse>>>,
        delete_bearer: Mutex<Option<String>>,
        token_gate: Mutex<
            Option<(
                tokio::sync::mpsc::UnboundedSender<()>,
                Arc<tokio::sync::Semaphore>,
            )>,
        >,
        initialize_gate: Mutex<
            Option<(
                tokio::sync::mpsc::UnboundedSender<()>,
                Arc<tokio::sync::Semaphore>,
            )>,
        >,
    }

    type RouteMap = Arc<ResponseRoutes>;

    struct TestServer {
        url: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        responses: Arc<Mutex<VecDeque<CannedResponse>>>,
        routes: RouteMap,
        /// The server-initiated GET stream is a channel of its own: GETs are
        /// logged and answered separately from the POST queue so a transport's
        /// background stream (REQ-MCP-006) neither steals a POST's canned reply
        /// nor inflates the POST request count an assertion checks. Unconfigured,
        /// a GET gets 405 (the spec's "no stream offered"), which parks the
        /// stream task without disturbing a POST-only test.
        get_requests: Arc<Mutex<Vec<RecordedRequest>>>,
        request_count: watch::Sender<usize>,
        get_responses: Arc<Mutex<VecDeque<CannedResponse>>>,
        accept_task: tokio::task::JoinHandle<()>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.accept_task.abort();
        }
    }

    impl TestServer {
        async fn start(responses: Vec<CannedResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let url = format!("http://{}/mcp", listener.local_addr().expect("addr"));
            let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::default();
            let responses: Arc<Mutex<VecDeque<CannedResponse>>> =
                Arc::new(Mutex::new(responses.into()));
            let routes: RouteMap = Arc::default();
            let get_requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::default();
            let (request_count, _) = watch::channel(0);
            let get_responses: Arc<Mutex<VecDeque<CannedResponse>>> = Arc::default();

            let req_log = Arc::clone(&requests);
            let resp_queue = Arc::clone(&responses);
            let route_map = Arc::clone(&routes);
            let get_req_log = Arc::clone(&get_requests);
            let get_resp_queue = Arc::clone(&get_responses);
            let request_count_for_task = request_count.clone();
            let accept_task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(handle_connection(
                        stream,
                        Arc::clone(&req_log),
                        Arc::clone(&resp_queue),
                        Arc::clone(&route_map),
                        Arc::clone(&get_req_log),
                        request_count_for_task.clone(),
                        Arc::clone(&get_resp_queue),
                    ));
                }
            });

            Self {
                url,
                requests,
                responses,
                routes,
                get_requests,
                request_count,
                get_responses,
                accept_task,
            }
        }

        /// The server's base URL (scheme://host:port), which doubles as the
        /// authorization-server issuer in the OAuth tests.
        fn base(&self) -> String {
            self.url.trim_end_matches("/mcp").to_string()
        }

        fn push_responses(&self, responses: Vec<CannedResponse>) {
            self.responses.lock().unwrap().extend(responses);
        }

        /// Serve `response` for every request to `path`.
        fn route(&self, path: &str, response: CannedResponse) {
            self.route_seq(path, vec![response]);
        }

        /// Serve `responses` in order for requests to `path`, replaying the
        /// last one indefinitely.
        fn route_seq(&self, path: &str, responses: Vec<CannedResponse>) {
            self.routes
                .paths
                .lock()
                .unwrap()
                .insert(path.to_string(), responses.into());
        }

        fn recorded(&self) -> Vec<(String, String)> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|r| (r.http_method().to_string(), r.rpc_method()))
                .collect()
        }

        /// Recorded requests whose path matches, as (method, body) pairs.
        fn recorded_for_path(&self, path: &str) -> Vec<(String, String)> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.path() == path)
                .map(|r| (r.http_method().to_string(), r.body.clone()))
                .collect()
        }

        /// Serve `responses` in order for server-initiated GET stream requests,
        /// replaying the last one indefinitely. Each is sent as the full body
        /// of one GET; the transport's stream task reconnects for the next.
        fn set_get_responses(&self, responses: Vec<CannedResponse>) {
            *self.get_responses.lock().unwrap() = responses.into();
        }

        async fn wait_for_requests(&self, count: usize) {
            let mut receiver = self.request_count.subscribe();
            tokio::time::timeout(Duration::from_secs(5), async {
                while *receiver.borrow_and_update() < count {
                    receiver.changed().await.expect("test server still running");
                }
            })
            .await
            .expect("expected request count reached");
        }

        /// The GET stream requests recorded so far.
        fn get_recorded(&self) -> Vec<RecordedRequest> {
            std::mem::take(&mut *self.get_requests.lock().unwrap())
        }
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Pick the canned response for one parsed request and log it on the right
    /// channel. A GET to the MCP endpoint -- whatever its path -- is the
    /// server-initiated stream (REQ-MCP-006), answered from its own queue
    /// (default 405) so it never touches the POST queue, routes, or request
    /// log; explicitly routed OAuth metadata stays on the ordinary request log.
    #[allow(clippy::too_many_arguments)]
    fn select_response(
        method: &str,
        path: &str,
        recorded: RecordedRequest,
        requests: &Mutex<Vec<RecordedRequest>>,
        responses: &Mutex<VecDeque<CannedResponse>>,
        routes: &RouteMap,
        get_requests: &Mutex<Vec<RecordedRequest>>,
        request_count: &watch::Sender<usize>,
        get_responses: &Mutex<VecDeque<CannedResponse>>,
    ) -> CannedResponse {
        request_count.send_modify(|count| *count += 1);
        if method == "GET"
            && !path.starts_with("/.well-known")
            && !routes.paths.lock().unwrap().contains_key(path)
        {
            get_requests.lock().unwrap().push(recorded);
            let mut queue = get_responses.lock().unwrap();
            let picked = if queue.len() > 1 {
                queue.pop_front()
            } else {
                queue.front().cloned()
            };
            return picked.unwrap_or_else(|| status_response(405, &[]));
        }
        let rejected_delete = method == "DELETE"
            && routes
                .delete_bearer
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|expected| {
                    recorded.header("authorization") != Some(expected.as_str())
                });
        requests.lock().unwrap().push(recorded);
        if rejected_delete {
            return status_response(401, &[]);
        }
        let path_response = {
            let mut routes = routes.paths.lock().unwrap();
            match routes.get_mut(path) {
                Some(queue) if queue.len() > 1 => queue.pop_front(),
                Some(queue) => queue.front().cloned(),
                None => None,
            }
        };
        path_response.unwrap_or_else(|| {
            responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(CannedResponse {
                    status: 500,
                    headers: Vec::new(),
                    body: String::new(),
                    delay_ms: 0,
                    echo_result: None,
                })
        })
    }

    async fn handle_connection(
        mut stream: TcpStream,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        responses: Arc<Mutex<VecDeque<CannedResponse>>>,
        routes: RouteMap,
        get_requests: Arc<Mutex<Vec<RecordedRequest>>>,
        request_count: watch::Sender<usize>,
        get_responses: Arc<Mutex<VecDeque<CannedResponse>>>,
    ) {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let head_end = loop {
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    break pos;
                }
                let mut chunk = [0u8; 4096];
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            };

            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut lines = head.lines();
            let request_line = lines.next().unwrap_or_default().to_string();
            let mut headers = HashMap::new();
            for line in lines {
                if let Some((key, value)) = line.split_once(':') {
                    headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let content_length: usize = headers
                .get("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let body_start = head_end + 4;
            while buf.len() < body_start + content_length {
                let mut chunk = [0u8; 4096];
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let body =
                String::from_utf8_lossy(&buf[body_start..body_start + content_length]).to_string();
            buf.drain(..body_start + content_length);

            let request_id = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("id").cloned());
            let method = request_line.split(' ').next().unwrap_or("").to_string();
            let path = request_line
                .split(' ')
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("")
                .to_string();
            let recorded = RecordedRequest {
                request_line,
                headers,
                body,
            };

            let initializing = recorded.rpc_method() == "initialize";
            let response = select_response(
                &method,
                &path,
                recorded,
                &requests,
                &responses,
                &routes,
                &get_requests,
                &request_count,
                &get_responses,
            );
            let gate = if path == "/token" {
                routes.token_gate.lock().unwrap().take()
            } else if initializing {
                routes.initialize_gate.lock().unwrap().take()
            } else {
                None
            };
            if let Some((started, release)) = gate {
                started.send(()).unwrap();
                release.acquire().await.unwrap().forget();
            }
            if response.delay_ms > 0 {
                // test-timing-allow: scripted HTTP response latency drives recovery/timeout behavior
                tokio::time::sleep(Duration::from_millis(response.delay_ms)).await;
            }
            let response_body = match &response.echo_result {
                Some(result) => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id.unwrap_or(Value::Null),
                    "result": result,
                })
                .to_string(),
                None => response.body.clone(),
            };
            let mut out = format!("HTTP/1.1 {} Test\r\n", response.status);
            for (key, value) in &response.headers {
                let _ = write!(out, "{key}: {value}\r\n");
            }
            let _ = write!(out, "content-length: {}\r\n\r\n", response_body.len());
            out.push_str(&response_body);
            if stream.write_all(out.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    fn http_config(url: &str, auth: HttpAuth) -> McpServerConfig {
        McpServerConfig::Http {
            url: url.to_string(),
            headers: HashMap::from([("x-org".to_string(), "acme".to_string())]),
            auth,
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        }
    }

    /// The canned handshake triple: initialize (with a session id), the
    /// notification ack, and a one-tool tools/list.
    fn handshake_responses(session_id: &str) -> Vec<CannedResponse> {
        vec![
            json_response(
                1,
                &serde_json::json!({"protocolVersion": "2025-03-26", "capabilities": {}}),
                &[("mcp-session-id", session_id)],
            ),
            accepted(),
            json_response(
                2,
                &serde_json::json!({"tools": [
                    {"name": "report", "description": "d", "inputSchema": {"type": "object"}}
                ]}),
                &[],
            ),
        ]
    }

    async fn connect_http(server: &TestServer, auth: HttpAuth) -> Result<crate::McpServer, String> {
        McpClientManager::connect_one(
            "remote",
            &http_config(&server.url, auth),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::default(),
            crate::OAuthHandshakeAction::Refresh,
        )
        .await
        .map_err(|failure| failure.message)
    }

    #[tokio::test]
    async fn http_initialize_negotiates_session_and_protocol_version() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);

        // initialize: advertises both response framings and a Streamable
        // HTTP protocol revision, no session yet.
        assert_eq!(requests[0].rpc_method(), "initialize");
        assert_eq!(
            requests[0].header("accept"),
            Some("application/json, text/event-stream")
        );
        assert_eq!(requests[0].header("mcp-session-id"), None);
        assert_eq!(requests[0].header("x-org"), Some("acme"));
        let init_body: Value = serde_json::from_str(&requests[0].body).expect("json body");
        assert_eq!(
            init_body
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str),
            Some("2025-03-26"),
            "HTTP must not advertise the stdio-era HTTP+SSE revision"
        );

        // Every request after initialize echoes the session id and the
        // negotiated protocol version.
        for request in &requests[1..] {
            assert_eq!(request.header("mcp-session-id"), Some("sess-1"));
            assert_eq!(request.header("mcp-protocol-version"), Some("2025-03-26"));
        }
        assert_eq!(requests[1].rpc_method(), "notifications/initialized");
        assert_eq!(requests[2].rpc_method(), "tools/list");
    }

    #[tokio::test]
    async fn http_call_tool_parses_sse_response_and_forwards_notifications() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        // ids: initialize=1, tools/list=2, tools/call=3.
        let sse_body = concat!(
            ": keepalive\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":",
            "{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n\n",
        );
        server.push_responses(vec![sse_response(sse_body)]);

        let output = mcp
            .call_tool("report", serde_json::json!({}), &CancellationToken::new())
            .await
            .expect("tools/call over SSE");

        assert_eq!(output, "hi");
        assert!(
            mcp.tools_changed.load(std::sync::atomic::Ordering::Acquire),
            "the in-stream list_changed notification must reach the protocol layer"
        );
    }

    #[tokio::test]
    async fn http_static_bearer_is_attached_to_every_request() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        connect_http(
            &server,
            HttpAuth::Static(StaticCred::Bearer("tok".to_string())),
        )
        .await
        .expect("connect");

        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests.iter() {
            assert_eq!(request.header("authorization"), Some("Bearer tok"));
        }
    }

    #[tokio::test]
    async fn http_session_expired_404_reinitializes_and_retries() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // The call 404s (session expired), recovery re-runs the handshake
        // with a fresh session, then the retried call succeeds.
        server.push_responses(vec![status_response(404, &[]), delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        server.push_responses(vec![json_response(
            3,
            &serde_json::json!({"content": [{"type": "text", "text": "ok"}]}),
            &[],
        )]);

        let output = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .expect("retried call");
        assert_eq!(output, "ok");

        let recorded = server.recorded();
        let methods: Vec<&str> = recorded.iter().map(|(_, rpc)| rpc.as_str()).collect();
        assert_eq!(
            methods,
            vec![
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
                "", // DELETE ending the expired session
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
            ]
        );
        assert_eq!(recorded[4].0, "DELETE");

        // The retried call rides the fresh session, not the expired one.
        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.header("mcp-session-id"), Some("sess-2"));
    }

    #[tokio::test]
    async fn http_session_expiry_during_tool_refresh_reestablishes() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        mcp.tools_changed
            .store(true, std::sync::atomic::Ordering::Release);
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // The lazy list_changed refresh 404s (session expired); the refresh
        // path must re-establish (fresh handshake) instead of leaving the
        // server with a stale tool list.
        server.push_responses(vec![status_response(404, &[]), delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));

        let defs = manager.tool_definitions().await;
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].0, "remote");
        assert_eq!(defs[0].1.name, "report");

        let recorded = server.recorded();
        let methods: Vec<&str> = recorded.iter().map(|(_, rpc)| rpc.as_str()).collect();
        assert_eq!(
            methods,
            vec![
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/list", // the refresh that 404s
                "",           // DELETE ending the expired session
                "initialize",
                "notifications/initialized",
                "tools/list",
            ]
        );
        assert_eq!(recorded[4].0, "DELETE");
    }

    #[tokio::test]
    async fn http_aborted_connect_deletes_the_created_session() {
        // initialize succeeds and creates a session, but the first
        // tools/list fails: the connect must end the session with a DELETE
        // instead of leaking it server-side until expiry.
        let server = TestServer::start(vec![
            json_response(
                1,
                &serde_json::json!({"protocolVersion": "2025-03-26", "capabilities": {}}),
                &[("mcp-session-id", "sess-1")],
            ),
            accepted(),
            status_response(500, &[]),
            status_response(200, &[]), // DELETE ack
        ])
        .await;

        let err = connect_http(&server, HttpAuth::None)
            .await
            .expect_err("handshake must fail");
        assert!(err.contains("HTTP 500"), "got: {err}");

        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.http_method(), "DELETE");
        assert_eq!(last.header("mcp-session-id"), Some("sess-1"));
    }

    #[tokio::test]
    async fn http_call_during_refresh_recovery_joins_the_claim() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        mcp.tools_changed
            .store(true, std::sync::atomic::Ordering::Release);
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // The list_changed refresh 404s and escalates into a recovery whose
        // re-initialize is held open for 300ms. A tool call STARTING at
        // 100ms -- while the refresh holds the server out of the map -- must
        // wait on the parked claim and succeed, not fail "not connected".
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![status_response(404, &[]), delete_ack()]);
        let mut recovery = handshake_responses("sess-2");
        recovery[0] = delayed(recovery[0].clone(), 300);
        server.push_responses(recovery);
        server.push_responses(vec![echo_id_response(&call_result)]);

        let (defs, call) = tokio::join!(manager.tool_definitions(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            manager
                .call_tool("remote", "report", serde_json::json!({}))
                .await
        });

        assert_eq!(defs.len(), 1, "refresh recovery must serve fresh defs");
        assert_eq!(call.expect("call joins the refresh hold"), "ok");
    }

    #[test]
    fn transport_managed_config_headers_are_rejected() {
        let generic = HttpTransport::connect(
            "s",
            "http://127.0.0.1:1/mcp",
            &HashMap::from([("Mcp-Session-Id".to_string(), "boo".to_string())]),
            &HttpAuth::None,
            Arc::default(),
            discard_sink(),
        );
        let err = generic.err().expect("generic header must be rejected");
        assert!(err.contains("transport-managed"), "got: {err}");

        let auth = HttpTransport::connect(
            "s",
            "http://127.0.0.1:1/mcp",
            &HashMap::new(),
            &HttpAuth::Static(StaticCred::Headers(HashMap::from([(
                "MCP-Protocol-Version".to_string(),
                "boo".to_string(),
            )]))),
            Arc::default(),
            discard_sink(),
        );
        let err = auth.err().expect("auth header must be rejected");
        assert!(err.contains("transport-managed"), "got: {err}");

        // Last-Event-ID is the GET stream's runtime resume cursor; a config
        // copy would ride the initial open and duplicate on reconnect.
        let resume = HttpTransport::connect(
            "s",
            "http://127.0.0.1:1/mcp",
            &HashMap::from([("Last-Event-ID".to_string(), "42".to_string())]),
            &HttpAuth::None,
            Arc::default(),
            discard_sink(),
        );
        let err = resume.err().expect("Last-Event-ID must be rejected");
        assert!(err.contains("transport-managed"), "got: {err}");

        // Authorization is a credential: as a generic header it would dodge
        // the auth classification and collide with an OAuth bearer. The SAME
        // key under auth.headers is the explicit static-credential form and
        // stays accepted.
        let smuggled = HttpTransport::connect(
            "s",
            "http://127.0.0.1:1/mcp",
            &HashMap::from([("Authorization".to_string(), "Bearer tok".to_string())]),
            &HttpAuth::None,
            Arc::default(),
            discard_sink(),
        );
        let Err(err) = smuggled else {
            panic!("generic Authorization must be rejected");
        };
        assert!(err.contains("auth.bearer or auth.headers"), "got: {err}");

        let explicit = HttpTransport::connect(
            "s",
            "http://127.0.0.1:1/mcp",
            &HashMap::new(),
            &HttpAuth::Static(StaticCred::Headers(HashMap::from([(
                "Authorization".to_string(),
                "Bearer tok".to_string(),
            )]))),
            Arc::default(),
            discard_sink(),
        );
        assert!(explicit.is_ok(), "auth.headers Authorization stays valid");
    }

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<Value>>);

    impl ServerMessageSink for RecordingSink {
        fn on_message(&self, message: Value) {
            self.0.lock().unwrap().push(message);
        }
    }

    /// A throwaway sink for transports whose server-initiated stream the test
    /// does not observe.
    fn discard_sink() -> Arc<dyn ServerMessageSink> {
        Arc::new(RecordingSink::default())
    }

    #[tokio::test]
    async fn http_server_request_with_colliding_id_goes_to_the_sink() {
        // The server-initiated `ping` reuses id 1 -- the same id as our
        // first request. It must reach the sink, not be parsed as a
        // result-less reply that aborts the call.
        let server = TestServer::start(vec![sse_response(concat!(
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
        ))])
        .await;
        let transport = HttpTransport::connect(
            "remote",
            &server.url,
            &HashMap::new(),
            &HttpAuth::None,
            Arc::default(),
            discard_sink(),
        )
        .expect("connect");

        let sink = RecordingSink::default();
        let result = transport
            .request(
                "tools/call",
                serde_json::json!({}),
                Duration::from_secs(5),
                &sink,
            )
            .await
            .expect("the real reply must still be correlated");

        assert_eq!(result, serde_json::json!({"ok": true}));
        let messages = sink.0.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].get("method").and_then(Value::as_str),
            Some("ping")
        );
    }

    #[tokio::test]
    async fn server_initiated_stream_delivers_list_changed_and_resumes() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        // The GET stream serves one list_changed carrying an event id, then
        // closes; the reconnect is answered 405 to park the task. The
        // responses must be queued before connect, since the stream opens as
        // soon as `initialize` returns.
        server.set_get_responses(vec![
            sse_response(concat!(
                "id: 7\n",
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
            )),
            status_response(405, &[]),
        ]);

        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        // The second GET proves the first stream response was consumed and its
        // event cursor triggered a resume.
        server.wait_for_requests(5).await;
        assert!(mcp.tools_changed.load(Ordering::Acquire));
        let gets = server.get_recorded();
        assert_eq!(gets.len(), 2, "open then resume");
        assert_eq!(gets[0].http_method(), "GET");
        assert_eq!(gets[0].header("accept"), Some("text/event-stream"));
        assert_eq!(gets[0].header("mcp-session-id"), Some("sess-1"));
        assert_eq!(gets[0].header("mcp-protocol-version"), Some("2025-03-26"));
        // A generic config header rides the GET like any other request.
        assert_eq!(gets[0].header("x-org"), Some("acme"));
        assert_eq!(
            gets[0].header("last-event-id"),
            None,
            "the first open does not resume"
        );
        // The reconnect resumes past the last delivered event (REQ-MCP-006).
        assert_eq!(gets[1].header("last-event-id"), Some("7"));
    }

    #[tokio::test]
    async fn server_initiated_stream_stops_on_non_sse_response() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        // A 200 application/json health page is not a stream. The task must
        // stop rather than frame it as empty and reconnect forever.
        server.set_get_responses(vec![json_response(
            0,
            &serde_json::json!({"status": "ok"}),
            &[],
        )]);
        let _mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        server.wait_for_requests(4).await;
        // Past one local backoff interval a buggy poll loop would have issued a
        // second GET; a stopped stream stays at one.
        // test-timing-allow: crossing the retry interval proves a terminal response stops polling
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(
            server.get_requests.lock().unwrap().len(),
            1,
            "a non-SSE 2xx response must stop the stream, not poll"
        );
    }

    #[tokio::test]
    async fn server_initiated_stream_404_flags_session_reset() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        // The session-bearing GET stream observes the session expired (404). It
        // must flag the server so the next definitions read re-establishes
        // (REQ-MCP-005), not leave it ready on a dead session.
        server.set_get_responses(vec![status_response(404, &[])]);
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        server.wait_for_requests(4).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !mcp.tools_changed.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("session-reset flag set from a GET-stream 404");

        // The GET carried the negotiated session, and the stream stopped rather
        // than re-GETting a dead session in a loop.
        // test-timing-allow: crossing the reconnect interval proves a dead session is not retried
        tokio::time::sleep(Duration::from_millis(300)).await;
        let gets = server.get_recorded();
        assert_eq!(gets.len(), 1, "a dead session must not be re-GETted");
        assert_eq!(gets[0].header("mcp-session-id"), Some("sess-1"));
    }

    #[tokio::test]
    async fn server_initiated_stream_handles_batched_notifications() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        // A JSON-RPC batch (array) on the stream carrying list_changed must
        // flip tools_changed, not be dropped as a non-object payload.
        server.set_get_responses(vec![
            sse_response(
                "data: [{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}]\n\n",
            ),
            status_response(405, &[]),
        ]);
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        server.wait_for_requests(5).await;
        assert!(mcp.tools_changed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn server_initiated_stream_carries_the_oauth_bearer() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        server.set_get_responses(vec![status_response(405, &[])]);
        let manager = Arc::new(McpClientManager::new());
        // A stored, unexpired token resumes the connection silently and seeds
        // the bearer the GET stream must also carry (REQ-MCP-012).
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-live",
                Some("rt-1"),
                &["read"],
                far_future(),
            ))
            .await
            .unwrap();
        let _mcp = connect_http_managed(
            &manager,
            &server,
            HttpAuth::OAuth(crate::OAuthConfig::default()),
        )
        .await
        .expect("connect");

        server.wait_for_requests(4).await;
        let gets = server.get_recorded();
        assert_eq!(gets[0].header("authorization"), Some("Bearer at-live"));
    }

    #[tokio::test]
    async fn http_stale_error_does_not_tear_down_a_fresh_connection() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // Two concurrent calls 404. One failure is held open for 400ms; the
        // other fails instantly and completes its recovery (~ms) long before
        // the held one lands. The stale failure must observe the fresh
        // generation and just retry -- exactly one recovery handshake total,
        // and no teardown of the newly established session.
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![
            delayed(status_response(404, &[]), 400),
            status_response(404, &[]),
            delete_ack(),
        ]);
        server.push_responses(handshake_responses("sess-2"));
        server.push_responses(vec![
            echo_id_response(&call_result),
            echo_id_response(&call_result),
        ]);

        let (first, second) = tokio::join!(
            manager.call_tool("remote", "report", serde_json::json!({})),
            manager.call_tool("remote", "report", serde_json::json!({})),
        );

        assert_eq!(first.expect("call must succeed"), "ok");
        assert_eq!(second.expect("call must succeed"), "ok");

        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(
            initializes, 2,
            "connect + one recovery; the stale error must not re-reconnect"
        );
    }

    #[tokio::test]
    async fn http_session_expiry_during_first_tools_list_retries_handshake() {
        // The server issues a session at initialize but loses it before the
        // first tools/list: one fresh-connection retry must connect the
        // server instead of skipping it (REQ-MCP-005).
        let server = TestServer::start(vec![
            json_response(
                1,
                &serde_json::json!({"protocolVersion": "2025-03-26", "capabilities": {}}),
                &[("mcp-session-id", "sess-1")],
            ),
            accepted(),
            status_response(404, &[]), // tools/list: session gone
            delete_ack(),              // dead session's terminate
        ])
        .await;
        server.push_responses(handshake_responses("sess-2"));

        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("handshake must retry once and connect");
        assert_eq!(mcp.tools().len(), 1);

        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(initializes, 2, "exactly one fresh-connection retry");
    }

    #[tokio::test]
    async fn http_failed_refresh_recovery_drops_the_server() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        mcp.tools_changed
            .store(true, std::sync::atomic::Ordering::Release);
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // The refresh 404s (recoverable), but the recovery handshake fails:
        // the server must be dropped, not reinserted with stale definitions
        // over a torn-down transport.
        server.push_responses(vec![
            status_response(404, &[]),
            delete_ack(),
            status_response(500, &[]),
        ]);

        let defs = manager.tool_definitions().await;
        assert!(defs.is_empty(), "stale definitions must not be advertised");
        // Dropped from the connected map, but retained in status as failed with
        // its cause rather than vanishing (REQ-MCP-018).
        let status = manager.status().await;
        assert_eq!(status.len(), 1);
        assert!(matches!(status[0].state, McpConnState::Failed));
        assert!(status[0].last_error.is_some());
    }

    #[tokio::test]
    async fn http_call_arriving_mid_recovery_joins_it() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // The first call 404s and leads a recovery whose re-initialize is
        // held open for 300ms. The second call STARTS at 100ms -- while the
        // server is out of the map -- and must join the in-flight recovery
        // at the initial lookup instead of failing with "not connected".
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![status_response(404, &[]), delete_ack()]);
        let mut recovery = handshake_responses("sess-2");
        recovery[0] = delayed(recovery[0].clone(), 300);
        server.push_responses(recovery);
        server.push_responses(vec![
            echo_id_response(&call_result),
            echo_id_response(&call_result),
        ]);

        let (first, second) = tokio::join!(
            manager.call_tool("remote", "report", serde_json::json!({})),
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                manager
                    .call_tool("remote", "report", serde_json::json!({}))
                    .await
            },
        );

        assert_eq!(first.expect("leading call recovers"), "ok");
        assert_eq!(second.expect("late call joins the recovery"), "ok");

        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(initializes, 2, "connect + one shared recovery");
    }

    #[tokio::test]
    async fn http_concurrent_expired_calls_share_one_recovery() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // Two concurrent calls both 404. The delays order the race: the first
        // failure claims the recovery immediately; the second failure lands
        // (100ms) while the leader's re-initialize is still pending (300ms),
        // so it must join the in-flight recovery rather than failing with
        // "not connected". Retried call ids depend on scheduling order, so
        // those responses echo the request id.
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![
            status_response(404, &[]),
            delayed(status_response(404, &[]), 100),
            delete_ack(),
        ]);
        let mut recovery = handshake_responses("sess-2");
        recovery[0] = delayed(recovery[0].clone(), 300);
        server.push_responses(recovery);
        server.push_responses(vec![
            echo_id_response(&call_result),
            echo_id_response(&call_result),
        ]);

        let (first, second) = tokio::join!(
            manager.call_tool("remote", "report", serde_json::json!({})),
            manager.call_tool("remote", "report", serde_json::json!({})),
        );

        assert_eq!(first.expect("first call recovers"), "ok");
        assert_eq!(second.expect("second call joins the recovery"), "ok");

        // Exactly one recovery handshake ran for the two failing calls.
        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(initializes, 2, "connect + one shared recovery");
        assert!(matches!(
            manager
                .servers
                .read()
                .await
                .get("remote")
                .expect("supervisor retained")
                .snapshot()
                .state,
            crate::supervisor::SupervisorState::Ready(_)
        ));
    }

    #[tokio::test]
    async fn http_unauthorized_401_is_surfaced_not_retried() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        server.push_responses(vec![status_response(
            401,
            &[("www-authenticate", "Bearer realm=\"mcp\"")],
        )]);

        let err = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .expect_err("401 must surface");
        assert!(err.contains("unauthorized (HTTP 401)"), "got: {err}");

        // Exactly one tools/call went out -- no blind retry of an auth failure.
        let calls = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "tools/call")
            .count();
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn http_shutdown_deletes_the_session() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        server.push_responses(vec![status_response(200, &[])]);
        mcp.terminate().await.expect("shutdown succeeds");

        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.http_method(), "DELETE");
        assert_eq!(last.header("mcp-session-id"), Some("sess-1"));
        assert_eq!(
            last.header("mcp-protocol-version"),
            Some("2025-03-26"),
            "the negotiated version rides the session DELETE too"
        );
    }

    #[tokio::test]
    async fn http_shutdown_retains_session_for_retry_until_delete_succeeds() {
        let server = TestServer::start(handshake_responses("sess-retry")).await;
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        server.push_responses(vec![status_response(503, &[]), status_response(200, &[])]);

        let first = mcp.terminate().await.expect_err("DELETE failure surfaces");
        assert!(first.to_string().contains("HTTP 503"), "{first}");
        mcp.terminate().await.expect("same session retries");
        mcp.terminate()
            .await
            .expect("successful shutdown is idempotent");

        let deletes: Vec<_> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.http_method() == "DELETE")
            .map(|request| request.header("mcp-session-id").map(str::to_string))
            .collect();
        assert_eq!(
            deletes,
            vec![
                Some("sess-retry".to_string()),
                Some("sess-retry".to_string())
            ],
            "the failed DELETE retains the session ID and success clears it"
        );
    }

    #[tokio::test]
    async fn http_shutdown_retry_accepts_session_already_gone() {
        let server = TestServer::start(handshake_responses("sess-gone")).await;
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        server.push_responses(vec![status_response(503, &[]), status_response(404, &[])]);

        mcp.terminate()
            .await
            .expect_err("ambiguous first DELETE remains retryable");
        mcp.terminate()
            .await
            .expect("404 confirms the retained session is already gone");
        mcp.terminate().await.expect("session ID was cleared");

        let deletes = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.http_method() == "DELETE")
            .count();
        assert_eq!(deletes, 2);
    }

    struct NullSink;

    impl ServerMessageSink for NullSink {
        fn on_message(&self, _message: Value) {}
    }

    #[tokio::test]
    async fn http_concurrent_404s_both_classify_as_session_expired() {
        let server =
            TestServer::start(vec![status_response(404, &[]), status_response(404, &[])]).await;
        let transport = HttpTransport::connect(
            "remote",
            &server.url,
            &HashMap::new(),
            &HttpAuth::None,
            Arc::default(),
            discard_sink(),
        )
        .expect("connect");
        *transport.session_id.lock().unwrap() = Some("sess-1".to_string());

        // Both in-flight requests carried the expired session id; the first
        // 404 clearing the shared state must not demote the second to a
        // generic protocol error.
        let (first, second) = tokio::join!(
            transport.request(
                "tools/call",
                serde_json::json!({}),
                Duration::from_secs(5),
                &NullSink,
            ),
            transport.request(
                "tools/call",
                serde_json::json!({}),
                Duration::from_secs(5),
                &NullSink,
            ),
        );

        assert_eq!(
            first.expect_err("404 must fail"),
            TransportError::SessionExpired
        );
        assert_eq!(
            second.expect_err("404 must fail"),
            TransportError::SessionExpired
        );
        // The expired id stays visible on the doomed transport so calls
        // racing in before recovery still classify as SessionExpired;
        // recovery replaces the transport wholesale.
        assert_eq!(
            *transport.session_id.lock().unwrap(),
            Some("sess-1".to_string())
        );
    }

    #[test]
    fn sse_framer_yields_event_per_blank_line() {
        let mut framer = SseFramer::default();
        let events = framer.push(b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(events, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn sse_framer_handles_split_chunks_and_crlf() {
        let mut framer = SseFramer::default();
        assert!(framer.push(b"event: message\r\ndata: {\"a\"").is_empty());
        let events = framer.push(b":1}\r\n\r\n");
        assert_eq!(events, vec!["{\"a\":1}"]);
    }

    #[test]
    fn sse_framer_joins_multiline_data_and_flushes_on_finish() {
        let mut framer = SseFramer::default();
        assert!(framer.push(b"data: line1\ndata: line2\n").is_empty());
        assert_eq!(framer.finish(), Some("line1\nline2".to_string()));
        assert!(framer.finish().is_none());
    }

    #[test]
    fn sse_framer_flushes_an_unterminated_final_data_line() {
        // A server may close the response right after the last data line,
        // with no trailing newline; EOF must act as the terminator.
        let mut framer = SseFramer::default();
        assert!(framer.push(b"data: {\"a\":1}").is_empty());
        assert_eq!(framer.finish(), Some("{\"a\":1}".to_string()));
        assert!(framer.finish().is_none());
    }

    #[test]
    fn sse_framer_tracks_id_and_retry_and_ignores_comments() {
        // `id:` sets the persistent last event id buffer and `retry:` the
        // reconnect hint (REQ-MCP-006); comments are ignored.
        let mut framer = SseFramer::default();
        let events = framer.push(b": keepalive\nid: 7\nretry: 100\ndata: x\n\n");
        assert_eq!(events, vec!["x"]);
        assert_eq!(framer.last_id.as_deref(), Some("7"));
        assert_eq!(framer.retry, Some(Duration::from_millis(100)));
        // The id persists across later events until the server changes it.
        framer.push(b"data: y\n\n");
        assert_eq!(framer.last_id.as_deref(), Some("7"));
    }

    #[test]
    fn sse_framer_empty_or_bare_id_resets_the_cursor() {
        // An empty or bare `id` resets the last-event-id buffer (SSE spec), so
        // the next reconnect omits Last-Event-ID rather than resuming stale.
        let mut framer = SseFramer::default();
        framer.push(b"id: 7\ndata: x\n\n");
        assert_eq!(framer.last_id.as_deref(), Some("7"));
        framer.push(b"id:\ndata: y\n\n");
        assert_eq!(framer.last_id, None, "empty id: resets");
        framer.push(b"id: 9\ndata: z\n\n");
        assert_eq!(framer.last_id.as_deref(), Some("9"));
        framer.push(b"id\ndata: w\n\n");
        assert_eq!(framer.last_id, None, "bare id resets");
    }

    #[test]
    fn sse_framer_ignores_non_integer_retry() {
        let mut framer = SseFramer::default();
        framer.push(b"retry: soon\ndata: x\n\n");
        assert_eq!(framer.retry, None);
    }

    // -----------------------------------------------------------------------
    // OAuth 2.1 lifecycle (REQ-MCP-009..013) against the scripted server.
    // The TestServer doubles as the protected resource (/mcp), its RFC 9728
    // metadata, the authorization server's RFC 8414 metadata, and the
    // registration + token endpoints, via path routing.
    // -----------------------------------------------------------------------

    use crate::oauth::{self, OAuthRegistrationRecord, OAuthTokenRecord};
    use base64::Engine as _;
    use sha2::Digest as _;

    const REDIRECT_BASE: &str = "http://localhost:7777";
    const CALLBACK: &str = "http://localhost:7777/api/mcp/oauth/callback";

    fn json_doc(value: &Value) -> CannedResponse {
        CannedResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: value.to_string(),
            delay_ms: 0,
            echo_result: None,
        }
    }

    /// A 401 whose Bearer challenge arrives SECOND, behind a Basic one --
    /// the multi-challenge shape real gateways produce. Every flow driven
    /// through this helper proves the Bearer challenge is selected rather
    /// than whichever header is first.
    fn unauthorized(server: &TestServer) -> CannedResponse {
        status_response(
            401,
            &[
                ("www-authenticate", "Basic realm=\"legacy\""),
                (
                    "www-authenticate",
                    &format!(
                        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
                        server.base()
                    ),
                ),
            ],
        )
    }

    /// Wire up the discovery documents: PRM naming the server itself as the
    /// authorization server, and AS metadata with PKCE + iss support.
    fn install_oauth_discovery(server: &TestServer, with_registration_endpoint: bool) {
        let base = server.base();
        server.route(
            "/.well-known/oauth-protected-resource/mcp",
            json_doc(&serde_json::json!({
                "resource": server.url,
                "authorization_servers": [base],
                "scopes_supported": ["mcp.read"],
            })),
        );
        let mut metadata = serde_json::json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "code_challenge_methods_supported": ["S256"],
            "authorization_response_iss_parameter_supported": true,
        });
        if with_registration_endpoint {
            metadata["registration_endpoint"] = Value::String(format!("{base}/register"));
        }
        server.route(
            "/.well-known/oauth-authorization-server",
            json_doc(&metadata),
        );
    }

    fn token_response(access: &str, refresh: Option<&str>, scope: Option<&str>) -> CannedResponse {
        let mut body = serde_json::json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        if let Some(refresh) = refresh {
            body["refresh_token"] = Value::String(refresh.to_string());
        }
        if let Some(scope) = scope {
            body["scope"] = Value::String(scope.to_string());
        }
        json_doc(&body)
    }

    /// A throwaway public-client registration (`cid-1`, no recorded
    /// `redirect_uri`) for seeding the OAuth store in tests.
    fn none_registration(auth_server: &str) -> OAuthRegistrationRecord {
        OAuthRegistrationRecord {
            auth_server: auth_server.to_string(),
            client_id: "cid-1".to_string(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_string(),
            redirect_uri: None,
        }
    }

    fn stored_token(
        server: &TestServer,
        access: &str,
        refresh: Option<&str>,
        scopes: &[&str],
        expires_at: i64,
    ) -> OAuthTokenRecord {
        OAuthTokenRecord {
            server_name: "remote".to_string(),
            resource: oauth::canonical_resource(&server.url),
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
            access_token: access.to_string(),
            refresh_token: refresh.map(str::to_string),
            expires_at,
        }
    }

    fn far_future() -> i64 {
        chrono::Utc::now().timestamp() + 3600
    }

    fn in_the_past() -> i64 {
        chrono::Utc::now().timestamp() - 3600
    }

    async fn connect_http_managed(
        manager: &McpClientManager,
        server: &TestServer,
        auth: HttpAuth,
    ) -> Result<crate::McpServer, String> {
        McpClientManager::connect_one(
            "remote",
            &http_config(&server.url, auth),
            Arc::clone(&manager.pending_oauth_urls),
            Arc::clone(&manager.oauth),
            crate::OAuthHandshakeAction::Refresh,
        )
        .await
        .map_err(|failure| failure.message)
    }

    async fn pending_auth_url(manager: &McpClientManager) -> Option<String> {
        if let Some(url) = manager
            .pending_oauth_urls
            .read()
            .await
            .get("remote")
            .cloned()
        {
            return Some(url);
        }
        manager
            .status()
            .await
            .into_iter()
            .find_map(|s| s.pending_oauth_url)
    }

    async fn next_pending_auth_url(
        manager: &McpClientManager,
        publications: &mut watch::Receiver<u64>,
    ) -> String {
        tokio::time::timeout(Duration::from_secs(10), publications.changed())
            .await
            .expect("OAuth URL published in time")
            .expect("OAuth runtime still active");
        pending_auth_url(manager)
            .await
            .expect("published OAuth URL")
    }

    fn query_params(url: &str) -> HashMap<String, String> {
        reqwest::Url::parse(url)
            .expect("valid url")
            .query_pairs()
            .into_owned()
            .collect()
    }

    /// Minimal x-www-form-urlencoded decoder for asserting token requests.
    fn parse_form(body: &str) -> HashMap<String, String> {
        fn decode(s: &str) -> String {
            let bytes = s.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'+' => out.push(b' '),
                    b'%' if i + 2 < bytes.len() => {
                        let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                        if let Ok(byte) = u8::from_str_radix(hex, 16) {
                            out.push(byte);
                            i += 2;
                        } else {
                            out.push(bytes[i]);
                        }
                    }
                    other => out.push(other),
                }
                i += 1;
            }
            String::from_utf8_lossy(&out).into_owned()
        }
        body.split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (decode(k), decode(v)))
            .collect()
    }

    #[tokio::test]
    // reason: end-to-end assertion of one ordered flow (discovery → DCR →
    // PKCE URL → exchange → authenticated reconnect); splitting would
    // duplicate the scripted-server setup at every stage.
    #[allow(clippy::too_many_lines)]
    async fn oauth_full_flow_discovers_registers_authorizes_and_connects() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route(
            "/register",
            json_doc(&serde_json::json!({
                "client_id": "cid-1",
                "token_endpoint_auth_method": "none",
            })),
        );
        server.route(
            "/token",
            token_response("at-1", Some("rt-1"), Some("mcp.read")),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());

        // The 401 starts discovery; the connect fails with the surfaced URL.
        let err = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("unauthorized connect must not publish");
        assert!(err.contains("requires OAuth authorization"), "got: {err}");

        // The authorization URL is structured state (REQ-MCP-013), carrying
        // PKCE, state, the resource indicator, and the registered client.
        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let params = query_params(&auth_url);
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(params.get("client_id").map(String::as_str), Some("cid-1"));
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some(CALLBACK)
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("resource").map(String::as_str),
            Some(oauth::canonical_resource(&server.url).as_str())
        );
        assert_eq!(params.get("scope").map(String::as_str), Some("mcp.read"));
        let state = params.get("state").expect("state nonce").clone();
        let challenge = params
            .get("code_challenge")
            .expect("pkce challenge")
            .clone();

        // The registration request carried our redirect and a public client.
        let registrations = server.recorded_for_path("/register");
        assert_eq!(registrations.len(), 1);
        let reg_body: Value = serde_json::from_str(&registrations[0].1).expect("json");
        assert_eq!(
            reg_body.get("redirect_uris"),
            Some(&serde_json::json!([CALLBACK]))
        );

        // Operator completes the browser round trip; the callback exchanges
        // the code and reconnects with the token on every request.
        server.push_responses(handshake_responses("sess-1"));
        let name = manager
            .complete_oauth_authorization(&state, "code-1", Some(&server.base()))
            .await
            .expect("authorization completes");
        assert_eq!(name, "remote");

        tokio::time::timeout(Duration::from_secs(10), manager.await_background_tasks())
            .await
            .expect("server connected in background");
        assert!(manager.status().await.iter().any(|s| s.tool_count == 1));

        // The token request was a PKCE code exchange bound to the resource.
        let token_requests = server.recorded_for_path("/token");
        assert_eq!(token_requests.len(), 1);
        let form = parse_form(&token_requests[0].1);
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
        assert_eq!(form.get("code").map(String::as_str), Some("code-1"));
        assert_eq!(form.get("client_id").map(String::as_str), Some("cid-1"));
        assert_eq!(form.get("redirect_uri").map(String::as_str), Some(CALLBACK));
        assert_eq!(
            form.get("resource").map(String::as_str),
            Some(oauth::canonical_resource(&server.url).as_str())
        );
        let verifier = form.get("code_verifier").expect("pkce verifier");
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(challenge, expected, "challenge must be S256(verifier)");

        // The bearer rides EVERY request, the reconnect initialize included
        // (REQ-MCP-012), and the URL is no longer pending.
        {
            let requests = server.requests.lock().unwrap();
            let last_init = requests
                .iter()
                .rfind(|r| r.rpc_method() == "initialize")
                .expect("reconnect initialize");
            assert_eq!(last_init.header("authorization"), Some("Bearer at-1"));
        }
        assert!(pending_auth_url(&manager).await.is_none());

        // The token and the AS-keyed registration persisted.
        let token = manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .expect("token persisted");
        assert_eq!(token.access_token, "at-1");
        assert_eq!(token.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(token.scopes, vec!["mcp.read"]);
        assert_eq!(token.resource, oauth::canonical_resource(&server.url));
        let registration = manager
            .oauth
            .store()
            .registration(&server.base())
            .await
            .unwrap()
            .expect("registration persisted");
        assert_eq!(registration.client_id, "cid-1");
    }

    #[tokio::test]
    async fn oauth_discovery_falls_back_to_same_origin_as_metadata_when_prm_missing() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![status_response(
            401,
            &[("www-authenticate", "Bearer realm=\"OAuth\"")],
        )]);
        let base = server.base();
        server.route(
            "/.well-known/oauth-protected-resource/mcp",
            status_response(404, &[]),
        );
        server.route(
            "/.well-known/oauth-protected-resource",
            status_response(404, &[]),
        );
        server.route(
            "/.well-known/oauth-authorization-server",
            json_doc(&serde_json::json!({
                // Atlassian serves this document from the MCP resource origin
                // while declaring a different issuer. Resource-advertised trust
                // must still accept the declared issuer as the registration key.
                "issuer": format!("{base}/issuer"),
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            })),
        );
        server.route(
            "/register",
            json_doc(&serde_json::json!({
                "client_id": "cid-1",
                "token_endpoint_auth_method": "none",
            })),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());

        let err = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("unauthorized connect must surface auth URL");
        assert!(err.contains("requires OAuth authorization"), "got: {err}");

        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let params = query_params(&auth_url);
        assert_eq!(params.get("client_id").map(String::as_str), Some("cid-1"));
        assert_eq!(
            params.get("resource").map(String::as_str),
            Some(oauth::canonical_resource(&server.url).as_str())
        );

        let registration = manager
            .oauth
            .store()
            .registration(&format!("{base}/issuer"))
            .await
            .unwrap()
            .expect("registration persisted under declared issuer");
        assert_eq!(registration.client_id, "cid-1");
    }

    #[tokio::test]
    async fn cached_registration_with_stale_redirect_is_reregistered() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route(
            "/register",
            json_doc(&serde_json::json!({
                "client_id": "cid-fresh",
                "token_endpoint_auth_method": "none",
            })),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());

        // A cached DCR registration whose recorded redirect_uri predates the
        // current canonical redirect base (REQ-MCP-020): the flow must
        // re-register rather than reuse a client the AS would reject on a
        // redirect mismatch (REQ-MCP-011).
        manager
            .oauth
            .store()
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: server.base(),
                client_id: "cid-stale".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
                redirect_uri: Some("http://stale.example/api/mcp/oauth/callback".to_string()),
            })
            .await
            .unwrap();

        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("unauthorized connect must not publish");

        // Re-registration happened with the current redirect, and the flow
        // uses the freshly registered client.
        let registrations = server.recorded_for_path("/register");
        assert_eq!(
            registrations.len(),
            1,
            "stale redirect forces a re-register"
        );
        let reg_body: Value = serde_json::from_str(&registrations[0].1).expect("json");
        assert_eq!(
            reg_body.get("redirect_uris"),
            Some(&serde_json::json!([CALLBACK]))
        );
        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        assert_eq!(
            query_params(&auth_url).get("client_id").map(String::as_str),
            Some("cid-fresh")
        );

        // The persisted registration now records the current redirect.
        let stored = manager
            .oauth
            .store()
            .registration(&server.base())
            .await
            .unwrap()
            .expect("registration");
        assert_eq!(stored.client_id, "cid-fresh");
        assert_eq!(stored.redirect_uri.as_deref(), Some(CALLBACK));
    }

    #[tokio::test]
    async fn cached_registration_with_matching_redirect_is_reused() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        // The AS advertises DCR, but no /register route exists: a re-register
        // attempt would fail loudly, proving reuse.
        install_oauth_discovery(&server, true);

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: server.base(),
                client_id: "cid-cached".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
                redirect_uri: Some(CALLBACK.to_string()),
            })
            .await
            .unwrap();

        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("unauthorized connect must not publish");

        assert!(
            server.recorded_for_path("/register").is_empty(),
            "a matching redirect reuses the cached client"
        );
        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        assert_eq!(
            query_params(&auth_url).get("client_id").map(String::as_str),
            Some("cid-cached")
        );
    }

    #[tokio::test]
    async fn static_auth_401_is_hard_failure_without_oauth_discovery() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());

        let err = connect_http_managed(
            &manager,
            &server,
            HttpAuth::Static(StaticCred::Bearer("bad".to_string())),
        )
        .await
        .expect_err("rejected static auth must fail");
        assert!(err.contains("unauthorized (HTTP 401)"), "got: {err}");

        // StaticAuthRejected: no discovery, no pending flow (REQ-MCP-008).
        assert!(server
            .recorded_for_path("/.well-known/oauth-protected-resource/mcp")
            .is_empty());
        assert!(pending_auth_url(&manager).await.is_none());
    }

    #[tokio::test]
    async fn stored_unexpired_token_restores_onto_first_initialize() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-9",
                Some("rt-9"),
                &["mcp.read"],
                far_future(),
            ))
            .await
            .unwrap();

        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("silent restore connects with no 401 round trip");
        assert_eq!(mcp.tools().len(), 1);

        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests.iter() {
            assert_eq!(request.header("authorization"), Some("Bearer at-9"));
        }
    }

    #[tokio::test]
    async fn repointed_url_discards_stored_token_instead_of_sending_it() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mut token = stored_token(&server, "at-9", None, &[], far_future());
        token.resource = "https://elsewhere.example/mcp".to_string();
        manager.oauth.store().upsert_token(&token).await.unwrap();

        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("connect (unauthenticated)");

        // The token bound to the old resource was neither sent nor kept.
        {
            let requests = server.requests.lock().unwrap();
            for request in requests.iter() {
                assert_eq!(request.header("authorization"), None);
            }
        }
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn expired_stored_token_refreshes_silently_on_first_401() {
        // The restored-but-expired bearer rides the first initialize, the
        // server 401s, and the refresh path reconnects -- no re-prompt
        // (REQ-MCP-012).
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(handshake_responses("sess-2"));

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&server.base()))
            .await
            .unwrap();
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-old",
                Some("rt-1"),
                &["mcp.read"],
                in_the_past(),
            ))
            .await
            .unwrap();

        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("silent refresh must connect without a prompt");
        assert_eq!(mcp.tools().len(), 1);
        assert!(pending_auth_url(&manager).await.is_none());

        // The refresh grant carried the resource indicator and rotated both
        // halves, persisted (REQ-MCP-012).
        let form = parse_form(&server.recorded_for_path("/token")[0].1);
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        assert_eq!(form.get("refresh_token").map(String::as_str), Some("rt-1"));
        assert_eq!(
            form.get("resource").map(String::as_str),
            Some(oauth::canonical_resource(&server.url).as_str())
        );
        let token = manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.access_token, "at-2");
        assert_eq!(token.refresh_token.as_deref(), Some("rt-2"));

        // The post-refresh initialize carried the new bearer.
        let requests = server.requests.lock().unwrap();
        let last_init = requests
            .iter()
            .rfind(|r| r.rpc_method() == "initialize")
            .expect("post-refresh initialize");
        assert_eq!(last_init.header("authorization"), Some("Bearer at-2"));
    }

    #[tokio::test]
    async fn tool_call_401_refreshes_and_replays_the_call() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&server.base()))
            .await
            .unwrap();
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-1"),
                &["mcp.read"],
                far_future(),
            ))
            .await
            .unwrap();
        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // The call 401s (revoked/expired server-side); the silent refresh
        // rotates the bearer and the executor replays the call
        // (TokenRefreshNeeded -> TokenRefreshed -> retry).
        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-2".to_string());
        install_oauth_discovery(&server, true);
        server.route("/token", token_response("at-2", None, None));
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![unauthorized(&server), delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        server.push_responses(vec![echo_id_response(&call_result)]);

        let output = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .expect("call replays after silent refresh");
        assert_eq!(output, "ok");

        // Exactly two tools/call attempts; the replay carries the rotated
        // bearer on the still-live session.
        {
            let requests = server.requests.lock().unwrap();
            let deletion = requests
                .iter()
                .position(|r| r.http_method() == "DELETE")
                .expect("old session deleted");
            let refresh = requests
                .iter()
                .position(|r| r.path() == "/token")
                .expect("token refreshed");
            assert!(
                refresh < deletion,
                "refresh must precede authenticated cleanup"
            );
            assert_eq!(
                requests[deletion].header("authorization"),
                Some("Bearer at-2")
            );
            assert_eq!(requests[deletion].header("mcp-session-id"), Some("sess-1"));
            let calls: Vec<_> = requests
                .iter()
                .filter(|r| r.rpc_method() == "tools/call")
                .collect();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].header("authorization"), Some("Bearer at-1"));
            assert_eq!(calls[1].header("authorization"), Some("Bearer at-2"));
            assert_eq!(calls[1].header("mcp-session-id"), Some("sess-2"));
        }
        let token = manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.access_token, "at-2");
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("rt-1"),
            "a non-rotating server keeps the existing refresh token"
        );
    }

    async fn ready_refreshable_manager(server: &TestServer) -> Arc<McpClientManager> {
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&server.base()))
            .await
            .unwrap();
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                server,
                "at-1",
                Some("rt-1"),
                &["mcp.read"],
                far_future(),
            ))
            .await
            .unwrap();
        let mcp = connect_http_managed(&manager, server, HttpAuth::None)
            .await
            .unwrap();
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));
        install_oauth_discovery(server, true);
        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-2".to_string());
        manager
    }

    #[tokio::test]
    async fn oauth_refresh_keeps_failed_delete_owned_and_blocks_replacement() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![unauthorized(&server), status_response(503, &[])]);
        let error = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(error.contains("HTTP 503"), "{error}");
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        assert!(matches!(
            handle.snapshot().state,
            crate::supervisor::SupervisorState::Failed
        ));
        assert!(handle.inspect().await.is_err());
        {
            let requests = server.requests.lock().unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| r.rpc_method() == "initialize")
                    .count(),
                1
            );
            assert_eq!(requests.iter().filter(|r| r.path() == "/token").count(), 1);
        }
        server.push_responses(vec![delete_ack()]);
        handle
            .reconfigure(http_config(&server.url, HttpAuth::None))
            .await
            .unwrap();
        let deletes: Vec<_> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.http_method() == "DELETE")
            .map(|r| {
                (
                    r.header("authorization").map(str::to_string),
                    r.header("mcp-session-id").map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            deletes,
            vec![(Some("Bearer at-2".to_string()), Some("sess-1".to_string())); 2]
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_oauth_recovery_refreshes_once_and_cleans_up_with_fresh_bearer() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        server.route("/token", token_response("at-2", None, None));
        server.push_responses(vec![unauthorized(&server), unauthorized(&server)]);
        let cancel = CancellationToken::new();
        let (first, second) = tokio::join!(
            handle.call("report".to_string(), serde_json::json!({}), cancel.clone()),
            handle.call("report".to_string(), serde_json::json!({}), cancel),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        let crate::supervisor::CallRecovery::OAuth(first_kind) = first.recovery else {
            panic!("OAuth recovery");
        };
        let crate::supervisor::CallRecovery::OAuth(second_kind) = second.recovery else {
            panic!("OAuth recovery");
        };
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        let (first, second) = tokio::join!(
            manager.recover_oauth("remote", &handle, first.epoch, first_kind),
            manager.recover_oauth("remote", &handle, second.epoch, second_kind),
        );
        first.unwrap();
        second.unwrap();
        assert!(handle.snapshot().is_ready());
        {
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.iter().filter(|r| r.path() == "/token").count(), 1);
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| r.http_method() == "DELETE")
                    .count(),
                1
            );
        }
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn replacement_oauth_connection_cannot_publish_a_flow_after_reload() {
        let old = TestServer::start(handshake_responses("old-session")).await;
        let manager = ready_refreshable_manager(&old).await;
        old.route("/token", token_response("at-2", Some("rt-2"), None));
        old.push_responses(vec![unauthorized(&old), delete_ack(), unauthorized(&old)]);
        let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *old.routes.initialize_gate.lock().unwrap() = Some((started, Arc::clone(&release)));
        let call = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                manager
                    .call_tool("remote", "report", serde_json::json!({}))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            manager.oauth.mutation_gate("remote").try_lock().is_err(),
            "replacement initialize must retain credential mutation ownership"
        );

        let replacement = TestServer::start(handshake_responses("new-session")).await;
        let config = http_config(
            &replacement.url,
            HttpAuth::Static(StaticCred::Bearer("static-token".into())),
        );
        let reload = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                manager
                    .reload_from_configs(vec![("remote".into(), config)])
                    .await
            }
        });
        release.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(5), reload)
            .await
            .unwrap()
            .unwrap();
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let _ = tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .unwrap()
            .unwrap();
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());
        assert!(pending_auth_url(&manager).await.is_none());
        assert!(!manager.oauth.pending.lock().unwrap().contains_key("remote"));
        let statuses = manager.status().await;
        assert_eq!(statuses[0].state, crate::McpConnState::Ready);
        replacement.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn oauth_refresh_mutations_are_serialized_with_reload_for_success_and_rejection() {
        for (rejected, background_retry) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let server = TestServer::start(handshake_responses("sess-1")).await;
            let manager = ready_refreshable_manager(&server).await;
            *server.routes.delete_bearer.lock().unwrap() = None;
            let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
            let release = Arc::new(tokio::sync::Semaphore::new(0));

            let response = if rejected {
                let mut response = json_doc(&serde_json::json!({"error": "invalid_grant"}));
                response.status = 400;
                response
            } else {
                token_response("at-2", Some("rt-2"), None)
            };
            let responses = if background_retry {
                vec![status_response(503, &[]), response]
            } else {
                vec![response]
            };
            server.route_seq("/token", responses);
            server.push_responses(vec![unauthorized(&server), delete_ack()]);
            server.push_responses(handshake_responses("sess-2"));
            server.push_responses(vec![delete_ack()]);
            let call = if background_retry {
                let error = manager
                    .call_tool("remote", "report", serde_json::json!({}))
                    .await
                    .unwrap_err();
                assert!(error.contains("refresh failed"), "{error}");
                *server.routes.token_gate.lock().unwrap() = Some((started, Arc::clone(&release)));
                tokio::spawn({
                    let manager = Arc::clone(&manager);
                    async move {
                        manager.await_background_tasks().await;
                    }
                })
            } else {
                *server.routes.token_gate.lock().unwrap() = Some((started, Arc::clone(&release)));
                tokio::spawn({
                    let manager = Arc::clone(&manager);
                    async move {
                        let _ = manager
                            .call_tool("remote", "report", serde_json::json!({}))
                            .await;
                    }
                })
            };
            tokio::time::timeout(Duration::from_secs(15), started_rx.recv())
                .await
                .unwrap()
                .unwrap();
            let handle = manager.servers.read().await.get("remote").unwrap().clone();
            let new_token = stored_token(
                &server,
                "new-config-token",
                Some("new-config-refresh"),
                &["mcp.read"],
                far_future(),
            );
            let reload = tokio::spawn({
                let manager = Arc::clone(&manager);
                let config = http_config(&server.url, HttpAuth::None);
                let handle = handle.clone();
                async move {
                    let gate = manager.oauth.mutation_gate("remote");
                    let _serial = gate.lock().await;
                    manager.cancel_pending_oauth_flow("remote").await;
                    handle.reconfigure(config).await.unwrap();
                    manager
                        .oauth
                        .store()
                        .upsert_token(&new_token)
                        .await
                        .unwrap();
                }
            });
            assert!(
                manager.oauth.mutation_gate("remote").try_lock().is_err(),
                "in-flight recovery owns the mutation fence"
            );
            release.add_permits(1);
            tokio::time::timeout(Duration::from_secs(5), reload)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), call)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                manager
                    .oauth
                    .store()
                    .token("remote")
                    .await
                    .unwrap()
                    .unwrap()
                    .access_token,
                "new-config-token"
            );
            assert!(pending_auth_url(&manager).await.is_none());
            assert!(!manager.oauth.pending.lock().unwrap().contains_key("remote"));
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn transient_oauth_refresh_retries_before_session_cleanup() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        server.route_seq(
            "/token",
            vec![
                status_response(503, &[]),
                token_response("at-2", Some("rt-2"), None),
            ],
        );
        server.push_responses(vec![unauthorized(&server), delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        let error = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(error.contains("refresh failed"), "{error}");
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        assert!(matches!(
            handle.snapshot().state,
            crate::supervisor::SupervisorState::Recovering
        ));
        assert!(server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.http_method() != "DELETE"));
        tokio::time::timeout(Duration::from_secs(15), handle.wait_for_settled())
            .await
            .unwrap();
        assert!(handle.snapshot().is_ready());
        {
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.iter().filter(|r| r.path() == "/token").count(), 2);
            let deletion = requests
                .iter()
                .find(|r| r.http_method() == "DELETE")
                .unwrap();
            assert_eq!(deletion.header("authorization"), Some("Bearer at-2"));
        }
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn refresh_rejection_discards_token_and_reprompts() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&server.base()))
            .await
            .unwrap();
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-dead"),
                &["mcp.read"],
                far_future(),
            ))
            .await
            .unwrap();
        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-2".to_string());
        install_oauth_discovery(&server, true);
        server.route(
            "/token",
            CannedResponse {
                status: 400,
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: serde_json::json!({"error": "invalid_grant"}).to_string(),
                delay_ms: 0,
                echo_result: None,
            },
        );
        server.push_responses(vec![unauthorized(&server), delete_ack()]);

        let err = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .expect_err("dead grant chain must surface");
        assert!(err.contains("re-authorize at"), "got: {err}");

        // TokenRefreshFailed: row discarded, server unauthorized with a fresh
        // flow surfaced (REQ-MCP-012), and the server left the map.
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());
        assert!(pending_auth_url(&manager).await.is_some());
        assert!(matches!(
            manager
                .servers
                .read()
                .await
                .get("remote")
                .expect("supervisor retained")
                .snapshot()
                .state,
            crate::supervisor::SupervisorState::Recovering
        ));
        let auth_url = pending_auth_url(&manager).await.unwrap();
        let state = query_params(&auth_url).remove("state").unwrap();
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(handshake_responses("sess-2"));
        manager
            .complete_oauth_authorization(&state, "fresh-code", Some(&server.base()))
            .await
            .unwrap();
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_settled())
            .await
            .unwrap();
        assert!(handle.snapshot().is_ready());
        let deletes: Vec<_> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.http_method() == "DELETE")
            .map(|r| r.header("authorization").map(str::to_string))
            .collect();
        assert_eq!(deletes, vec![Some("Bearer at-2".to_string())]);
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn removal_preserves_transient_refresh_until_cleanup_or_readdition() {
        for (readd, reject_retry) in [(false, false), (true, false), (false, true)] {
            let server = TestServer::start(handshake_responses("sess-1")).await;
            let manager = ready_refreshable_manager(&server).await;
            let handle = manager.servers.read().await.get("remote").unwrap().clone();
            let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
            rejected.status = 400;
            server.route_seq(
                "/token",
                vec![
                    status_response(503, &[]),
                    if reject_retry {
                        rejected
                    } else {
                        token_response("at-2", Some("rt-2"), None)
                    },
                ],
            );
            server.push_responses(vec![unauthorized(&server)]);
            manager
                .call_tool("remote", "report", serde_json::json!({}))
                .await
                .unwrap_err();
            let epoch = handle.snapshot().epoch;
            let removed = manager.reload_from_configs(vec![]).await;
            assert_eq!(removed.removed, vec!["remote"]);
            assert!(removed.failed.is_empty());
            assert!(matches!(
                handle.snapshot().recovery_target,
                crate::supervisor::RecoveryTarget::Remove
            ));
            assert_eq!(handle.snapshot().epoch, epoch);
            assert!(manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_some());
            if readd {
                manager
                    .reload_from_configs(vec![("remote".into(), handle.snapshot().config)])
                    .await;
                assert!(matches!(
                    handle.snapshot().recovery_target,
                    crate::supervisor::RecoveryTarget::Configured
                ));
            }
            if !reject_retry {
                server.push_responses(vec![delete_ack()]);
            }
            if readd {
                server.push_responses(handshake_responses("sess-2"));
            }
            tokio::time::timeout(Duration::from_secs(15), manager.await_background_tasks())
                .await
                .unwrap();
            if reject_retry {
                let params = query_params(&pending_auth_url(&manager).await.unwrap());
                assert!(matches!(
                    manager
                        .oauth
                        .pending
                        .lock()
                        .unwrap()
                        .get("remote")
                        .unwrap()
                        .owner,
                    Some(crate::OAuthFlowOwner::Remove(_, _))
                ));
                server.route("/token", token_response("at-2", Some("rt-2"), None));
                server.push_responses(vec![delete_ack()]);
                manager
                    .complete_oauth_authorization(&params["state"], "code", Some(&server.base()))
                    .await
                    .unwrap();
            }
            assert_eq!(manager.servers.read().await.contains_key("remote"), readd);
            if readd {
                assert!(handle.snapshot().is_ready());
                server.push_responses(vec![delete_ack()]);
                manager.shutdown().await;
            } else {
                assert!(manager
                    .oauth
                    .store()
                    .token("remote")
                    .await
                    .unwrap()
                    .is_none());
                assert_eq!(
                    server
                        .requests
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|request| request.rpc_method() == "initialize")
                        .count(),
                    1
                );
            }
        }
    }

    #[tokio::test]
    async fn queued_removal_uses_the_bearer_recovered_by_refresh() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *server.routes.token_gate.lock().unwrap() = Some((started, Arc::clone(&release)));
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![unauthorized(&server), delete_ack()]);
        let call = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                manager
                    .call_tool("remote", "report", serde_json::json!({}))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let mut removal = Box::pin(manager.reload_from_configs(vec![]));
        assert!(
            futures::poll!(removal.as_mut()).is_pending(),
            "removal waits behind token refresh"
        );
        release.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(5), removal)
            .await
            .unwrap();
        assert!(result.failed.is_empty());
        assert_eq!(result.removed, vec!["remote"]);
        let _ = tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .unwrap()
            .unwrap();
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());
        assert!(!manager.servers.read().await.contains_key("remote"));
        let requests = server.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.rpc_method() == "initialize")
                .count(),
            1
        );
        let deletion = requests
            .iter()
            .find(|r| r.http_method() == "DELETE")
            .unwrap();
        assert_eq!(deletion.header("authorization"), Some("Bearer at-2"));
    }

    #[tokio::test]
    async fn slow_connection_does_not_block_another_servers_oauth_refresh() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let slow = TestServer::start(handshake_responses("slow-session")).await;
        let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *slow.routes.initialize_gate.lock().unwrap() = Some((started, Arc::clone(&release)));
        let config = manager
            .servers
            .read()
            .await
            .get("remote")
            .unwrap()
            .snapshot()
            .config;
        let result = manager
            .reload_from_configs(vec![
                ("remote".into(), config),
                ("slow".into(), http_config(&slow.url, HttpAuth::None)),
            ])
            .await;
        assert!(result.failed.is_empty());
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![unauthorized(&server), delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        server.push_responses(vec![echo_id_response(
            &serde_json::json!({"content": [{"type":"text","text":"ok"}]}),
        )]);
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            manager.call_tool("remote", "report", serde_json::json!({})),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output, "ok");
        release.add_permits(1);
        manager.await_background_tasks().await;
        server.push_responses(vec![delete_ack()]);
        slow.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn oauth_prompt_binds_cleanup_owner_before_recovery_returns() {
        for step_up in [false, true] {
            let server = TestServer::start(handshake_responses("sess-1")).await;
            let manager = ready_refreshable_manager(&server).await;
            let handle = manager.servers.read().await.get("remote").unwrap().clone();
            let crate::supervisor::RecoveryClaim::Leader(permit) =
                handle.claim_oauth_recovery(0).await
            else {
                panic!("OAuth recovery owner");
            };
            if step_up {
                manager
                    .step_up_authorization(
                        "remote",
                        &handle,
                        &permit,
                        "Bearer error=\"insufficient_scope\", scope=\"write\"",
                    )
                    .await
                    .unwrap();
            } else {
                let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
                rejected.status = 400;
                server.route("/token", rejected);
                let outcome = manager
                    .refresh_authorized_server("remote", &handle, &permit, None)
                    .await;
                assert!(matches!(outcome, crate::RefreshServerOutcome::Reprompt(_)));
            }
            let auth_url = pending_auth_url(&manager).await.unwrap();
            {
                let flows = manager.oauth.pending.lock().unwrap();
                let crate::OAuthFlowOwner::Reconnect(owner, epoch) = flows
                    .get("remote")
                    .unwrap()
                    .owner
                    .as_ref()
                    .expect("published flow owns cleanup")
                else {
                    panic!("reconnect owner");
                };
                assert!(owner.same_actor(&handle));
                assert_eq!(*epoch, permit.epoch);
            }
            assert_eq!(
                handle.snapshot().pending_oauth_url.as_deref(),
                Some(auth_url.as_str())
            );
            let state = query_params(&auth_url).remove("state").unwrap();
            server.route("/token", token_response("at-2", Some("rt-2"), None));
            server.push_responses(vec![delete_ack()]);
            server.push_responses(handshake_responses("sess-2"));
            manager
                .complete_oauth_authorization(&state, "fast-code", Some(&server.base()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), handle.wait_for_settled())
                .await
                .unwrap();
            manager
                .wait_for_ready(&handle, &tokio_util::sync::CancellationToken::new())
                .await
                .unwrap();
            assert!(pending_auth_url(&manager).await.is_none());
            let deletes: Vec<_> = server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.http_method() == "DELETE")
                .map(|request| request.header("authorization").map(str::to_string))
                .collect();
            assert_eq!(deletes, vec![Some("Bearer at-2".into())]);
            server.push_responses(vec![delete_ack()]);
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn failed_removal_preserves_oauth_cleanup_owner_for_callback() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
        rejected.status = 400;
        server.route("/token", rejected);
        server.push_responses(vec![unauthorized(&server), delete_ack()]);
        assert!(manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .is_err());
        let auth_url = pending_auth_url(&manager).await.unwrap();
        let state = query_params(&auth_url).remove("state").unwrap();
        let result = manager.reload_from_configs(vec![]).await;
        assert_eq!(result.failed.len(), 1);
        assert!(result.failed[0].error.contains("HTTP 401"));
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        manager
            .complete_oauth_authorization(&state, "code", Some(&server.base()))
            .await
            .unwrap();
        assert!(!manager.servers.read().await.contains_key("remote"));
        assert!(pending_auth_url(&manager).await.is_none());
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.rpc_method() == "initialize")
                .count(),
            1
        );
        let deletes: Vec<_> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.http_method() == "DELETE")
            .map(|request| request.header("authorization").map(str::to_string))
            .collect();
        assert_eq!(
            deletes,
            vec![Some("Bearer at-1".into()), Some("Bearer at-2".into())]
        );
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());
    }

    struct FailOnceDeleteStore {
        inner: Arc<dyn crate::OAuthStore>,
        fail_next: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl crate::OAuthStore for FailOnceDeleteStore {
        async fn registration(
            &self,
            issuer: &str,
        ) -> Result<Option<OAuthRegistrationRecord>, String> {
            self.inner.registration(issuer).await
        }
        async fn upsert_registration(
            &self,
            record: &OAuthRegistrationRecord,
        ) -> Result<(), String> {
            self.inner.upsert_registration(record).await
        }
        async fn token(&self, name: &str) -> Result<Option<OAuthTokenRecord>, String> {
            self.inner.token(name).await
        }
        async fn upsert_token(&self, record: &OAuthTokenRecord) -> Result<(), String> {
            self.inner.upsert_token(record).await
        }
        async fn delete_token(&self, name: &str) -> Result<(), String> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err("injected token deletion failure".into());
            }
            self.inner.delete_token(name).await
        }
    }

    struct FailingRefreshStore {
        inner: Arc<dyn crate::OAuthStore>,
        failures_remaining: std::sync::atomic::AtomicUsize,
        fail_lookup: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl crate::OAuthStore for FailingRefreshStore {
        async fn registration(
            &self,
            issuer: &str,
        ) -> Result<Option<OAuthRegistrationRecord>, String> {
            self.inner.registration(issuer).await
        }
        async fn upsert_registration(
            &self,
            record: &OAuthRegistrationRecord,
        ) -> Result<(), String> {
            self.inner.upsert_registration(record).await
        }
        async fn token(&self, name: &str) -> Result<Option<OAuthTokenRecord>, String> {
            if self.fail_lookup.load(Ordering::SeqCst) {
                return Err("injected token lookup failure".into());
            }
            self.inner.token(name).await
        }
        async fn upsert_token(&self, record: &OAuthTokenRecord) -> Result<(), String> {
            if self
                .failures_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err("injected token persistence failure".into());
            }
            self.inner.upsert_token(record).await
        }
        async fn delete_token(&self, name: &str) -> Result<(), String> {
            self.inner.delete_token(name).await
        }
    }

    #[tokio::test]
    async fn oauth_refresh_persistence_retry_does_not_repeat_rotating_grant() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        manager.set_oauth_store(Arc::new(FailingRefreshStore {
            inner: manager.oauth.store(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(2),
            fail_lookup: std::sync::atomic::AtomicBool::new(false),
        }));
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth recovery owner");
        };
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        let outcome = manager
            .refresh_authorized_server("remote", &handle, &permit, None)
            .await;
        assert!(matches!(outcome, crate::RefreshServerOutcome::Transient(_)));
        let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
        rejected.status = 400;
        server.route("/token", rejected);
        let outcome = manager
            .refresh_authorized_server("remote", &handle, &permit, None)
            .await;
        assert!(matches!(outcome, crate::RefreshServerOutcome::Transient(_)));
        assert_eq!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "at-1"
        );
        assert!(!server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.http_method() == "DELETE"));
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        let outcome = manager
            .refresh_authorized_server("remote", &handle, &permit, None)
            .await;
        assert!(matches!(outcome, crate::RefreshServerOutcome::Refreshed));
        manager
            .finish_oauth_refresh("remote", &handle, &permit, outcome)
            .await
            .unwrap();
        assert!(handle.snapshot().is_ready());
        assert!(pending_auth_url(&manager).await.is_none());
        let token = manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.access_token, "at-2");
        assert_eq!(token.refresh_token.as_deref(), Some("rt-2"));
        assert!(manager
            .oauth
            .unpersisted_refresh_tokens
            .lock()
            .unwrap()
            .is_empty());
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.request_line.starts_with("POST /token "))
                .count(),
            1
        );
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn configuration_invalidates_unpersisted_refresh_even_when_store_lookup_fails() {
        let server = TestServer::start(vec![]).await;
        let manager = McpClientManager::new();
        let token = stored_token(&server, "at-2", Some("rt-2"), &["read"], far_future());
        manager
            .oauth
            .unpersisted_refresh_tokens
            .lock()
            .unwrap()
            .insert("remote".into(), token);
        manager.set_oauth_store(Arc::new(FailingRefreshStore {
            inner: manager.oauth.store(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(0),
            fail_lookup: std::sync::atomic::AtomicBool::new(true),
        }));
        let old = http_config(&server.url, HttpAuth::None);
        let new = http_config(
            &server.url,
            HttpAuth::Static(crate::StaticCred::Bearer("configured".into())),
        );
        manager
            .invalidate_oauth_on_config_change("remote", &old, &new)
            .await;
        assert!(manager
            .oauth
            .unpersisted_refresh_tokens
            .lock()
            .unwrap()
            .is_empty());
        assert!(server.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn oauth_failure_takes_over_failed_transport_cleanup_from_same_epoch() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        assert!(matches!(
            handle.claim_recovery(0).await,
            crate::supervisor::RecoveryClaim::Unavailable(_)
        ));
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        manager
            .recover_oauth(
                "remote",
                &handle,
                0,
                crate::OAuthRecoveryKind::Refresh {
                    www_authenticate: None,
                },
            )
            .await
            .unwrap();
        assert!(handle.snapshot().is_ready());
        {
            let requests = server.requests.lock().unwrap();
            let deletes = requests
                .iter()
                .filter(|request| request.request_line.starts_with("DELETE "))
                .collect::<Vec<_>>();
            assert_eq!(deletes.len(), 2);
            assert_eq!(deletes[0].header("authorization"), Some("Bearer at-1"));
            assert_eq!(deletes[1].header("authorization"), Some("Bearer at-2"));
        }
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn rejected_refresh_with_unstartable_authorization_settles_failed() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        *manager.oauth.redirect_base.lock().unwrap() = None;
        let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
        rejected.status = 400;
        server.route("/token", rejected);
        server.push_responses(vec![unauthorized(&server)]);
        let error = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            error.contains("re-authorization could not start"),
            "{error}"
        );
        assert!(pending_auth_url(&manager).await.is_none());
        assert_eq!(manager.status().await[0].state, crate::McpConnState::Failed);
        tokio::time::timeout(Duration::from_secs(1), manager.await_background_tasks())
            .await
            .unwrap();
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        let config = handle.snapshot().config;
        manager
            .reload_from_configs(vec![("remote".into(), config)])
            .await;
        assert!(pending_auth_url(&manager).await.is_some());
        *server.routes.delete_bearer.lock().unwrap() = None;
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn unstartable_step_up_preserves_scope_union_for_explicit_retry() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        *manager.oauth.redirect_base.lock().unwrap() = None;
        let error = manager
            .recover_oauth(
                "remote",
                &handle,
                0,
                crate::OAuthRecoveryKind::StepUp {
                    www_authenticate: "Bearer error=\"insufficient_scope\", scope=\"write\"".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("re-authorization could not start"));
        assert_eq!(manager.status().await[0].state, crate::McpConnState::Failed);
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .reload_from_configs(vec![("remote".into(), handle.snapshot().config)])
            .await;
        let params = query_params(&pending_auth_url(&manager).await.unwrap());
        let scopes = params["scope"]
            .split_whitespace()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(scopes, ["mcp.read", "write"].into_iter().collect());
        assert!(!server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.request_line.starts_with("DELETE ")));
        *server.routes.delete_bearer.lock().unwrap() = None;
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn denied_authorization_retry_preserves_challenge_directed_discovery() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let issuer = TestServer::start(vec![]).await;
        install_oauth_discovery(&issuer, true);
        let manager = ready_refreshable_manager(&server).await;
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&issuer.base()))
            .await
            .unwrap();
        for path in [
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-authorization-server",
            "/.well-known/openid-configuration",
        ] {
            server.route(path, status_response(404, &[]));
        }
        server.route("/custom-prm", json_doc(&serde_json::json!({
            "resource": server.url, "authorization_servers": [issuer.base()], "scopes_supported": ["mcp.read"]
        })));
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth owner");
        };
        manager
            .step_up_authorization(
                "remote",
                &handle,
                &permit,
                &format!(
                    "Bearer resource_metadata=\"{}/custom-prm\", scope=\"write\"",
                    server.base()
                ),
            )
            .await
            .unwrap();
        let first = query_params(&pending_auth_url(&manager).await.unwrap());
        manager
            .fail_oauth_authorization(&first["state"], "access_denied")
            .await
            .unwrap();
        manager
            .reload_from_configs(vec![("remote".into(), permit.config)])
            .await;
        let second = query_params(&pending_auth_url(&manager).await.unwrap());
        assert_ne!(first["state"], second["state"]);
        assert_eq!(server.recorded_for_path("/custom-prm").len(), 2);
        *server.routes.delete_bearer.lock().unwrap() = None;
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn replacement_handshake_reauthorization_is_owned_and_visible() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
        rejected.status = 400;
        server.route_seq(
            "/token",
            vec![
                token_response("at-2", Some("rt-2"), None),
                rejected,
                token_response("at-3", Some("rt-3"), None),
            ],
        );
        server.push_responses(vec![
            unauthorized(&server),
            delete_ack(),
            unauthorized(&server),
        ]);
        manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(
            manager.status().await[0].state,
            crate::McpConnState::Unauthorized
        );
        let params = query_params(&pending_auth_url(&manager).await.unwrap());
        assert!(manager
            .oauth
            .pending
            .lock()
            .unwrap()
            .get("remote")
            .unwrap()
            .owner
            .is_some());
        server.push_responses(handshake_responses("sess-3"));
        manager
            .complete_oauth_authorization(&params["state"], "code", Some(&server.base()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
            .await
            .unwrap();
        assert_eq!(manager.status().await[0].state, crate::McpConnState::Ready);
        *server.routes.delete_bearer.lock().unwrap() = None;
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn replacement_tools_list_401_and_failed_delete_can_reauthorize() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth recovery owner")
        };
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![delete_ack()]);
        let outcome = manager
            .refresh_authorized_server("remote", &handle, &permit, None)
            .await;
        assert!(matches!(outcome, crate::RefreshServerOutcome::Refreshed));
        let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
        rejected.status = 400;
        server.route("/token", rejected);
        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-3".into());
        let mut replacement = handshake_responses("sess-2");
        replacement[2] = unauthorized(&server);
        server.push_responses(replacement);
        assert!(manager
            .finish_oauth_refresh("remote", &handle, &permit, outcome)
            .await
            .is_err());
        assert_eq!(
            manager.status().await[0].state,
            crate::McpConnState::Unauthorized
        );
        let params = query_params(&pending_auth_url(&manager).await.unwrap());
        assert!(params["scope"]
            .split_whitespace()
            .any(|scope| scope == "mcp.read"));
        manager
            .fail_oauth_authorization(&params["state"], "access_denied")
            .await
            .unwrap();
        manager
            .reload_from_configs(vec![("remote".into(), permit.config)])
            .await;
        let next = query_params(&pending_auth_url(&manager).await.unwrap());
        assert_ne!(next["state"], params["state"]);
        server.route("/token", token_response("at-3", Some("rt-3"), None));
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-3"));
        manager
            .complete_oauth_authorization(&next["state"], "code", Some(&server.base()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
            .await
            .unwrap();
        assert_eq!(manager.status().await[0].state, crate::McpConnState::Ready);
        let deletes: Vec<_> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.http_method() == "DELETE")
            .map(|request| request.header("authorization").map(str::to_owned))
            .collect();
        assert_eq!(
            deletes,
            vec![
                Some("Bearer at-2".into()),
                Some("Bearer at-2".into()),
                Some("Bearer at-3".into())
            ]
        );
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn startup_tools_list_401_and_failed_delete_refresh_silently() {
        for transient in [false, true] {
            let server = TestServer::start(vec![]).await;
            let mut initial = handshake_responses("sess-1");
            initial[2] = unauthorized(&server);
            server.push_responses(initial);
            server.push_responses(vec![delete_ack()]);
            server.push_responses(handshake_responses("sess-2"));
            install_oauth_discovery(&server, true);
            if transient {
                server.route_seq(
                    "/token",
                    vec![
                        status_response(503, &[]),
                        token_response("at-2", Some("rt-2"), None),
                    ],
                );
            } else {
                server.route("/token", token_response("at-2", Some("rt-2"), None));
            }
            *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-2".into());
            let manager = McpClientManager::new();
            manager.set_oauth_redirect_base(REDIRECT_BASE.into());
            manager
                .oauth
                .store()
                .upsert_registration(&none_registration(&server.base()))
                .await
                .unwrap();
            manager
                .oauth
                .store()
                .upsert_token(&stored_token(
                    &server,
                    "at-1",
                    Some("rt-1"),
                    &["mcp.read"],
                    1,
                ))
                .await
                .unwrap();
            let result = manager
                .reload_from_configs(vec![(
                    "remote".into(),
                    http_config(&server.url, HttpAuth::None),
                )])
                .await;
            assert_eq!(result.added, vec!["remote"]);
            tokio::time::timeout(Duration::from_secs(15), manager.await_background_tasks())
                .await
                .unwrap();
            assert_eq!(manager.status().await[0].state, crate::McpConnState::Ready);
            assert!(pending_auth_url(&manager).await.is_none());
            let token = manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(token.access_token, "at-2");
            assert_eq!(token.refresh_token.as_deref(), Some("rt-2"));
            let deletes: Vec<_> = server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.http_method() == "DELETE")
                .map(|request| request.header("authorization").map(str::to_owned))
                .collect();
            assert_eq!(
                deletes,
                vec![Some("Bearer at-1".into()), Some("Bearer at-2".into())]
            );
            server.push_responses(vec![delete_ack()]);
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn refreshed_handshake_cleanup_preserves_auth_cause_without_repeating_grant() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        let mut replacement = handshake_responses("sess-2");
        replacement[2] = unauthorized(&server);
        server.push_responses(replacement);
        install_oauth_discovery(&server, true);
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-3".into());
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.into());
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&server.base()))
            .await
            .unwrap();
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-1"),
                &["mcp.read"],
                1,
            ))
            .await
            .unwrap();
        manager
            .reload_from_configs(vec![(
                "remote".into(),
                http_config(&server.url, HttpAuth::None),
            )])
            .await;
        tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
            .await
            .unwrap();
        assert_eq!(
            manager.status().await[0].state,
            crate::McpConnState::Unauthorized
        );
        assert_eq!(server.recorded_for_path("/token").len(), 1);
        let params = query_params(&pending_auth_url(&manager).await.unwrap());
        server.route("/token", token_response("at-3", Some("rt-3"), None));
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-3"));
        manager
            .complete_oauth_authorization(&params["state"], "code", Some(&server.base()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
            .await
            .unwrap();
        assert_eq!(manager.status().await[0].state, crate::McpConnState::Ready);
        let deletes: Vec<_> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.http_method() == "DELETE")
            .map(|request| request.header("authorization").map(str::to_owned))
            .collect();
        assert_eq!(
            deletes,
            vec![Some("Bearer at-2".into()), Some("Bearer at-3".into())]
        );
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn refreshed_handshake_with_successful_teardown_does_not_repeat_grant() {
        for session_on_replacement in [false, true] {
            let server = TestServer::start(vec![]).await;
            let mut initial = handshake_responses("sess-1");
            initial[2] = unauthorized(&server);
            server.push_responses(initial);
            server.push_responses(vec![delete_ack()]);
            if session_on_replacement {
                let mut replacement = handshake_responses("sess-2");
                replacement[2] = unauthorized(&server);
                server.push_responses(replacement);
                server.push_responses(vec![delete_ack()]);
            } else {
                server.push_responses(vec![unauthorized(&server)]);
            }
            install_oauth_discovery(&server, true);
            server.route("/token", token_response("at-2", Some("rt-2"), None));
            *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-2".into());
            let manager = Arc::new(McpClientManager::new());
            manager.set_oauth_redirect_base(REDIRECT_BASE.into());
            manager
                .oauth
                .store()
                .upsert_registration(&none_registration(&server.base()))
                .await
                .unwrap();
            manager
                .oauth
                .store()
                .upsert_token(&stored_token(
                    &server,
                    "at-1",
                    Some("rt-1"),
                    &["mcp.read", "mcp.write"],
                    1,
                ))
                .await
                .unwrap();
            manager
                .reload_from_configs(vec![(
                    "remote".into(),
                    http_config(&server.url, HttpAuth::None),
                )])
                .await;
            tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
                .await
                .unwrap();
            assert_eq!(server.recorded_for_path("/token").len(), 1);
            assert_eq!(
                manager.status().await[0].state,
                crate::McpConnState::Unauthorized
            );
            let params = query_params(&pending_auth_url(&manager).await.unwrap());
            assert_eq!(
                params["scope"]
                    .split_whitespace()
                    .collect::<std::collections::BTreeSet<_>>(),
                ["mcp.read", "mcp.write"].into_iter().collect()
            );
            server.route("/token", token_response("at-3", Some("rt-3"), None));
            server.push_responses(handshake_responses("sess-3"));
            manager
                .complete_oauth_authorization(&params["state"], "code", Some(&server.base()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
                .await
                .unwrap();
            assert_eq!(manager.status().await[0].state, crate::McpConnState::Ready);
            *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-3".into());
            server.push_responses(vec![delete_ack()]);
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn changed_configuration_waits_for_transient_oauth_cleanup() {
        for reject_retry in [false, true] {
            let server = TestServer::start(handshake_responses("sess-1")).await;
            let replacement = TestServer::start(handshake_responses("new-session")).await;
            let manager = ready_refreshable_manager(&server).await;
            let handle = manager.servers.read().await.get("remote").unwrap().clone();
            let mut rejected = json_doc(&serde_json::json!({"error": "invalid_grant"}));
            rejected.status = 400;
            server.route_seq(
                "/token",
                vec![
                    status_response(503, &[]),
                    if reject_retry {
                        rejected
                    } else {
                        token_response("at-2", Some("rt-2"), None)
                    },
                ],
            );
            server.push_responses(vec![unauthorized(&server)]);
            manager
                .call_tool("remote", "report", serde_json::json!({}))
                .await
                .unwrap_err();
            let config = http_config(
                &replacement.url,
                HttpAuth::Static(crate::StaticCred::Bearer("new-configured".into())),
            );
            let epoch = handle.snapshot().epoch;
            let reload = manager
                .reload_from_configs(vec![("remote".into(), config.clone())])
                .await;
            assert!(reload.failed.is_empty());
            assert_eq!(handle.snapshot().epoch, epoch);
            assert!(manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_some());
            if !reject_retry {
                server.push_responses(vec![delete_ack()]);
            }
            tokio::time::timeout(Duration::from_secs(15), manager.await_background_tasks())
                .await
                .unwrap();
            if reject_retry {
                let params = query_params(&pending_auth_url(&manager).await.unwrap());
                server.route("/token", token_response("at-2", Some("rt-2"), None));
                server.push_responses(vec![delete_ack()]);
                manager
                    .complete_oauth_authorization(&params["state"], "code", Some(&server.base()))
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
                    .await
                    .unwrap();
            }
            assert!(handle.snapshot().is_ready());
            assert_eq!(handle.snapshot().config, config);
            assert!(manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none());
            assert_eq!(
                server
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|request| request.rpc_method() == "initialize")
                    .count(),
                1
            );
            assert_eq!(
                replacement
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|request| request.rpc_method() == "initialize")
                    .count(),
                1
            );
            replacement.push_responses(vec![delete_ack()]);
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn changed_configuration_replaces_pending_cleanup_authorization() {
        for denied_before_change in [false, true] {
            let server = TestServer::start(handshake_responses("sess-1")).await;
            let replacement = TestServer::start(handshake_responses("new-session")).await;
            let manager = ready_refreshable_manager(&server).await;
            let handle = manager.servers.read().await.get("remote").unwrap().clone();
            let crate::supervisor::RecoveryClaim::Leader(permit) =
                handle.claim_oauth_recovery(0).await
            else {
                panic!("OAuth owner");
            };
            manager
                .step_up_authorization(
                    "remote",
                    &handle,
                    &permit,
                    "Bearer error=\"insufficient_scope\", scope=\"write\"",
                )
                .await
                .unwrap();
            let old = query_params(&pending_auth_url(&manager).await.unwrap());
            if denied_before_change {
                manager
                    .fail_oauth_authorization(&old["state"], "access_denied")
                    .await
                    .unwrap();
            }
            let config = http_config(
                &replacement.url,
                HttpAuth::Static(crate::StaticCred::Bearer("new-configured".into())),
            );
            manager
                .reload_from_configs(vec![("remote".into(), config.clone())])
                .await;
            let new = query_params(&pending_auth_url(&manager).await.unwrap());
            assert_ne!(new["state"], old["state"]);
            let epoch = handle.snapshot().epoch;
            let repeated = manager
                .reload_from_configs(vec![("remote".into(), config.clone())])
                .await;
            assert_eq!(repeated.unchanged, vec!["remote"]);
            assert_eq!(handle.snapshot().epoch, epoch);
            assert_eq!(
                query_params(&pending_auth_url(&manager).await.unwrap())["state"],
                new["state"]
            );
            assert!(manager
                .complete_oauth_authorization(&old["state"], "old", Some(&server.base()))
                .await
                .is_err());
            assert!(new["scope"]
                .split_whitespace()
                .any(|scope| scope == "write"));
            server.route("/token", token_response("at-2", Some("rt-2"), None));
            server.push_responses(vec![delete_ack()]);
            manager
                .complete_oauth_authorization(&new["state"], "code", Some(&server.base()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), manager.await_background_tasks())
                .await
                .unwrap();
            assert!(handle.snapshot().is_ready());
            assert_eq!(handle.snapshot().config, config);
            assert!(manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none());
            replacement.push_responses(vec![delete_ack()]);
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn denied_step_up_can_reauthorize_on_unchanged_reload() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth recovery owner");
        };
        manager
            .step_up_authorization(
                "remote",
                &handle,
                &permit,
                "Bearer error=\"insufficient_scope\", scope=\"write\"",
            )
            .await
            .unwrap();
        let old_url = pending_auth_url(&manager).await.unwrap();
        let old = query_params(&old_url);
        manager
            .fail_oauth_authorization(&old["state"], "access_denied")
            .await
            .unwrap();
        assert!(matches!(
            handle.snapshot().state,
            crate::supervisor::SupervisorState::Failed
        ));
        assert!(pending_auth_url(&manager).await.is_none());
        manager
            .reload_from_configs(vec![("remote".into(), permit.config)])
            .await;
        let new = query_params(&pending_auth_url(&manager).await.unwrap());
        let scope_set = |scopes: &str| {
            scopes
                .split_whitespace()
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(scope_set(&new["scope"]), scope_set(&old["scope"]));
        assert!(!server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.request_line.starts_with("DELETE ")));
        assert!(new["scope"]
            .split_whitespace()
            .any(|scope| scope == "write"));
        assert_ne!(new["state"], old["state"]);
        assert!(manager
            .complete_oauth_authorization(&old["state"], "old", Some(&server.base()))
            .await
            .is_err());
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        manager
            .complete_oauth_authorization(&new["state"], "code", Some(&server.base()))
            .await
            .unwrap();
        handle.wait_for_settled().await;
        assert!(handle.snapshot().is_ready());
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn removed_oauth_owner_is_forgotten_after_denial_cleanup_succeeds() {
        for cleanup_succeeds in [false, true] {
            let server = TestServer::start(handshake_responses("sess-1")).await;
            let manager = ready_refreshable_manager(&server).await;
            let handle = manager.servers.read().await.get("remote").unwrap().clone();
            let crate::supervisor::RecoveryClaim::Leader(permit) =
                handle.claim_oauth_recovery(0).await
            else {
                panic!("OAuth recovery owner");
            };
            manager
                .step_up_authorization(
                    "remote",
                    &handle,
                    &permit,
                    "Bearer error=\"insufficient_scope\", scope=\"write\"",
                )
                .await
                .unwrap();
            let state = query_params(&pending_auth_url(&manager).await.unwrap())
                .remove("state")
                .unwrap();
            assert_eq!(manager.reload_from_configs(vec![]).await.failed.len(), 1);
            if cleanup_succeeds {
                *server.routes.delete_bearer.lock().unwrap() = None;
                server.push_responses(vec![delete_ack()]);
            }
            assert_eq!(
                manager
                    .fail_oauth_authorization(&state, "access_denied")
                    .await
                    .as_deref(),
                Some("remote")
            );
            assert_eq!(
                manager.servers.read().await.contains_key("remote"),
                !cleanup_succeeds
            );
            assert!(pending_auth_url(&manager).await.is_none());
            if !cleanup_succeeds {
                assert!(matches!(
                    handle.snapshot().state,
                    crate::supervisor::SupervisorState::Failed
                ));
                let result = manager
                    .reload_from_configs(vec![("remote".into(), permit.config)])
                    .await;
                assert_eq!(result.failed.len(), 1);
                let auth_url = pending_auth_url(&manager).await.unwrap();
                let next = query_params(&auth_url);
                assert_ne!(next["state"], state);
                assert!(next["scope"]
                    .split_whitespace()
                    .any(|scope| scope == "write"));
                assert!(manager
                    .complete_oauth_authorization(&state, "obsolete", Some(&server.base()))
                    .await
                    .is_err());
                server.route("/token", token_response("at-2", Some("rt-2"), None));
                server.push_responses(vec![delete_ack()]);
                server.push_responses(handshake_responses("sess-2"));
                manager
                    .complete_oauth_authorization(&next["state"], "code", Some(&server.base()))
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(5), handle.wait_for_settled())
                    .await
                    .unwrap();
                assert!(handle.snapshot().is_ready());
                server.push_responses(vec![delete_ack()]);
                manager.shutdown().await;
            }
        }
    }

    #[tokio::test]
    async fn readding_removed_owner_reconnects_after_callback_token_delete_failure() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth recovery owner");
        };
        manager
            .step_up_authorization(
                "remote",
                &handle,
                &permit,
                "Bearer error=\"insufficient_scope\", scope=\"write\"",
            )
            .await
            .unwrap();
        let state = query_params(&pending_auth_url(&manager).await.unwrap())
            .remove("state")
            .unwrap();
        assert_eq!(manager.reload_from_configs(vec![]).await.failed.len(), 1);
        manager.set_oauth_store(Arc::new(FailOnceDeleteStore {
            inner: manager.oauth.store(),
            fail_next: std::sync::atomic::AtomicBool::new(true),
        }));
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![delete_ack()]);
        let error = manager
            .complete_oauth_authorization(&state, "code", Some(&server.base()))
            .await
            .unwrap_err();
        assert!(error.contains("injected token deletion failure"));
        assert!(matches!(
            handle.snapshot().state,
            crate::supervisor::SupervisorState::Removed
        ));
        *server.routes.delete_bearer.lock().unwrap() = None;
        server.push_responses(handshake_responses("sess-2"));
        let result = manager
            .reload_from_configs(vec![("remote".into(), permit.config)])
            .await;
        assert!(result.failed.is_empty());
        assert_eq!(result.restarted, vec!["remote"]);
        assert!(handle.snapshot().is_ready());
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn oauth_claim_quiesces_http_stream_without_deleting_the_session() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        server.set_get_responses(vec![delayed(sse_response(": keepalive\n\n"), 30_000)]);
        let bearer: crate::SharedBearer = Arc::default();
        *bearer.write().unwrap() = Some("expired".into());
        let transport = Arc::new(
            HttpTransport::connect(
                "remote",
                &server.url,
                &HashMap::new(),
                &HttpAuth::None,
                Arc::clone(&bearer),
                discard_sink(),
            )
            .unwrap(),
        );
        let mut mcp = crate::McpServer {
            name: "remote".into(),
            transport: transport.clone(),
            config: http_config(&server.url, HttpAuth::None),
            tools: std::sync::RwLock::new(Vec::new()),
            tools_changed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
            oauth_bearer: bearer,
        };
        mcp.initialize().await.unwrap();
        mcp.list_tools().await.unwrap();
        let abort = transport
            .stream_task
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .abort_handle();
        let handle = crate::supervisor::SupervisorHandle::connected(mcp);
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth recovery owner");
        };
        assert!(abort.is_finished());
        assert!(transport.stream_task.lock().unwrap().is_none());
        assert_eq!(
            transport.session_id.lock().unwrap().as_deref(),
            Some("sess-1")
        );
        assert!(!server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.http_method() == "DELETE"));
        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer fresh".into());
        server.push_responses(vec![delete_ack()]);
        assert!(handle
            .finish_oauth_cleanup(permit.epoch, "fresh".into())
            .await
            .unwrap());
        assert!(transport.session_id.lock().unwrap().is_none());
        handle.remove().await.unwrap();
    }

    #[tokio::test]
    async fn readding_a_server_restores_reconnect_intent_after_failed_removal() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = ready_refreshable_manager(&server).await;
        let handle = manager.servers.read().await.get("remote").unwrap().clone();
        let crate::supervisor::RecoveryClaim::Leader(permit) = handle.claim_oauth_recovery(0).await
        else {
            panic!("OAuth recovery owner");
        };
        manager
            .step_up_authorization(
                "remote",
                &handle,
                &permit,
                "Bearer error=\"insufficient_scope\", scope=\"write\"",
            )
            .await
            .unwrap();
        let auth_url = pending_auth_url(&manager).await.unwrap();
        let state = query_params(&auth_url).remove("state").unwrap();
        let result = manager.reload_from_configs(vec![]).await;
        assert_eq!(result.failed.len(), 1);
        let result = manager
            .reload_from_configs(vec![("remote".into(), permit.config)])
            .await;
        assert_eq!(result.unchanged, vec!["remote"]);
        server.route("/token", token_response("at-2", Some("rt-2"), None));
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        manager
            .complete_oauth_authorization(&state, "code", Some(&server.base()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_settled())
            .await
            .unwrap();
        assert!(handle.snapshot().is_ready());
        assert!(manager.servers.read().await.contains_key("remote"));
        assert_eq!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "at-2"
        );
        server.push_responses(vec![delete_ack()]);
        manager.shutdown().await;
    }

    // One end-to-end lifecycle: trigger step-up, inspect the union, complete
    // authorization, and prove the held call replays with the upgraded token.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn insufficient_scope_steps_up_with_union_and_replays_the_call() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&none_registration(&server.base()))
            .await
            .unwrap();
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-1"),
                &["read"],
                far_future(),
            ))
            .await
            .unwrap();
        let mcp = connect_http_managed(
            &manager,
            &server,
            HttpAuth::OAuth(crate::OAuthConfig {
                client: None,
                scopes: vec!["configured".to_string()],
            }),
        )
        .await
        .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        *server.routes.delete_bearer.lock().unwrap() = Some("Bearer at-up".to_string());
        install_oauth_discovery(&server, true);
        // The 403 step-up challenge names the missing scope -- delivered
        // behind a Basic challenge, which must not mask the step-up
        // classification.
        server.push_responses(vec![
            status_response(
                403,
                &[
                    ("www-authenticate", "Basic realm=\"legacy\""),
                    (
                        "www-authenticate",
                        "Bearer error=\"insufficient_scope\", scope=\"write\"",
                    ),
                ],
            ),
            delete_ack(),
        ]);

        // The triggering call parks on the step-up claim and replays once the
        // operator re-authorizes (deferred ReAuthCallRetry).
        let mut publications = manager.oauth.pending_publications.subscribe();
        let caller = Arc::clone(&manager);
        let held_call = tokio::spawn(async move {
            caller
                .call_tool("remote", "report", serde_json::json!({}))
                .await
        });

        let auth_url = next_pending_auth_url(&manager, &mut publications).await;
        let params = query_params(&auth_url);
        let scopes: Vec<&str> = params
            .get("scope")
            .map(|s| s.split(' ').collect())
            .unwrap_or_default();
        assert!(
            scopes.contains(&"configured") && scopes.contains(&"read") && scopes.contains(&"write"),
            "step-up must request configured, prior, and challenged scopes, got: {scopes:?}"
        );
        // The narrow token is gone before re-authorization (OneTokenPerServer).
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());

        // Operator re-authorizes; the call replays with the upgraded token.
        server.route(
            "/token",
            token_response("at-up", Some("rt-up"), Some("read write")),
        );
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(handshake_responses("sess-2"));
        server.push_responses(vec![echo_id_response(&call_result)]);

        let state = params.get("state").expect("state").clone();
        manager
            .complete_oauth_authorization(&state, "code-up", Some(&server.base()))
            .await
            .expect("step-up authorization completes");

        let output = held_call
            .await
            .expect("join")
            .expect("held call must replay after step-up");
        assert_eq!(output, "ok");

        let token = manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.access_token, "at-up");
        assert_eq!(token.scopes, vec!["read", "write"]);
    }

    #[tokio::test]
    async fn callback_rejects_state_mismatch_and_iss_mismatch() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route(
            "/register",
            json_doc(&serde_json::json!({"client_id": "cid-1"})),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("connect blocks on authorization");
        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let state = query_params(&auth_url).get("state").unwrap().clone();

        // A callback whose state matches no pending flow is rejected before
        // any exchange (REQ-MCP-011)...
        let err = manager
            .complete_oauth_authorization("wrong-state", "code-1", Some(&server.base()))
            .await
            .expect_err("state mismatch must be rejected");
        assert!(err.contains("state mismatch"), "got: {err}");

        // ...as is a state-valid callback from the wrong issuer...
        let err = manager
            .complete_oauth_authorization(&state, "code-1", Some("https://evil.example"))
            .await
            .expect_err("iss mismatch must be rejected");
        assert!(
            err.contains("does not match the authorization server"),
            "got: {err}"
        );

        // ...and one omitting iss when the server advertises it.
        let err = manager
            .complete_oauth_authorization(&state, "code-1", None)
            .await
            .expect_err("missing iss must be rejected");
        assert!(err.contains("omitted the 'iss'"), "got: {err}");

        // No code ever reached the token endpoint, and the flow is intact for
        // a correct callback.
        assert!(server.recorded_for_path("/token").is_empty());
        assert!(pending_auth_url(&manager).await.is_some());
    }

    #[tokio::test]
    async fn authorization_server_without_pkce_support_is_refused() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        let base = server.base();
        server.route(
            "/.well-known/oauth-protected-resource/mcp",
            json_doc(&serde_json::json!({
                "authorization_servers": [base],
            })),
        );
        // Metadata WITHOUT code_challenge_methods_supported.
        server.route(
            "/.well-known/oauth-authorization-server",
            json_doc(&serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
            })),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        let err = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("PKCE-less authorization server must be refused");
        assert!(
            err.contains("code_challenge_methods_supported"),
            "got: {err}"
        );

        // Refused before the browser round trip: no flow, no registration.
        assert!(pending_auth_url(&manager).await.is_none());
        assert!(server.recorded_for_path("/register").is_empty());
    }

    #[tokio::test]
    async fn preconfigured_client_seeds_registration_and_skips_dcr() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        // No registration endpoint advertised, and no /register route: DCR
        // would fail loudly if attempted.
        install_oauth_discovery(&server, false);

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        // The authorization server is discovered from the resource metadata;
        // the pre-configured client is seeded under that issuer post-discovery.
        let auth = HttpAuth::OAuth(crate::OAuthConfig {
            client: Some(crate::PreconfiguredClient {
                client_id: "pre-1".to_string(),
                callback_port: None,
            }),
            scopes: vec![
                "configured.read".to_string(),
                "configured.write".to_string(),
            ],
        });
        connect_http_managed(&manager, &server, auth)
            .await
            .expect_err("connect blocks on authorization");

        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let params = query_params(&auth_url);
        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some("pre-1"),
            "the pre-configured client must be reused (OAuthClientReused)"
        );
        assert!(server.recorded_for_path("/register").is_empty());
        assert_eq!(
            params.get("scope").map(String::as_str),
            Some("configured.read configured.write"),
            "configured scopes must override the protected-resource fallback"
        );
    }

    #[tokio::test]
    async fn authorization_flow_skips_an_unusable_advertised_server() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        let base = server.base();
        // The PRM advertises a dead issuer first; its metadata candidates all
        // 404. The flow must fall through to the live sibling instead of
        // failing (REQ-MCP-009).
        server.route(
            "/.well-known/oauth-protected-resource/mcp",
            json_doc(&serde_json::json!({
                "authorization_servers": [format!("{base}/down"), base],
            })),
        );
        let not_found = CannedResponse {
            status: 404,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: "{}".to_string(),
            delay_ms: 0,
            echo_result: None,
        };
        server.route(
            "/.well-known/oauth-authorization-server/down",
            not_found.clone(),
        );
        server.route("/.well-known/openid-configuration/down", not_found.clone());
        server.route("/down/.well-known/openid-configuration", not_found);
        server.route(
            "/.well-known/oauth-authorization-server",
            json_doc(&serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            })),
        );
        server.route(
            "/register",
            json_doc(&serde_json::json!({"client_id": "cid-live"})),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("connect blocks on authorization");

        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        assert_eq!(
            query_params(&auth_url).get("client_id").map(String::as_str),
            Some("cid-live"),
            "the live advertised issuer must be used"
        );
    }

    #[tokio::test]
    async fn authorization_flow_prefers_the_issuer_with_a_registration() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        let base = server.base();
        // Two usable issuers; only the SECOND has a client registration. It
        // must be chosen — picking the first would force DCR even though the
        // user pre-registered a client with its sibling (REQ-MCP-010).
        server.route(
            "/.well-known/oauth-protected-resource/mcp",
            json_doc(&serde_json::json!({
                "authorization_servers": [base, format!("{base}/tenant")],
            })),
        );
        server.route(
            "/.well-known/oauth-authorization-server",
            json_doc(&serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            })),
        );
        server.route(
            "/.well-known/oauth-authorization-server/tenant",
            json_doc(&serde_json::json!({
                "issuer": format!("{base}/tenant"),
                "authorization_endpoint": format!("{base}/tenant/authorize"),
                "token_endpoint": format!("{base}/tenant/token"),
                "code_challenge_methods_supported": ["S256"],
            })),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: format!("{base}/tenant"),
                client_id: "ten-1".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
                redirect_uri: None,
            })
            .await
            .unwrap();
        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("connect blocks on authorization");

        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let params = query_params(&auth_url);
        assert_eq!(params.get("client_id").map(String::as_str), Some("ten-1"));
        assert!(
            auth_url.starts_with(&format!("{base}/tenant/authorize")),
            "the registered issuer's endpoints must be used, got: {auth_url}"
        );
        assert!(
            server.recorded_for_path("/register").is_empty(),
            "no DCR when a sibling issuer already has a registration"
        );
    }

    #[tokio::test]
    async fn non_bearer_token_type_fails_the_flow() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route(
            "/register",
            json_doc(&serde_json::json!({"client_id": "cid-1"})),
        );
        server.route(
            "/token",
            json_doc(&serde_json::json!({
                "access_token": "dpop-tok",
                "token_type": "DPoP",
            })),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("connect blocks on authorization");
        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let state = query_params(&auth_url).get("state").unwrap().clone();

        let err = manager
            .complete_oauth_authorization(&state, "code-1", Some(&server.base()))
            .await
            .expect_err("a non-Bearer token must fail the flow");
        assert!(err.contains("unsupported token_type"), "got: {err}");

        // Nothing was persisted, and the flow stays retryable.
        assert!(manager
            .oauth
            .store()
            .token("remote")
            .await
            .unwrap()
            .is_none());
        assert!(pending_auth_url(&manager).await.is_some());
    }

    #[tokio::test]
    async fn reload_client_id_change_discards_the_stored_token() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-1"),
                &["read"],
                far_future(),
            ))
            .await
            .unwrap();
        let preconfigured = |client_id: &str| McpServerConfig::Http {
            url: server.url.clone(),
            headers: HashMap::new(),
            auth: HttpAuth::OAuth(crate::OAuthConfig {
                client: Some(crate::PreconfiguredClient {
                    client_id: client_id.to_string(),
                    callback_port: None,
                }),
                scopes: Vec::new(),
            }),
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        };
        let mcp = McpClientManager::connect_one(
            "remote",
            &preconfigured("cid-1"),
            Arc::clone(&manager.pending_oauth_urls),
            Arc::clone(&manager.oauth),
            crate::OAuthHandshakeAction::Refresh,
        )
        .await
        .expect("connect with restored token");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // Same URL, different pre-configured client_id: the token was minted
        // under the old client identity and must not restore under the new one
        // (ReloadInvalidatesOAuth). The restarted handshake therefore runs
        // unauthenticated.
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), preconfigured("cid-2"))])
            .await;

        assert_eq!(result.restarted, vec!["remote"]);
        assert!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none(),
            "a changed pre-configured client_id must discard the stored token"
        );
        let requests = server.requests.lock().unwrap();
        let last_init = requests
            .iter()
            .rfind(|r| r.rpc_method() == "initialize")
            .expect("restart initialize");
        assert_eq!(last_init.header("authorization"), None);
    }

    #[tokio::test]
    async fn reload_configured_scope_change_discards_the_stored_token() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-1"),
                &["read"],
                far_future(),
            ))
            .await
            .unwrap();
        let configured = |scope: &str| McpServerConfig::Http {
            url: server.url.clone(),
            headers: HashMap::new(),
            auth: HttpAuth::OAuth(crate::OAuthConfig {
                client: None,
                scopes: vec![scope.to_string()],
            }),
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        };
        let mcp = McpClientManager::connect_one(
            "remote",
            &configured("read"),
            Arc::clone(&manager.pending_oauth_urls),
            Arc::clone(&manager.oauth),
            crate::OAuthHandshakeAction::Refresh,
        )
        .await
        .expect("connect with restored token");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), configured("write"))])
            .await;

        assert_eq!(result.restarted, vec!["remote"]);
        assert!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none(),
            "changed configured scopes must discard the stored token"
        );
        let requests = server.requests.lock().unwrap();
        let last_init = requests
            .iter()
            .rfind(|request| request.rpc_method() == "initialize")
            .expect("restart initialize");
        assert_eq!(last_init.header("authorization"), None);
    }

    #[tokio::test]
    async fn cold_start_de_oauthed_config_discards_the_stored_token() {
        // The auth mode moved to a static credential while Phoenix was down,
        // so no reload rule saw the transition: connect must still discard
        // the OAuth-era token rather than leave it to restore if the config
        // later flips back.
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-old",
                Some("rt-old"),
                &[],
                far_future(),
            ))
            .await
            .unwrap();

        connect_http_managed(
            &manager,
            &server,
            HttpAuth::Static(StaticCred::Bearer("tok".to_string())),
        )
        .await
        .expect("static connect");

        assert!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none(),
            "a non-OAuth config must not leave a stored OAuth token behind"
        );
        // The static credential, not the stale OAuth bearer, rode the wire.
        let requests = server.requests.lock().unwrap();
        for request in requests.iter() {
            assert_eq!(request.header("authorization"), Some("Bearer tok"));
        }
    }

    #[tokio::test]
    async fn reload_url_change_discards_the_stored_token() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(
                &server,
                "at-1",
                Some("rt-1"),
                &["read"],
                far_future(),
            ))
            .await
            .unwrap();
        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        // Repoint the URL (same host, different path = different resource).
        // The restart handshake runs unauthenticated -- and crucially the old
        // token must not survive to be sent to the new endpoint
        // (ReloadInvalidatesOAuth).
        server.push_responses(vec![delete_ack()]);
        server.push_responses(handshake_responses("sess-2"));
        let repointed = McpServerConfig::Http {
            url: format!("{}/v2", server.url),
            headers: HashMap::from([("x-org".to_string(), "acme".to_string())]),
            auth: HttpAuth::None,
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        };
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), repointed)])
            .await;

        assert_eq!(result.restarted, vec!["remote"]);
        assert!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none(),
            "the repointed server's token must be discarded"
        );
        let requests = server.requests.lock().unwrap();
        let last_init = requests
            .iter()
            .rfind(|r| r.rpc_method() == "initialize")
            .expect("restart initialize");
        assert_eq!(
            last_init.header("authorization"),
            None,
            "the old token must not reach the new endpoint"
        );
    }

    #[tokio::test]
    async fn reload_removed_server_deletes_its_token() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager
            .oauth
            .store()
            .upsert_token(&stored_token(&server, "at-1", None, &[], far_future()))
            .await
            .unwrap();
        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), server_handle(mcp));

        server.push_responses(vec![delete_ack()]);
        let result = manager.reload_from_configs(Vec::new()).await;

        assert_eq!(result.removed, vec!["remote"]);
        assert!(
            manager
                .oauth
                .store()
                .token("remote")
                .await
                .unwrap()
                .is_none(),
            "a removed server must not orphan its token (TokenImpliesOAuthServer)"
        );
    }

    #[tokio::test]
    async fn reload_with_changed_config_cancels_pending_auth_and_rotates_nonce() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route(
            "/register",
            json_doc(&serde_json::json!({"client_id": "cid-1"})),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("connect blocks on authorization");
        let old_url = pending_auth_url(&manager).await.expect("pending url");
        let old_state = query_params(&old_url).get("state").unwrap().clone();

        // Reload with a changed config (extra header): the pending flow is
        // cancelled; the restarted connect 401s again and surfaces a NEW flow
        // (ReloadCancelsPendingAuth).
        server.push_responses(vec![unauthorized(&server)]);
        let changed = McpServerConfig::Http {
            url: server.url.clone(),
            headers: HashMap::from([("x-org".to_string(), "other".to_string())]),
            auth: HttpAuth::None,
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        };
        let mut publications = manager.oauth.pending_publications.subscribe();
        manager
            .reload_from_configs(vec![("remote".to_string(), changed)])
            .await;

        let new_url = next_pending_auth_url(&manager, &mut publications).await;
        let new_state = query_params(&new_url).get("state").unwrap().clone();
        assert_ne!(old_state, new_state, "the nonce must rotate");

        // A delayed callback from the pre-reload flow is rejected.
        let err = manager
            .complete_oauth_authorization(&old_state, "stale-code", Some(&server.base()))
            .await
            .expect_err("stale callback must be rejected");
        assert!(err.contains("state mismatch"), "got: {err}");
    }

    #[tokio::test]
    async fn reload_with_unchanged_config_keeps_the_pending_flow() {
        let server = TestServer::start(vec![]).await;
        server.push_responses(vec![unauthorized(&server)]);
        install_oauth_discovery(&server, true);
        server.route(
            "/register",
            json_doc(&serde_json::json!({"client_id": "cid-1"})),
        );

        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        let config = http_config(&server.url, HttpAuth::None);
        connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect_err("connect blocks on authorization");
        let old_url = pending_auth_url(&manager).await.expect("pending url");

        // An unchanged reload must NOT rotate the nonce -- the operator may
        // already have the URL open in a browser.
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), config)])
            .await;
        assert!(
            result.unchanged == vec!["remote"] || result.added == vec!["remote"],
            "reload keeps or re-materializes the same pending server: {result:?}"
        );
        let current_url = pending_auth_url(&manager).await;
        if result.unchanged == vec!["remote"] {
            assert_eq!(current_url.as_deref(), Some(old_url.as_str()));
        }
    }
}
