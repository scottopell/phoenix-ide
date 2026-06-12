//! Streamable HTTP MCP transport (REQ-MCP-004, REQ-MCP-005, REQ-MCP-008).
//!
//! A remote MCP server exposes a single endpoint URL: JSON-RPC requests go
//! out as POSTs, and a response arrives either as `application/json` (one
//! JSON-RPC reply) or as `text/event-stream` (a sequence of JSON-RPC
//! messages, ending with the reply). Server-initiated messages on a stream
//! are forwarded to the `ServerMessageSink`; the protocol layer interprets
//! them. Unlike stdio, requests are not serialized: each POST is an
//! independent HTTP exchange correlated by the JSON-RPC id.

use super::{HttpAuth, McpTransport, ServerMessageSink, SharedBearer, StaticCred, TransportError};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const MCP_SESSION_ID: HeaderName = HeaderName::from_static("mcp-session-id");
const MCP_PROTOCOL_VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");

/// Both response framings a Streamable HTTP server may choose (REQ-MCP-004).
const ACCEPT_BOTH: &str = "application/json, text/event-stream";

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
    /// on every request — initialize, tools/*, and the session DELETE
    /// (REQ-MCP-012). `None` for static-credential servers, whose config
    /// authorization is already in `base_headers` and must not be shadowed.
    oauth_bearer: Option<SharedBearer>,
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
    ) -> Result<Self, String> {
        let mut base_headers = HeaderMap::new();
        for (key, value) in headers {
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
            next_id: AtomicU64::new(1),
        })
    }

    /// The current OAuth bearer header value, when one applies.
    fn bearer_header(&self) -> Option<HeaderValue> {
        let token = self.oauth_bearer.as_ref()?.read().unwrap().clone()?;
        HeaderValue::from_str(&format!("Bearer {token}")).ok()
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
        let www_authenticate = response
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
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

fn insert_header(map: &mut HeaderMap, server: &str, key: &str, value: &str) -> Result<(), String> {
    // The session and protocol-version headers are transport state; a
    // config-supplied copy would ride alongside the real value (reqwest's
    // `.header()` appends rather than replaces) and could bind a request to
    // the wrong session. Reject loudly rather than silently dropping what
    // the user wrote.
    if key.eq_ignore_ascii_case("mcp-session-id")
        || key.eq_ignore_ascii_case("mcp-protocol-version")
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
        // request (REQ-MCP-004).
        if method == "initialize" {
            if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
                *self.protocol_version.lock().unwrap() = Some(version.to_string());
            }
        }

        Ok(result)
    }

    async fn notify(&self, notification: &Value) -> Result<(), TransportError> {
        let (builder, sent_session_id) = self.post(super::NOTIFY_TIMEOUT);
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

    fn is_alive(&mut self) -> bool {
        // No process to probe; failures are classified per request and
        // recovery is reconnection (REQ-MCP-007).
        true
    }

    async fn shutdown(&mut self) {
        // End the server-side session explicitly so it does not linger until
        // expiry (REQ-MCP-005). Stateless servers have nothing to delete.
        let session_id = self.session_id.lock().unwrap().take();
        if let Some(session_id) = session_id {
            let mut builder = self
                .client
                .delete(&self.url)
                .timeout(Duration::from_secs(5))
                .headers(self.base_headers.clone())
                .header(MCP_SESSION_ID, session_id);
            // The bearer rides the session DELETE too (REQ-MCP-012).
            if let Some(bearer) = self.bearer_header() {
                builder = builder.header(AUTHORIZATION, bearer);
            }
            // The negotiated protocol version rides every post-initialize
            // request, the session DELETE included (REQ-MCP-004).
            if let Some(version) = self.protocol_version.lock().unwrap().clone() {
                builder = builder.header(MCP_PROTOCOL_VERSION, version);
            }
            let result = builder.send().await;
            if let Err(e) = result {
                tracing::debug!(server = %self.name, "MCP session DELETE failed: {e}");
            }
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
// SSE framing
// ---------------------------------------------------------------------------

/// Incremental SSE event framer over a byte stream: yields the joined `data:`
/// payload of each event. `event:`/`retry:` fields and comments are ignored;
/// `id:` is ignored because POST replies are not resumed (`Last-Event-ID`
/// replay belongs to the server-initiated GET stream, REQ-MCP-006).
#[derive(Default)]
struct SseFramer {
    buf: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseFramer {
    /// Feed a chunk; returns the data payloads of any events completed by it.
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
    use super::super::{McpClientManager, McpServerConfig};
    use super::*;
    use std::collections::VecDeque;
    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::RwLock;

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
    type RouteMap = Arc<Mutex<HashMap<String, VecDeque<CannedResponse>>>>;

    struct TestServer {
        url: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        responses: Arc<Mutex<VecDeque<CannedResponse>>>,
        routes: RouteMap,
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

            let req_log = Arc::clone(&requests);
            let resp_queue = Arc::clone(&responses);
            let route_map = Arc::clone(&routes);
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
                    ));
                }
            });

            Self {
                url,
                requests,
                responses,
                routes,
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
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    async fn handle_connection(
        mut stream: TcpStream,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        responses: Arc<Mutex<VecDeque<CannedResponse>>>,
        routes: RouteMap,
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
            let path = request_line
                .split(' ')
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("")
                .to_string();
            requests.lock().unwrap().push(RecordedRequest {
                request_line,
                headers,
                body,
            });

            let path_response = {
                let mut routes = routes.lock().unwrap();
                match routes.get_mut(&path) {
                    Some(queue) if queue.len() > 1 => queue.pop_front(),
                    Some(queue) => queue.front().cloned(),
                    None => None,
                }
            };
            let response = path_response.unwrap_or_else(|| {
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
            });
            if response.delay_ms > 0 {
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

    async fn connect_http(
        server: &TestServer,
        auth: HttpAuth,
    ) -> Result<super::super::McpServer, String> {
        McpClientManager::connect_one(
            "remote",
            &http_config(&server.url, auth),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::default(),
        )
        .await
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
            .call_tool("report", serde_json::json!({}))
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
        let manager = McpClientManager::new();
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

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
        let manager = McpClientManager::new();
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        mcp.tools_changed
            .store(true, std::sync::atomic::Ordering::Release);
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

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
            .err()
            .expect("handshake must fail");
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
            .insert("remote".to_string(), mcp);

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
        );
        let err = auth.err().expect("auth header must be rejected");
        assert!(err.contains("transport-managed"), "got: {err}");
    }

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<Value>>);

    impl ServerMessageSink for RecordingSink {
        fn on_message(&self, message: Value) {
            self.0.lock().unwrap().push(message);
        }
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
            .insert("remote".to_string(), mcp);

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
    async fn reload_waits_for_held_servers_instead_of_duplicating() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

        // Simulate an in-flight hold (refresh/recovery): the server is out
        // of the map with a claim parked, released 100ms later.
        let (sender, _) = tokio::sync::watch::channel(());
        manager
            .recovering_map()
            .insert("remote".to_string(), sender);
        let held = manager
            .servers
            .write()
            .await
            .remove("remote")
            .expect("server present");
        let holder = Arc::clone(&manager);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            holder
                .servers
                .write()
                .await
                .insert("remote".to_string(), held);
            holder.recovering_map().remove("remote");
        });

        // Reload with the identical config must settle the hold and report
        // unchanged -- not misread the held server as newly added and start
        // a duplicate connection.
        let result = manager
            .reload_from_configs(vec![(
                "remote".to_string(),
                http_config(&server.url, HttpAuth::None),
            )])
            .await;

        assert_eq!(result.unchanged, vec!["remote"]);
        assert!(result.added.is_empty());
        assert!(result.failed.is_empty());
        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(initializes, 1, "no duplicate connection during reload");
    }

    #[tokio::test]
    async fn reload_removes_a_held_server_that_left_the_config() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

        // The server is held out of the map (claim parked) when a reload
        // arrives with it gone from config: the sweep must settle the hold
        // and remove it, or the holder's reinsert would leave a zombie.
        let (sender, _) = tokio::sync::watch::channel(());
        manager
            .recovering_map()
            .insert("remote".to_string(), sender);
        let held = manager
            .servers
            .write()
            .await
            .remove("remote")
            .expect("server present");
        let holder = Arc::clone(&manager);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            holder
                .servers
                .write()
                .await
                .insert("remote".to_string(), held);
            holder.recovering_map().remove("remote");
        });

        server.push_responses(vec![delete_ack()]);
        let result = manager.reload_from_configs(Vec::new()).await;

        assert_eq!(result.removed, vec!["remote"]);
        assert!(manager.servers.read().await.is_empty());
        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.http_method(), "DELETE");
    }

    #[tokio::test]
    async fn late_connect_superseded_by_newer_reload_is_discarded() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());

        // A connect attempt holds this ticket while it is still handshaking...
        let ticket =
            manager.issue_connect_ticket("remote", &http_config(&server.url, HttpAuth::None));
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        // ...when a newer reload (with the server gone from config) revokes
        // the ticket.
        let result = manager.reload_from_configs(Vec::new()).await;
        assert!(result.removed.is_empty());

        // The late publish must be discarded -- not resurrect the removed
        // server -- and the session it created must be DELETEd.
        server.push_responses(vec![delete_ack()]);
        let published = super::super::publish_if_current(
            &manager.servers,
            &manager.connect_tickets,
            "remote",
            ticket,
            mcp,
        )
        .await;

        assert!(!published);
        assert!(manager.servers.read().await.is_empty());
        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.http_method(), "DELETE");
        assert_eq!(last.header("mcp-session-id"), Some("sess-1"));
    }

    #[tokio::test]
    async fn http_call_survives_back_to_back_recovery_claims() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

        // The server is held under claim 1; when that claim releases the
        // server is STILL absent because claim 2 is parked in the same
        // instant (a second recovery starting back-to-back). The call must
        // keep waiting on the new claim instead of failing "not connected".
        let (first_claim, _) = tokio::sync::watch::channel(());
        manager
            .recovering_map()
            .insert("remote".to_string(), first_claim);
        let held = manager
            .servers
            .write()
            .await
            .remove("remote")
            .expect("server present");
        let holder = Arc::clone(&manager);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            // Replacing the entry drops claim 1 (waking waiters) with claim 2
            // already parked, atomically under the map lock.
            let (second_claim, _) = tokio::sync::watch::channel(());
            holder
                .recovering_map()
                .insert("remote".to_string(), second_claim);
            tokio::time::sleep(Duration::from_millis(100)).await;
            holder
                .servers
                .write()
                .await
                .insert("remote".to_string(), held);
            holder.recovering_map().remove("remote");
        });

        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![echo_id_response(&call_result)]);

        let output = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .expect("call must survive consecutive claims");
        assert_eq!(output, "ok");
    }

    #[tokio::test]
    async fn overlapping_reloads_serialize_instead_of_orphaning() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

        // Reload 1 restarts the server with a changed config; its connect is
        // held open for 300ms. Reload 2 (arriving at 100ms, removing the
        // server) must serialize behind it: reload 1 completes its restart,
        // then reload 2 removes the fresh server. Interleaved, reload 2 could
        // revoke reload 1's restart without replacing it.
        server.push_responses(vec![delete_ack()]); // old server's terminate
        let mut recovery = handshake_responses("sess-2");
        recovery[0] = delayed(recovery[0].clone(), 300);
        server.push_responses(recovery);
        server.push_responses(vec![delete_ack()]); // reload 2's removal terminate

        let changed = http_config(
            &server.url,
            HttpAuth::Static(StaticCred::Bearer("tok".to_string())),
        );
        let reloader = Arc::clone(&manager);
        let first_reload = tokio::spawn(async move {
            reloader
                .reload_from_configs(vec![("remote".to_string(), changed)])
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let second = manager.reload_from_configs(Vec::new()).await;

        let first = first_reload.await.expect("first reload");
        assert_eq!(first.restarted, vec!["remote"]);
        assert!(first.failed.is_empty());
        assert_eq!(second.removed, vec!["remote"]);
        assert!(manager.servers.read().await.is_empty());

        // The removed fresh session was explicitly ended.
        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.http_method(), "DELETE");
        assert_eq!(last.header("mcp-session-id"), Some("sess-2"));
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
        assert_eq!(mcp.tools.len(), 1);

        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(initializes, 2, "exactly one fresh-connection retry");
    }

    #[tokio::test]
    async fn reload_leaves_a_same_config_pending_add_to_finish() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let config = http_config(&server.url, HttpAuth::None);

        // An add for this exact config is still handshaking...
        let ticket = manager.issue_connect_ticket("remote", &config);
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        // ...when a reload arrives with the same config: it must not
        // supersede the attempt and gamble on a fresh one.
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), config)])
            .await;
        assert_eq!(result.added, vec!["remote"]);
        let initializes = server
            .recorded()
            .iter()
            .filter(|(_, rpc)| rpc == "initialize")
            .count();
        assert_eq!(initializes, 1, "no duplicate connect was spawned");

        // The in-flight attempt finishes and publishes normally.
        let published = super::super::publish_if_current(
            &manager.servers,
            &manager.connect_tickets,
            "remote",
            ticket,
            mcp,
        )
        .await;
        assert!(published);
        assert_eq!(manager.servers.read().await.len(), 1);
        assert!(manager.connect_tickets.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_connect_clears_its_ticket() {
        let manager = Arc::new(McpClientManager::new());
        // Port 1 refuses connections, so the spawned connect fails fast.
        let unreachable = McpServerConfig::Http {
            url: "http://127.0.0.1:1/mcp".to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
        };
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), unreachable)])
            .await;
        assert_eq!(result.added, vec!["remote"]);

        // The dead attempt must consume its ticket, or a later same-config
        // reload would mistake it for an in-flight connect and never retry.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if manager.connect_tickets.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("failed connect left its ticket parked");
    }

    #[tokio::test]
    async fn reload_applies_config_change_to_a_held_server() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

        // The old-config server is held out by a refresh/recovery claim when
        // a reload with a NEW config arrives. The holder reinserts the OLD
        // config; the reload must still apply the new one rather than
        // reporting unchanged.
        let (claim, _) = tokio::sync::watch::channel(());
        manager.recovering_map().insert("remote".to_string(), claim);
        let held = manager
            .servers
            .write()
            .await
            .remove("remote")
            .expect("server present");
        let holder = Arc::clone(&manager);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            holder
                .servers
                .write()
                .await
                .insert("remote".to_string(), held);
            holder.recovering_map().remove("remote");
        });

        server.push_responses(vec![delete_ack()]); // old server's terminate
        server.push_responses(handshake_responses("sess-2"));

        let changed = http_config(
            &server.url,
            HttpAuth::Static(StaticCred::Bearer("tok".to_string())),
        );
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), changed.clone())])
            .await;

        assert_eq!(result.restarted, vec!["remote"]);
        assert!(result.unchanged.is_empty());
        let servers = manager.servers.read().await;
        let live = servers.get("remote").expect("server present");
        assert_eq!(live.config(), changed, "the new config must be applied");
        drop(servers);

        // The restarted handshake carried the new static credential.
        let requests = server.requests.lock().unwrap();
        let last_init = requests
            .iter()
            .rfind(|r| r.rpc_method() == "initialize")
            .expect("restart initialize");
        assert_eq!(last_init.header("authorization"), Some("Bearer tok"));
    }

    #[tokio::test]
    async fn http_failed_refresh_recovery_drops_the_server() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = McpClientManager::new();
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        mcp.tools_changed
            .store(true, std::sync::atomic::Ordering::Release);
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

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
        assert!(manager.status().await.is_empty());
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
            .insert("remote".to_string(), mcp);

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
            .insert("remote".to_string(), mcp);

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
        assert!(manager.recovering.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn http_unauthorized_401_is_surfaced_not_retried() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = McpClientManager::new();
        let mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

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
        let mut mcp = connect_http(&server, HttpAuth::None)
            .await
            .expect("connect");

        server.push_responses(vec![status_response(200, &[])]);
        mcp.terminate().await;

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
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn sse_framer_flushes_an_unterminated_final_data_line() {
        // A server may close the response right after the last data line,
        // with no trailing newline; EOF must act as the terminator.
        let mut framer = SseFramer::default();
        assert!(framer.push(b"data: {\"a\":1}").is_empty());
        assert_eq!(framer.finish(), Some("{\"a\":1}".to_string()));
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn sse_framer_ignores_comments_and_ids() {
        let mut framer = SseFramer::default();
        let events = framer.push(b": keepalive\nid: 7\nretry: 100\ndata: x\n\n");
        assert_eq!(events, vec!["x"]);
    }

    // -----------------------------------------------------------------------
    // OAuth 2.1 lifecycle (REQ-MCP-009..013) against the scripted server.
    // The TestServer doubles as the protected resource (/mcp), its RFC 9728
    // metadata, the authorization server's RFC 8414 metadata, and the
    // registration + token endpoints, via path routing.
    // -----------------------------------------------------------------------

    use super::super::oauth::{self, OAuthRegistrationRecord, OAuthTokenRecord};
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

    fn unauthorized(server: &TestServer) -> CannedResponse {
        status_response(
            401,
            &[(
                "www-authenticate",
                &format!(
                    "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
                    server.base()
                ),
            )],
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
    ) -> Result<super::super::McpServer, String> {
        McpClientManager::connect_one(
            "remote",
            &http_config(&server.url, auth),
            Arc::clone(&manager.pending_oauth_urls),
            Arc::clone(&manager.oauth),
        )
        .await
    }

    /// Poll `f` until it yields Some, panicking after a deadline. The OAuth
    /// completion path reconnects in a background task, so tests observe it
    /// through the manager's published state.
    async fn poll_until<T, F, Fut>(what: &str, mut f: F) -> T
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Option<T>>,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(value) = f().await {
                return value;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn pending_auth_url(manager: &McpClientManager) -> Option<String> {
        manager
            .status()
            .await
            .into_iter()
            .find_map(|s| s.pending_oauth_url)
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
            .err()
            .expect("unauthorized connect must not publish");
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

        poll_until("server to connect", || async {
            manager
                .status()
                .await
                .into_iter()
                .find(|s| s.tool_count == 1)
        })
        .await;

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
        .err()
        .expect("rejected static auth must fail");
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
        assert_eq!(mcp.tools.len(), 1);

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
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: server.base(),
                client_id: "cid-1".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
            })
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
        assert_eq!(mcp.tools.len(), 1);
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
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: server.base(),
                client_id: "cid-1".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
            })
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
            .insert("remote".to_string(), mcp);

        // The call 401s (revoked/expired server-side); the silent refresh
        // rotates the bearer and the executor replays the call
        // (TokenRefreshNeeded -> TokenRefreshed -> retry).
        install_oauth_discovery(&server, true);
        server.route("/token", token_response("at-2", None, None));
        let call_result = serde_json::json!({"content": [{"type": "text", "text": "ok"}]});
        server.push_responses(vec![unauthorized(&server), echo_id_response(&call_result)]);

        let output = manager
            .call_tool("remote", "report", serde_json::json!({}))
            .await
            .expect("call replays after silent refresh");
        assert_eq!(output, "ok");

        // Exactly two tools/call attempts; the replay carries the rotated
        // bearer on the still-live session.
        {
            let requests = server.requests.lock().unwrap();
            let calls: Vec<_> = requests
                .iter()
                .filter(|r| r.rpc_method() == "tools/call")
                .collect();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].header("authorization"), Some("Bearer at-1"));
            assert_eq!(calls[1].header("authorization"), Some("Bearer at-2"));
            assert_eq!(calls[1].header("mcp-session-id"), Some("sess-1"));
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

    #[tokio::test]
    async fn refresh_rejection_discards_token_and_reprompts() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: server.base(),
                client_id: "cid-1".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
            })
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
            .insert("remote".to_string(), mcp);

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
        assert!(manager.servers.read().await.is_empty());
    }

    #[tokio::test]
    async fn insufficient_scope_steps_up_with_union_and_replays_the_call() {
        let server = TestServer::start(handshake_responses("sess-1")).await;
        let manager = Arc::new(McpClientManager::new());
        manager.set_oauth_redirect_base(REDIRECT_BASE.to_string());
        manager
            .oauth
            .store()
            .upsert_registration(&OAuthRegistrationRecord {
                auth_server: server.base(),
                client_id: "cid-1".to_string(),
                client_secret: None,
                token_endpoint_auth_method: "none".to_string(),
            })
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
        let mcp = connect_http_managed(&manager, &server, HttpAuth::None)
            .await
            .expect("connect");
        manager
            .servers
            .write()
            .await
            .insert("remote".to_string(), mcp);

        install_oauth_discovery(&server, true);
        // The 403 step-up challenge names the missing scope; the old session
        // is DELETEd during the step-up teardown.
        server.push_responses(vec![
            status_response(
                403,
                &[(
                    "www-authenticate",
                    "Bearer error=\"insufficient_scope\", scope=\"write\"",
                )],
            ),
            delete_ack(),
        ]);

        // The triggering call parks on the step-up claim and replays once the
        // operator re-authorizes (deferred ReAuthCallRetry).
        let caller = Arc::clone(&manager);
        let held_call = tokio::spawn(async move {
            caller
                .call_tool("remote", "report", serde_json::json!({}))
                .await
        });

        let auth_url = poll_until("step-up auth url", || async {
            pending_auth_url(&manager).await
        })
        .await;
        let params = query_params(&auth_url);
        let scopes: Vec<&str> = params
            .get("scope")
            .map(|s| s.split(' ').collect())
            .unwrap_or_default();
        assert!(
            scopes.contains(&"read") && scopes.contains(&"write"),
            "step-up must request the union of prior and challenged scopes, got: {scopes:?}"
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
            .err()
            .expect("connect blocks on authorization");
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
            .err()
            .expect("PKCE-less authorization server must be refused");
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
        let auth = HttpAuth::OAuth(Some(super::super::PreconfiguredClient {
            auth_server: server.base(),
            client_id: "pre-1".to_string(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_string(),
        }));
        connect_http_managed(&manager, &server, auth)
            .await
            .err()
            .expect("connect blocks on authorization");

        let auth_url = pending_auth_url(&manager).await.expect("pending url");
        let params = query_params(&auth_url);
        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some("pre-1"),
            "the pre-configured client must be reused (OAuthClientReused)"
        );
        assert!(server.recorded_for_path("/register").is_empty());
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
            .insert("remote".to_string(), mcp);

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
            .insert("remote".to_string(), mcp);

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
            .err()
            .expect("connect blocks on authorization");
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
        };
        manager
            .reload_from_configs(vec![("remote".to_string(), changed)])
            .await;

        let new_url = poll_until("re-surfaced auth url", || async {
            pending_auth_url(&manager).await
        })
        .await;
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
            .err()
            .expect("connect blocks on authorization");
        let old_url = pending_auth_url(&manager).await.expect("pending url");

        // An unchanged reload must NOT rotate the nonce -- the operator may
        // already have the URL open in a browser.
        let result = manager
            .reload_from_configs(vec![("remote".to_string(), config)])
            .await;
        assert_eq!(result.unchanged, vec!["remote"]);
        assert_eq!(
            pending_auth_url(&manager).await.as_deref(),
            Some(old_url.as_str())
        );
    }
}
