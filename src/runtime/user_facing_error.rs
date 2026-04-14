//! User-facing error type for SSE broadcasts.
//!
//! **Correct-by-construction principle** (AGENTS.md): `SseEvent::Error`
//! ships a payload that the client renders verbatim in a red toast. It
//! must not be possible to accidentally put a raw Rust `Debug`-format
//! string, an internal enum variant name, or a low-level error message
//! into that payload — the type system should refuse.
//!
//! Before task 24682 the payload was a bare `String`, and the call sites
//! used patterns like `e.to_string()` on `TransitionError`. An idle
//! `POST /cancel` produced the toast `"Invalid transition: No transition
//! from Idle with event UserCancel { reason: None }"` — the `Debug`
//! format of the internal state-machine enum leaking through.
//!
//! This module gates access: `SseEvent::Error` now carries a
//! [`UserFacingError`] which can only be constructed via the factory
//! functions defined here. Each factory hand-writes the user-visible text,
//! so there is no path for `Debug` to sneak in. `From<TransitionError>`
//! intentionally does *not* exist; the call site is forced to choose the
//! safe user-visible variant explicitly (and usually the right answer is
//! to log the internal error and show a generic message).

use crate::state_machine::transition::TransitionError;
use serde::Serialize;

/// Severity classification used by the client to decide retry affordances.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserFacingErrorKind {
    /// User might be able to retry the operation (rate limit, transient
    /// network, overload).
    Retryable,
    /// Non-recoverable at this layer (auth, invalid config). Usually needs
    /// user action outside Phoenix.
    Fatal,
    /// Something we couldn't recover from and can't explain without
    /// leaking internals. Always renders as a generic "unexpected error"
    /// in the UI.
    Internal,
}

/// Error payload suitable for the SSE `error` channel.
///
/// Construct via the `*_factory*` helpers or the typed `From` impls in
/// this module. The `String` fields are *user-visible*; callers must
/// ensure they've been written for humans.
#[derive(Debug, Clone, Serialize)]
pub struct UserFacingError {
    pub title: String,
    pub detail: Option<String>,
    pub kind: UserFacingErrorKind,
}

impl UserFacingError {
    /// Generic "something went wrong" — the catch-all for internal errors
    /// that cannot be surfaced without leaking `Debug` output. The real
    /// error is expected to be logged at `warn`/`error` by the caller.
    pub fn internal() -> Self {
        Self {
            title: "Unexpected error".to_string(),
            detail: Some(
                "Phoenix encountered an internal error handling this conversation. Check \
                 the server logs for details, or try again."
                    .to_string(),
            ),
            kind: UserFacingErrorKind::Internal,
        }
    }

    /// Retryable — something temporary that the user can reasonably retry.
    pub fn retryable(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: Some(detail.into()),
            kind: UserFacingErrorKind::Retryable,
        }
    }

    /// Fatal — the user needs to take action outside Phoenix (fix auth,
    /// restart the process, etc.).
    pub fn fatal(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: Some(detail.into()),
            kind: UserFacingErrorKind::Fatal,
        }
    }

    /// Expose a short reason ("resume LLM request", "persist message") for
    /// diagnostic UI without leaking internal types. Both title and detail
    /// are written in human language.
    pub fn with_action(action: impl Into<String>) -> Self {
        let action = action.into();
        Self {
            title: format!("Could not {action}"),
            detail: Some(
                "Phoenix could not complete that operation. Check the server logs, then \
                 retry or start a new conversation."
                    .to_string(),
            ),
            kind: UserFacingErrorKind::Internal,
        }
    }

    /// Flatten to a single string for the legacy `message` field the UI
    /// currently reads. Keep this narrow — it exists so the SSE JSON can
    /// still surface a `message` key while the full typed payload sits
    /// alongside it.
    pub fn flat_message(&self) -> String {
        match &self.detail {
            Some(d) => format!("{}: {}", self.title, d),
            None => self.title.clone(),
        }
    }
}

/// Map a `TransitionError` to a user-visible error *without* exposing its
/// `Debug` representation. Internally the full error is still useful for
/// logging — callers should log it separately before constructing the UI
/// payload.
///
/// This is deliberately a plain function rather than `impl From<..>` so
/// that every call site explicitly opts into the lossy mapping — no
/// accidental `?` conversion from `TransitionError` to `UserFacingError`.
pub fn from_transition_error(err: &TransitionError) -> UserFacingError {
    // EVERY variant is enumerated so that adding a new TransitionError
    // forces a compile error here, instead of silently falling through to
    // a generic message and leaking a Debug string somewhere else.
    match err {
        TransitionError::AgentBusy => UserFacingError::retryable(
            "Agent is busy",
            "Wait for the current response to finish, then try again.",
        ),
        TransitionError::CancellationInProgress => UserFacingError::retryable(
            "Cancellation in progress",
            "A previous cancel is still settling. Try again in a moment.",
        ),
        TransitionError::ContextExhausted => UserFacingError::fatal(
            "Context window exhausted",
            "This conversation has reached the model's context limit. Start a new \
             conversation to continue.",
        ),
        TransitionError::AwaitingTaskApproval => UserFacingError::retryable(
            "Conversation is awaiting task approval",
            "Approve or abandon the proposed task before sending a new message.",
        ),
        TransitionError::AwaitingUserResponse => UserFacingError::retryable(
            "Conversation is awaiting your response",
            "Answer the agent's pending question before sending a new message.",
        ),
        TransitionError::ConversationTerminal => UserFacingError::fatal(
            "Conversation already finished",
            "This conversation has been completed or abandoned. Start a new one to \
             continue.",
        ),
        // The default failure mode for unhandled (state, event) pairs in
        // the state machine. The `String` payload contains a Debug-format
        // dump of internal types — never expose it to the UI. The
        // *internal* error should still be logged at the call site.
        TransitionError::InvalidTransition(_) => UserFacingError::internal(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_variant_does_not_expose_debug_format() {
        let err = TransitionError::InvalidTransition(
            "No transition from Idle with event UserCancel { reason: None }".to_string(),
        );
        let user = from_transition_error(&err);
        assert!(!user.flat_message().contains("UserCancel"));
        assert!(!user.flat_message().contains("Idle"));
        assert!(!user.flat_message().contains("{"));
        assert_eq!(user.kind, UserFacingErrorKind::Internal);
    }

    #[test]
    fn agent_busy_is_retryable() {
        let user = from_transition_error(&TransitionError::AgentBusy);
        assert_eq!(user.kind, UserFacingErrorKind::Retryable);
        assert!(user.title.contains("busy"));
    }

    #[test]
    fn context_exhausted_is_fatal() {
        let user = from_transition_error(&TransitionError::ContextExhausted);
        assert_eq!(user.kind, UserFacingErrorKind::Fatal);
    }
}
