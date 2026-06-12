//! Streamable HTTP MCP transport (REQ-MCP-004, REQ-MCP-005, REQ-MCP-008).
//!
//! A remote MCP server exposes a single endpoint URL: JSON-RPC requests go
//! out as POSTs, and a response arrives either as `application/json` (one
//! JSON-RPC reply) or as `text/event-stream` (a sequence of JSON-RPC
//! messages, ending with the reply). Server-initiated messages on a stream
//! are forwarded to the `ServerMessageSink`; the protocol layer interprets
//! them. Unlike stdio, requests are not serialized: each POST is an
//! independent HTTP exchange correlated by the JSON-RPC id.

use super::{HttpAuth, McpTransport, ServerMessageSink, StaticCred, TransportError};
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
            next_id: AtomicU64::new(1),
        })
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
        &self,
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
                // The server-side session is gone; the next initialize must
                // not echo the dead id (REQ-MCP-005). Clear only if the
                // stored id is still the one this request sent -- a
                // concurrent recovery may already hold a fresh session.
                let mut stored = self.session_id.lock().unwrap();
                if stored.as_deref() == sent_session_id {
                    *stored = None;
                }
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

        // Server-initiated requests and notifications carry their own id (or
        // none); both are the protocol layer's concern.
        if message.get("method").is_some() {
            sink.on_message(message);
            return None;
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

        let response = self.classify_status(response, sent_session_id.as_deref())?;

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
        self.classify_status(response, sent_session_id.as_deref())
            .map(|_| ())
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

    /// Flush a final event not terminated by a blank line before EOF.
    fn finish(&mut self) -> Option<String> {
        if self.data_lines.is_empty() {
            None
        } else {
            let event = self.data_lines.join("\n");
            self.data_lines.clear();
            Some(event)
        }
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
        }
    }

    fn accepted() -> CannedResponse {
        CannedResponse {
            status: 202,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    fn sse_response(body: &str) -> CannedResponse {
        CannedResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
            body: body.to_string(),
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
        }
    }

    struct TestServer {
        url: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        responses: Arc<Mutex<VecDeque<CannedResponse>>>,
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

            let req_log = Arc::clone(&requests);
            let resp_queue = Arc::clone(&responses);
            let accept_task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(handle_connection(
                        stream,
                        Arc::clone(&req_log),
                        Arc::clone(&resp_queue),
                    ));
                }
            });

            Self {
                url,
                requests,
                responses,
                accept_task,
            }
        }

        fn push_responses(&self, responses: Vec<CannedResponse>) {
            self.responses.lock().unwrap().extend(responses);
        }

        fn recorded(&self) -> Vec<(String, String)> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|r| (r.http_method().to_string(), r.rpc_method()))
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

            requests.lock().unwrap().push(RecordedRequest {
                request_line,
                headers,
                body,
            });

            let response = responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(CannedResponse {
                    status: 500,
                    headers: Vec::new(),
                    body: String::new(),
                });
            let mut out = format!("HTTP/1.1 {} Test\r\n", response.status);
            for (key, value) in &response.headers {
                let _ = write!(out, "{key}: {value}\r\n");
            }
            let _ = write!(out, "content-length: {}\r\n\r\n", response.body.len());
            out.push_str(&response.body);
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
        server.push_responses(vec![status_response(404, &[])]);
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

        let methods: Vec<String> = server.recorded().into_iter().map(|(_, rpc)| rpc).collect();
        let methods: Vec<&str> = methods.iter().map(String::as_str).collect();
        assert_eq!(
            methods,
            vec![
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
            ]
        );

        // The retried call rides the fresh session, not the expired one.
        let requests = server.requests.lock().unwrap();
        let last = requests.last().expect("requests recorded");
        assert_eq!(last.header("mcp-session-id"), Some("sess-2"));
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
        let transport =
            HttpTransport::connect("remote", &server.url, &HashMap::new(), &HttpAuth::None)
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
        assert!(transport.session_id.lock().unwrap().is_none());
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
    fn sse_framer_ignores_comments_and_ids() {
        let mut framer = SseFramer::default();
        let events = framer.push(b": keepalive\nid: 7\nretry: 100\ndata: x\n\n");
        assert_eq!(events, vec!["x"]);
    }
}
