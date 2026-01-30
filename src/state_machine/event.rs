//! Events that can occur in a conversation

use crate::db::{ErrorKind, ImageData, SubAgentResult, ToolResult};
use crate::llm::{ContentBlock, Usage};
use crate::state_machine::state::ToolCall;

/// Events that trigger state transitions
#[derive(Debug, Clone)]
pub enum Event {
    // User events
    UserMessage {
        text: String,
        images: Vec<ImageData>,
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
    #[allow(dead_code)] // Reserved for sub-agent feature
    SubAgentResult {
        agent_id: String,
        result: SubAgentResult,
    },
}


