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
        /// The canonical message identifier (client-generated for user messages,
        /// server-generated for agent/tool messages)
        message_id: String,
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
    PersistSubAgentResults {
        results: Vec<SubAgentResult>,
        /// `tool_use_id` of `spawn_agents` call - used to update its `display_data`
        spawn_tool_id: Option<String>,
    },
}

impl Effect {
    pub fn persist_user_message(
        text: impl Into<String>,
        images: Vec<ImageData>,
        message_id: String,
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
            message_id,
        }
    }

    pub fn persist_agent_message(blocks: Vec<ContentBlock>, usage: Option<UsageData>) -> Self {
        Effect::PersistMessage {
            content: MessageContent::agent(blocks),
            display_data: None,
            usage_data: usage,
            message_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn persist_tool_message(
        tool_use_id: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
        display_data: Option<Value>,
    ) -> Self {
        let tool_use_id = tool_use_id.into();
        // Use predictable message_id so we can update display_data later (e.g., subagent results)
        let message_id = format!("{tool_use_id}-result");
        Effect::PersistMessage {
            content: MessageContent::tool(tool_use_id, output, is_error),
            display_data,
            usage_data: None,
            message_id,
        }
    }

    /// Create a `state_change` notification with the state as an object
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
