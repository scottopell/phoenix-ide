//! Events that can occur in a conversation

use crate::db::{ErrorKind, ImageData, ToolResult};
use crate::llm::{ContentBlock, Usage};
use crate::state_machine::state::{PendingSubAgent, SubAgentOutcome, ToolCall};

/// Events that trigger state transitions
#[derive(Debug, Clone)]
pub enum Event {
    // User events
    UserMessage {
        text: String,
        images: Vec<ImageData>,
        /// Client-generated UUID - the canonical identifier for this message
        message_id: String,
        /// Browser user agent for display (e.g., show iPhone icon in UI)
        user_agent: Option<String>,
    },
    UserCancel,

    // LLM events
    LlmResponse {
        content: Vec<ContentBlock>,
        /// Tool calls extracted from the content
        tool_calls: Vec<ToolCall>,
        #[allow(dead_code)] // Reserved for conversation flow control
        end_turn: bool,
        usage: Usage,
    },
    LlmError {
        message: String,
        error_kind: ErrorKind,
        #[allow(dead_code)] // Reserved for retry tracking
        attempt: u32,
    },
    /// LLM request was aborted due to cancellation
    LlmAborted,
    RetryTimeout {
        attempt: u32,
    },

    // Tool events
    ToolComplete {
        tool_use_id: String,
        result: ToolResult,
    },
    /// Tool was aborted due to cancellation
    ToolAborted {
        tool_use_id: String,
    },

    // Sub-agent events
    /// `spawn_agents` tool completed, sub-agents are now running
    SpawnAgentsComplete {
        tool_use_id: String,
        /// Normal tool result for LLM context
        result: ToolResult,
        /// Spawned sub-agents with their tasks
        spawned: Vec<PendingSubAgent>,
    },
    /// A sub-agent has completed (success or failure)
    SubAgentResult {
        agent_id: String,
        outcome: SubAgentOutcome,
    },

    // Context continuation events (REQ-BED-019 through REQ-BED-024)
    /// Continuation summary received from LLM
    ContinuationResponse {
        summary: String,
    },
    /// Continuation request failed after retries
    ContinuationFailed {
        error: String,
    },
    /// User manually triggered continuation (REQ-BED-023)
    UserTriggerContinuation,
}
