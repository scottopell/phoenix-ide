//! LLM error types

use std::time::Duration;
use thiserror::Error;

/// LLM error with classification
#[derive(Debug, Error)]
#[error("{message}")]
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl LlmError {
    pub fn new(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, duration: Duration) -> Self {
        self.retry_after = Some(duration);
        self
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
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Network | Self::RateLimit | Self::ServerError)
    }
}
