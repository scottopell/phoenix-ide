//! Typed outcome enums for the effect channel system.
//!
//! Each outcome type is exhaustive -- no `Unknown`, no `_ =>` match arms.
//! Adding a new variant is a compile error at every handler site.
//!
//! These types flow through oneshot channels: a `Sender<ToolExecOutcome>` physically
//! cannot send an `LlmOutcome`. The executor wraps received outcomes in
//! `EffectOutcome` before passing to `handle_outcome()`.

use crate::db::ToolResult;
use crate::llm::{ContentBlock, Usage};
use crate::state_machine::state::{SubAgentOutcome, ToolCall};
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
    },
    /// Rate limited (429) — retryable
    RateLimited {
        #[allow(dead_code)] // Populated when provider sends Retry-After header
        retry_after: Option<Duration>,
    },
    /// Server error (5xx) — retryable
    ServerError { status: u16, body: String },
    /// Network/connection error — retryable
    NetworkError { message: String },
    /// Token budget exceeded
    TokenBudgetExceeded,
    /// Authentication error (401/403) — non-retryable.
    /// `recovery_in_progress` is true when a credential helper is actively running.
    AuthError {
        message: String,
        recovery_in_progress: bool,
    },
    /// Request rejected (400, content filter, etc.) — non-retryable
    RequestRejected { message: String },
    /// Request was cancelled (abort signal received)
    #[allow(dead_code)] // Used when LLM abort is migrated to typed channels
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
    /// Execution time exceeded limit
    #[allow(dead_code)] // Will be used when timeout support is added
    Timeout,
    /// Parent conversation was cancelled
    #[allow(dead_code)] // Will be used when sub-agent cancellation uses typed channels
    ParentCancelled,
}

// ============================================================================
// Persist Outcome — returned by executor persistence task via oneshot channel
// ============================================================================

/// Outcome of a persistence operation, sent through a typed oneshot channel.
#[derive(Debug)]
#[allow(dead_code)] // Defined for architectural completeness; executor migrates incrementally
pub enum PersistOutcome {
    /// Persistence succeeded
    Ok,
    /// Persistence failed
    Failed { error: String },
}

// ============================================================================
// SpawnAgents Outcome — returned by executor spawn task
// ============================================================================

/// Outcome of a `spawn_agents` tool execution. This is handled synchronously
/// in the executor (not via oneshot), but the type constrains what can be produced.
#[derive(Debug)]
#[allow(dead_code)] // Defined for architectural completeness; spawn_agents still synchronous
pub enum SpawnOutcome {
    /// Agents were spawned successfully, tool result and pending agents returned
    Spawned {
        tool_use_id: String,
        result: ToolResult,
        spawned: Vec<crate::state_machine::state::PendingSubAgent>,
    },
    /// Spawning failed (returns error as a `ToolResult`)
    Failed {
        tool_use_id: String,
        result: ToolResult,
    },
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
    /// Sub-agent completed (arrives via event channel, wrapped here for `handle_outcome`)
    #[allow(dead_code)] // Sub-agent results still flow through event channel for now
    SubAgent {
        agent_id: String,
        outcome: SubAgentOutcome,
    },
    /// Persistence completed
    #[allow(dead_code)] // Persistence effects are still synchronous for now
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
