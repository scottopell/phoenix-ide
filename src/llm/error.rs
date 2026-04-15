//! LLM error types

use thiserror::Error;

/// LLM error with classification
#[derive(Debug, Error)]
#[error("{message}")]
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
    /// When true, a recovery mechanism (e.g. credential helper) is actively
    /// running and may resolve this error. The state machine should wait
    /// rather than treat it as terminal.
    pub recovery_in_progress: bool,
}

impl LlmError {
    pub fn new(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            recovery_in_progress: false,
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

    #[allow(dead_code)] // Will be used when providers detect content filter responses
    pub fn content_filter(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ContentFilter, message)
    }

    #[allow(dead_code)] // Will be used when providers detect context window errors
    pub fn context_window_exceeded(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ContextWindowExceeded, message)
    }
}

/// Error classification for retry logic.
///
/// No `Unknown` variant. No `#[non_exhaustive]`. Adding a new error class
/// requires adding a variant here and handling it in every consumer — the
/// compiler forces it.
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
    /// Content filter or safety block - not retryable
    #[allow(dead_code)] // Will be used when providers detect content filter responses
    ContentFilter,
    /// Context window exceeded - not retryable in current conversation
    #[allow(dead_code)] // Will be used when providers detect context window errors
    ContextWindowExceeded,
}

impl LlmErrorKind {
    pub fn is_retryable(self) -> bool {
        match self {
            Self::Network | Self::RateLimit | Self::ServerError => true,
            Self::Auth
            | Self::InvalidRequest
            | Self::ContentFilter
            | Self::ContextWindowExceeded => false,
        }
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
            // Unexpected status (1xx, 3xx, etc.) — treat as retryable server error
            _ => Self::server_error(format!("Unexpected HTTP {status}: {body}")),
        }
    }
}
