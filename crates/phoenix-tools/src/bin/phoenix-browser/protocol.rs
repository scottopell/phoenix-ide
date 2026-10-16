use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub const MAX_MESSAGE: u64 = 32 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub command: String,
    pub input: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub session: String,
    pub lifetime: Option<String>,
    pub browser_lifetime: Option<String>,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Success { result: Value },
    Error { error: Failure },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    StateLost,
    StartupFailed,
    TransportUncertain,
    OperationFailed,
    BrowserStartFailed,
    CleanupFailed,
    BrowserStateUnavailable,
    OperationCancelled,
    Busy,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<Value>,
    pub artifacts: Vec<Value>,
}

impl Response {
    pub fn success(session: &str, lifetime: Option<&str>, result: Value) -> Self {
        Self {
            session: session.into(),
            lifetime: lifetime.map(str::to_string),
            browser_lifetime: None,
            outcome: Outcome::Success { result },
        }
    }

    pub fn error(
        session: &str,
        lifetime: Option<&str>,
        code: ErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            session: session.into(),
            lifetime: lifetime.map(str::to_string),
            browser_lifetime: None,
            outcome: Outcome::Error {
                error: Failure {
                    code,
                    message: message.into(),
                    details: None,
                    artifacts: Vec::new(),
                },
            },
        }
    }

    pub fn with_browser_lifetime(mut self, lifetime: Option<String>) -> Self {
        self.browser_lifetime = lifetime;
        self
    }
}

#[derive(Clone, Debug)]
pub struct SessionPaths {
    pub directory: PathBuf,
    pub key: String,
}

impl SessionPaths {
    pub fn socket(&self) -> PathBuf {
        self.directory.join(format!("{}.sock", self.key))
    }
    pub fn owner_lock(&self) -> PathBuf {
        self.directory.join(format!("{}.owner", self.key))
    }
    pub fn startup_lock(&self) -> PathBuf {
        self.directory.join(format!("{}.startup", self.key))
    }
    pub fn log(&self) -> PathBuf {
        self.directory.join(format!("{}.log", self.key))
    }
}

pub async fn read_message<T: serde::de::DeserializeOwned>(
    stream: impl AsyncRead + Unpin,
) -> Result<T, String> {
    use tokio::io::AsyncReadExt;
    let mut line = String::new();
    let count = BufReader::new(stream)
        .take(MAX_MESSAGE + 1)
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;
    if count == 0 || count as u64 > MAX_MESSAGE || !line.ends_with('\n') {
        return Err("missing, incomplete, or oversized protocol message".into());
    }
    serde_json::from_str(&line).map_err(|e| format!("invalid protocol message: {e}"))
}

pub async fn write_message(
    stream: &mut (impl AsyncWrite + Unpin),
    message: &impl Serialize,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    if bytes.len() as u64 >= MAX_MESSAGE {
        return Err("protocol message too large".into());
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())
}
