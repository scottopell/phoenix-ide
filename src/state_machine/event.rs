//! Events that can occur in a conversation

use crate::db::{ErrorKind, SubAgentResult, ToolResult};
use crate::llm::{ContentBlock, ImageSource, Usage};
use crate::state_machine::state::ToolCall;
use serde::{Deserialize, Serialize};

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
        end_turn: bool,
        usage: Usage,
    },
    LlmError {
        message: String,
        error_kind: ErrorKind,
        attempt: u32,
    },
    RetryTimeout {
        attempt: u32,
    },
    
    // Tool events
    ToolComplete {
        tool_use_id: String,
        result: ToolResult,
    },
    
    // Sub-agent events
    SubAgentResult {
        agent_id: String,
        result: SubAgentResult,
    },
}

/// Image data for user messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub data: String,
    pub media_type: String,
}

impl ImageData {
    pub fn to_image_source(&self) -> ImageSource {
        ImageSource::Base64 {
            media_type: self.media_type.clone(),
            data: self.data.clone(),
        }
    }
}
