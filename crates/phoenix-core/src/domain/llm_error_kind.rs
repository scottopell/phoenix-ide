//! LLM error classification: the retry-taxonomy enum (`LlmErrorKind`) and the
//! wire-level retryable-reason projection (`LlmAttemptReason`).
//!
//! Both are co-owned by the llm layer (which produces them), the runtime /
//! state machine (which classify retries from them), and the api/wire layer
//! (which serializes `LlmAttemptReason`). They live in the base crate so those
//! layers depend *down* onto a common vocabulary instead of onto each other.

use crate::domain::retry_policy::{AutoRetryPolicy, UserResumePolicy};

/// Error classification for retry logic.
///
/// No `Unknown` variant. No `#[non_exhaustive]`. Adding a new error class
/// requires adding a variant here and handling it in every consumer — the
/// compiler forces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Network issues, timeouts - retryable
    Network,
    /// Transient rate-limit throttle (per-minute, per-second windows) - retryable with backoff
    RateLimit,
    /// Quota window exhausted (plan-level cap hit, credits depleted, etc.) - NOT retryable
    UsageLimitReached,
    /// Model output limit reached mid-generation - not retryable, but the user can resume with a narrower or shorter request.
    OutputLimitExceeded,
    /// Server error (5xx) - retryable
    ServerError,
    /// Selected model is at capacity (`server_is_overloaded` / `slow_down`) - NOT retryable
    ServerOverloaded,
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Provider returned bytes we could not parse or understand (malformed SSE
    /// event, unparseable body, unexpected content-block shape). The request
    /// was accepted; the *response* is at fault — a transient server/transport
    /// problem (often a transparent base-URL gateway), so it is retryable and
    /// user-resumable.
    InvalidResponse,
    /// Content filter or safety block - not retryable
    #[allow(dead_code)] // Will be used when providers detect content filter responses
    ContentFilter,
    /// Context window exceeded - not retryable in current conversation
    #[allow(dead_code)] // Will be used when providers detect context window errors
    ContextWindowExceeded,
}

/// The retryable-error classification that ships on the wire as part of
/// `SseWireEvent::LlmAttempt`. Mirrors the retryable subset of
/// `LlmErrorKind` exactly — adding a new retryable kind requires adding
/// a variant here and updating `LlmAttemptReason::from_kind`, which the
/// compiler forces via exhaustive `match`.
///
/// Specs: `specs/llm-retry-visibility/`. The wire-level `snake_case` is
/// emitted by `serde` via the `rename_all` attribute so the JSON values
/// match the spec's `{rate_limit, server_error, network}` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
#[serde(rename_all = "snake_case")]
pub enum LlmAttemptReason {
    /// Server returned 429 (rate-limit throttle). Transient — the
    /// state machine schedules a retry with exponential backoff.
    RateLimit,
    /// Server returned 5xx. Retryable; same backoff as `RateLimit`.
    ServerError,
    /// Network / timeout. Retryable.
    Network,
}

impl LlmAttemptReason {
    /// Project an `LlmErrorKind` onto the retryable subset. Returns `None`
    /// for non-retryable kinds (the state machine's
    /// `error_kind.is_retryable()` guard ensures `Effect::ScheduleRetry`
    /// is only fired for retryable kinds, so the `None` branch is
    /// structurally unreachable from the runtime; callers still
    /// gracefully `unwrap_or` so a future code change doesn't panic).
    ///
    /// Currently the runtime threads `db::ErrorKind` through
    /// `Event::LlmError` rather than `LlmErrorKind`, so the projection
    /// helper `state_machine::transition::error_kind_to_attempt_reason`
    /// is what the wire emission actually calls. This `from_kind` is
    /// retained for callers that hold an `LlmErrorKind` directly
    /// (tests, future producers).
    #[allow(dead_code)]
    #[must_use]
    pub fn from_kind(kind: LlmErrorKind) -> Option<Self> {
        match kind {
            LlmErrorKind::Network => Some(Self::Network),
            LlmErrorKind::RateLimit => Some(Self::RateLimit),
            // A malformed response is retryable; on the wire its transient
            // retry banner reuses the `server_error` reason (it is a
            // server/transport fault from the client's view) rather than
            // widening the spec'd `{rate_limit, server_error, network}` set.
            LlmErrorKind::ServerError | LlmErrorKind::InvalidResponse => Some(Self::ServerError),
            // Non-retryable kinds never reach Effect::ScheduleRetry.
            LlmErrorKind::UsageLimitReached
            | LlmErrorKind::OutputLimitExceeded
            | LlmErrorKind::ServerOverloaded
            | LlmErrorKind::Auth
            | LlmErrorKind::InvalidRequest
            | LlmErrorKind::ContentFilter
            | LlmErrorKind::ContextWindowExceeded => None,
        }
    }
}

impl LlmErrorKind {
    #[must_use]
    pub fn auto_retry_policy(self) -> AutoRetryPolicy {
        match self {
            Self::Network | Self::RateLimit | Self::ServerError | Self::InvalidResponse => {
                AutoRetryPolicy::AutoRetryable
            }
            Self::UsageLimitReached
            | Self::OutputLimitExceeded
            | Self::ServerOverloaded
            | Self::Auth
            | Self::InvalidRequest
            | Self::ContentFilter
            | Self::ContextWindowExceeded => AutoRetryPolicy::NoAutoRetry,
        }
    }

    #[must_use]
    pub fn is_auto_retryable(self) -> bool {
        self.auto_retry_policy().allows_auto_retry()
    }

    #[must_use]
    pub fn user_resume_policy(self) -> UserResumePolicy {
        match self {
            // A usage-limit window resets on a clock boundary ("try again at
            // 1:01 AM"). Like `ServerOverloaded`, the user can resume once the
            // window clears, so it is user-resumable even though it is never
            // *auto*-retried (no point hammering a reset-on-clock quota).
            Self::Auth
            | Self::Network
            | Self::RateLimit
            | Self::ServerError
            | Self::InvalidResponse
            | Self::ServerOverloaded
            | Self::UsageLimitReached
            | Self::OutputLimitExceeded => UserResumePolicy::Resumable,
            Self::InvalidRequest | Self::ContentFilter | Self::ContextWindowExceeded => {
                UserResumePolicy::NotResumable
            }
        }
    }

    #[must_use]
    pub fn is_user_resumable(self) -> bool {
        self.user_resume_policy().allows_user_resume()
    }
}
