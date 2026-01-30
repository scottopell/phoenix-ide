//! Effects produced by state transitions

use crate::db::{MessageType, ToolResult, UsageData};
use crate::state_machine::state::ToolCall;
use serde_json::Value;
use std::time::Duration;

/// Effects to be executed after state transition
#[derive(Debug, Clone)]
pub enum Effect {
    /// Persist a message to the database
    PersistMessage {
        msg_type: MessageType,
        content: Value,
        display_data: Option<Value>,
        usage_data: Option<UsageData>,
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

    /// Spawn a sub-agent
    #[allow(dead_code)] // Reserved for sub-agent feature
    SpawnSubAgent {
        agent_id: String,
        prompt: String,
        model: String,
    },

    /// Notify connected clients
    NotifyClient { event_type: String, data: Value },

    /// Schedule a retry
    ScheduleRetry { delay: Duration, attempt: u32 },

    /// Persist multiple tool results at once
    PersistToolResults { results: Vec<ToolResult> },
}

impl Effect {
    pub fn persist_user_message(content: Value) -> Self {
        Effect::PersistMessage {
            msg_type: MessageType::User,
            content,
            display_data: None,
            usage_data: None,
        }
    }

    pub fn persist_agent_message(content: Value, usage: Option<UsageData>) -> Self {
        Effect::PersistMessage {
            msg_type: MessageType::Agent,
            content,
            display_data: None,
            usage_data: usage,
        }
    }

    pub fn persist_tool_message(content: Value, display_data: Option<Value>) -> Self {
        Effect::PersistMessage {
            msg_type: MessageType::Tool,
            content,
            display_data,
            usage_data: None,
        }
    }

    #[allow(clippy::needless_pass_by_value)] // data is consumed by json! macro
    pub fn notify_state_change(state: &str, data: Value) -> Self {
        Effect::NotifyClient {
            event_type: "state_change".to_string(),
            data: serde_json::json!({
                "state": state,
                "state_data": data
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
