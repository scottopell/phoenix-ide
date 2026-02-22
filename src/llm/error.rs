//! LLM error types

use thiserror::Error;

/// LLM error with classification
#[derive(Debug, Error)]
#[error("{message}")]
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
}

impl LlmError {
    pub fn new(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::Network, message)
    }

    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::RateLimit, message)
    }

    pub fn server_error(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ServerError, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::Auth, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::InvalidRequest, message)
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::Unknown, message)
    }
}

/// Error classification for retry logic
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Network issues, timeouts - retryable
    Network,
    /// Rate limited (429) - retryable with backoff
    RateLimit,
    /// Server error (5xx) - retryable
    ServerError,
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Unknown error
    Unknown,
}

impl LlmErrorKind {
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Network | Self::RateLimit | Self::ServerError)
    }
}

impl LlmError {
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::InvalidRequest, message)
    }

    pub fn from_http_status(status: u16, body: &str) -> Self {
        match status {
            401 | 403 => Self::auth(format!("Authentication failed: {body}")),
            429 => Self::rate_limit(format!("Rate limited: {body}")),
            400..=499 => Self::invalid_request(format!("Bad request ({status}): {body}")),
            500..=599 => Self::server_error(format!("Server error ({status}): {body}")),
            _ => Self::unknown(format!("HTTP {status}: {body}")),
        }
    }
}
