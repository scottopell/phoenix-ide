//! Stdio MCP transport: a child process exchanging JSON-RPC 2.0 over its
//! stdin/stdout (REQ-MCP-003).

use super::{McpTransport, ServerMessageSink, TransportError};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

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

                // A message carrying a `method` is server-initiated (a
                // request or a notification); responses never carry one. Its
                // id space (if any) is independent of ours -- a server `ping`
                // whose id collides with our request id is not our reply --
                // so this check must come before id correlation. Forward to
                // the protocol layer and keep waiting for the response.
                if parsed.get("method").is_some() {
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

    fn requested_protocol_version(&self) -> &'static str {
        "2024-11-05"
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
