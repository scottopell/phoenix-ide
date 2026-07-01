//! Typed outcome enums for the effect channel system.
//!
//! Each outcome type is exhaustive -- no `Unknown`, no `_ =>` match arms.
//! Adding a new variant is a compile error at every handler site.
//!
//! These types flow through oneshot channels: a `Sender<ToolExecOutcome>` physically
//! cannot send an `LlmOutcome`. The executor wraps received outcomes in
//! `EffectOutcome` before passing to `handle_outcome()`.

use crate::state::{SubAgentOutcome, ToolCall};
use phoenix_core::domain::db_schema::ToolResult;
use phoenix_core::domain::llm_types::{ContentBlock, Usage};
use phoenix_core::domain::quota_details::QuotaDetails;
use std::time::Duration;

// ============================================================================
// LLM Outcome — returned by executor LLM task via oneshot channel
// ============================================================================

/// Outcome of an LLM request, sent through a typed oneshot channel.
#[derive(Debug)]
pub enum LlmOutcome {
    /// LLM responded successfully
    Response {
        content: Vec<ContentBlock>,
        tool_calls: Vec<ToolCall>,
        end_turn: bool,
        usage: Usage,
        /// Server-generated request id from the LLM dispatch. Becomes the
        /// `AssistantMessage.message_id` on persistence so that the streaming
        /// `Token` events (which already carry this id) share identity with
        /// the eventual finalized message. Lets the UI key the in-flight
        /// streaming view by the same value as the eventual `agent_turn`
        /// render unit — a same-key in-place transition rather than a
        /// cross-region key swap.
        request_id: String,
    },
    /// Transient rate-limit throttle (429) — retryable
    RateLimited {
        #[allow(dead_code)] // Populated when provider sends Retry-After header
        retry_after: Option<Duration>,
        /// Quota window reset timestamp from the upstream `QuotaDetails`,
        /// when the 429 response included one. Threaded through to
        /// `Event::LlmError.resets_at` → `Effect::ScheduleRetry.resets_at`
        /// → `SseEvent::LlmAttempt.resets_at` so the client can surface
        /// "(retry K/N after rate limit, resets at HH:MM)" — specs/llm-retry-visibility/.
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Quota window exhausted (codex backend 429 with `usage_limit_reached`) — terminal.
    /// `details` carries plan + reset + windows; `message` is the pre-rendered
    /// plan-aware string ready for display.
    UsageLimitReached {
        #[allow(dead_code)] // Persisted to message JSON in a follow-up task
        details: QuotaDetails,
        message: String,
    },
    /// Server error (5xx) — retryable
    ServerError { status: u16, body: String },
    /// Provider returned bytes we could not parse or understand (malformed SSE
    /// event, unparseable body, unexpected content-block shape) — retryable.
    InvalidResponse { message: String },
    /// Selected model is at capacity (`server_is_overloaded` / `slow_down`) — terminal,
    /// suggest a different model.
    ServerOverloaded { message: String },
    /// Network/connection error — retryable
    NetworkError { message: String },
    /// Token budget exceeded
    TokenBudgetExceeded,
    /// Model exhausted its output token budget before completing — non-retryable but resumable.
    OutputLimitExceeded { message: String },
    /// Authentication error (401/403) — non-retryable.
    /// `recovery_in_progress` is true when a credential helper is actively running.
    AuthError {
        message: String,
        recovery_in_progress: bool,
    },
    /// Request rejected (400, content filter, etc.) — non-retryable
    RequestRejected { message: String },
    /// Request was cancelled (abort signal received).
    #[allow(dead_code)]
    Cancelled,
}

// ============================================================================
// Tool Outcome — returned by executor tool task via oneshot channel
// ============================================================================

/// Outcome of a tool execution, sent through a typed oneshot channel.
#[derive(Debug)]
pub enum ToolExecOutcome {
    /// Tool ran to completion with a result
    Completed(ToolResult),
    /// Tool was aborted before completion
    Aborted {
        tool_use_id: String,
        #[allow(dead_code)] // Logged for diagnostics, not consumed by state machine yet
        reason: AbortReason,
    },
    /// Tool execution failed (e.g., unknown tool)
    Failed { tool_use_id: String, error: String },
}

/// Why a tool was aborted. Set by the component requesting cancellation,
/// never inferred from output content (FM-1 prevention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    /// User explicitly cancelled
    CancellationRequested,
}

// ============================================================================
// Persist Outcome — returned by executor persistence task via oneshot channel
// ============================================================================

/// Outcome of a persistence operation, sent through a typed oneshot channel.
#[derive(Debug)]
#[allow(dead_code)]
pub enum PersistOutcome {
    /// Persistence succeeded
    Ok,
    /// Persistence failed
    Failed { error: String },
}

// ============================================================================
// EffectOutcome — union type for all outcomes the executor can produce
// ============================================================================

/// Union type for all outcomes the executor can produce.
/// The executor constructs this from the typed oneshot channel result.
#[derive(Debug)]
pub enum EffectOutcome {
    /// LLM request completed
    Llm(LlmOutcome),
    /// Tool execution completed
    Tool(ToolExecOutcome),
    /// Sub-agent completed.
    #[allow(dead_code)]
    SubAgent {
        agent_id: String,
        outcome: SubAgentOutcome,
    },
    /// Persistence completed.
    #[allow(dead_code)]
    Persist(PersistOutcome),
    /// Retry timer fired
    RetryTimeout { attempt: u32 },
}

// ============================================================================
// InvalidOutcome — rejected outcomes from handle_outcome
// ============================================================================

/// An outcome that was rejected by `handle_outcome()` because it doesn't
/// make sense in the current state. The executor logs and discards these —
/// state is unchanged.
#[derive(Debug)]
pub struct InvalidOutcome {
    /// Why the outcome was rejected
    pub reason: String,
}

impl std::fmt::Display for InvalidOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid outcome: {}", self.reason)
    }
}

impl std::error::Error for InvalidOutcome {}
