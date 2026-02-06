//! Effects produced by state transitions

use crate::db::{ImageData, MessageContent, ToolResult, UsageData};
use crate::llm::ContentBlock;
use crate::state_machine::state::{SubAgentOutcome, SubAgentResult, ToolCall};
use serde_json::Value;
use std::time::Duration;

/// Effects to be executed after state transition
#[derive(Debug, Clone)]
pub enum Effect {
    /// Persist a message to the database
    PersistMessage {
        content: MessageContent,
        display_data: Option<Value>,
        usage_data: Option<UsageData>,
        /// Client-generated UUID for idempotency (user messages only)
        local_id: Option<String>,
        /// User agent string for UI display (user messages only)
        /// Stored in display_data by persist_user_message, not read directly from here
        #[allow(dead_code)]
        user_agent: Option<String>,
    },

    /// Persist the new state
    PersistState,

    /// Make an LLM request
    RequestLlm,

    /// Execute a tool (spawns as background task)
    ExecuteTool { tool: ToolCall },

    /// Abort the currently running tool
    AbortTool { tool_use_id: String },

    /// Abort the currently running LLM request
    AbortLlm,

    /// Cancel all pending sub-agents
    CancelSubAgents { ids: Vec<String> },

    /// Notify parent of sub-agent completion (sub-agent only)
    NotifyParent { outcome: SubAgentOutcome },

    /// Notify connected clients
    NotifyClient { event_type: String, data: Value },

    /// Schedule a retry
    ScheduleRetry { delay: Duration, attempt: u32 },

    /// Persist multiple tool results at once
    PersistToolResults { results: Vec<ToolResult> },

    /// Persist aggregated sub-agent results as a message
    PersistSubAgentResults { results: Vec<SubAgentResult> },
}

impl Effect {
    pub fn persist_user_message(
        text: impl Into<String>,
        images: Vec<ImageData>,
        local_id: String,
        user_agent: Option<String>,
    ) -> Self {
        let content = if images.is_empty() {
            MessageContent::user(text)
        } else {
            MessageContent::user_with_images(text, images)
        };
        // Store user_agent in display_data for UI to show device icon
        let display_data = user_agent.map(|ua| serde_json::json!({ "user_agent": ua }));
        Effect::PersistMessage {
            content,
            display_data,
            usage_data: None,
            local_id: Some(local_id),
            user_agent: None, // Already in display_data
        }
    }

    pub fn persist_agent_message(blocks: Vec<ContentBlock>, usage: Option<UsageData>) -> Self {
        Effect::PersistMessage {
            content: MessageContent::agent(blocks),
            display_data: None,
            usage_data: usage,
            local_id: None,
            user_agent: None,
        }
    }

    pub fn persist_tool_message(
        tool_use_id: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
        display_data: Option<Value>,
    ) -> Self {
        Effect::PersistMessage {
            content: MessageContent::tool(tool_use_id, output, is_error),
            display_data,
            usage_data: None,
            local_id: None,
            user_agent: None,
        }
    }

    /// Create a state_change notification with the state as an object
    /// This merges the state type with the additional data into a single object
    #[allow(clippy::needless_pass_by_value)] // data is consumed by json! macro
    pub fn notify_state_change(state_type: &str, mut data: Value) -> Self {
        // Merge type into the data object to create a state-like structure
        if let Some(obj) = data.as_object_mut() {
            obj.insert("type".to_string(), serde_json::json!(state_type));
        }
        Effect::NotifyClient {
            event_type: "state_change".to_string(),
            data: serde_json::json!({
                "state": data
            }),
        }
    }

    #[allow(dead_code)] // Constructor for API completeness
    pub fn notify_message(message: Value) -> Self {
        Effect::NotifyClient {
            event_type: "message".to_string(),
            data: message,
        }
    }

    pub fn notify_agent_done() -> Self {
        Effect::NotifyClient {
            event_type: "agent_done".to_string(),
            data: Value::Null,
        }
    }

    pub fn execute_tool(tool: ToolCall) -> Self {
        Effect::ExecuteTool { tool }
    }
}
