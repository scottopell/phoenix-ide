//! Mock implementations for testing
//!
//! These mocks enable integration testing without real I/O.

use super::traits::*;
use crate::db::{Message, MessageType, UsageData};
use crate::llm::{LlmError, LlmRequest, LlmResponse, ToolDefinition};
use crate::state_machine::ConvState;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

// ============================================================================
// Mock LLM Client
// ============================================================================

/// Mock LLM client that returns queued responses
#[allow(dead_code)]
pub struct MockLlmClient {
    responses: Mutex<VecDeque<Result<LlmResponse, LlmError>>>,
    model_id: String,
    /// Record of all requests made
    pub requests: Mutex<Vec<LlmRequest>>,
}

#[allow(dead_code)]
impl MockLlmClient {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            model_id: model_id.into(),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Queue a successful response
    pub fn queue_response(&self, response: LlmResponse) {
        self.responses.lock().unwrap().push_back(Ok(response));
    }

    /// Queue an error response
    pub fn queue_error(&self, error: LlmError) {
        self.responses.lock().unwrap().push_back(Err(error));
    }

    /// Get recorded requests
    pub fn recorded_requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(LlmError::network("No mock response queued")))
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

// ============================================================================
// Mock Tool Executor
// ============================================================================

/// Mock tool executor with predefined outputs
#[allow(dead_code)]
pub struct MockToolExecutor {
    outputs: HashMap<String, ToolOutput>,
    definitions: Vec<ToolDefinition>,
    /// Record of tool executions
    pub executions: Mutex<Vec<(String, Value)>>,
}

#[allow(dead_code)]
impl MockToolExecutor {
    pub fn new() -> Self {
        Self {
            outputs: HashMap::new(),
            definitions: Vec::new(),
            executions: Mutex::new(Vec::new()),
        }
    }

    /// Add a tool with a predefined output
    pub fn with_tool(mut self, name: impl Into<String>, output: ToolOutput) -> Self {
        let name = name.into();
        self.definitions.push(ToolDefinition {
            name: name.clone(),
            description: format!("Mock {name}"),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        });
        self.outputs.insert(name, output);
        self
    }

    /// Get recorded executions
    pub fn recorded_executions(&self) -> Vec<(String, Value)> {
        self.executions.lock().unwrap().clone()
    }
}

impl Default for MockToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(&self, name: &str, input: Value) -> Option<ToolOutput> {
        self.executions
            .lock()
            .unwrap()
            .push((name.to_string(), input));
        self.outputs.get(name).cloned()
    }

    fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions.clone()
    }
}

// ============================================================================
// In-Memory Storage
// ============================================================================

/// In-memory storage for testing
#[allow(dead_code)]
pub struct InMemoryStorage {
    messages: Mutex<HashMap<String, Vec<Message>>>,
    states: Mutex<HashMap<String, (crate::db::ConversationState, Option<Value>)>>,
    next_msg_id: Mutex<u64>,
}

#[allow(dead_code)]
impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            messages: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            next_msg_id: Mutex::new(1),
        }
    }

    /// Get all messages for a conversation
    pub fn get_all_messages(&self, conv_id: &str) -> Vec<Message> {
        self.messages
            .lock()
            .unwrap()
            .get(conv_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Get current state for a conversation
    pub fn get_current_state(
        &self,
        conv_id: &str,
    ) -> Option<(crate::db::ConversationState, Option<Value>)> {
        self.states.lock().unwrap().get(conv_id).cloned()
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MessageStore for InMemoryStorage {
    async fn add_message(
        &self,
        conv_id: &str,
        msg_type: MessageType,
        content: &Value,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        let mut id_guard = self.next_msg_id.lock().unwrap();
        let msg_num = *id_guard;
        let id = format!("msg-{msg_num}");
        #[allow(clippy::cast_possible_wrap)]
        let seq_id = msg_num as i64;
        *id_guard += 1;
        drop(id_guard);

        let msg = Message {
            id: id.clone(),
            conversation_id: conv_id.to_string(),
            sequence_id: seq_id,
            message_type: msg_type,
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: chrono::Utc::now(),
        };

        self.messages
            .lock()
            .unwrap()
            .entry(conv_id.to_string())
            .or_default()
            .push(msg.clone());

        Ok(msg)
    }

    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String> {
        Ok(self.get_all_messages(conv_id))
    }
}

#[async_trait]
impl StateStore for InMemoryStorage {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &crate::db::ConversationState,
        state_data: Option<&Value>,
    ) -> Result<(), String> {
        self.states
            .lock()
            .unwrap()
            .insert(conv_id.to_string(), (state.clone(), state_data.cloned()));
        Ok(())
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        // Convert db state to state machine state
        let (db_state, _) = self
            .states
            .lock()
            .unwrap()
            .get(conv_id)
            .cloned()
            .unwrap_or((crate::db::ConversationState::Idle, None));

        Ok(db_state_to_conv_state(&db_state))
    }
}

#[allow(dead_code)]
fn db_state_to_conv_state(db_state: &crate::db::ConversationState) -> ConvState {
    match db_state {
        crate::db::ConversationState::Idle => ConvState::Idle,
        crate::db::ConversationState::AwaitingLlm => ConvState::AwaitingLlm,
        crate::db::ConversationState::LlmRequesting { attempt } => {
            ConvState::LlmRequesting { attempt: *attempt }
        }
        crate::db::ConversationState::ToolExecuting {
            current_tool,
            remaining_tools,
            completed_results,
        } => ConvState::ToolExecuting {
            current_tool: current_tool.clone(),
            remaining_tools: remaining_tools.clone(),
            completed_results: completed_results.clone(),
        },
        crate::db::ConversationState::Cancelling { pending_tool_id } => ConvState::Cancelling {
            pending_tool_id: pending_tool_id.clone(),
        },
        crate::db::ConversationState::AwaitingSubAgents {
            pending_ids,
            completed_results,
        } => ConvState::AwaitingSubAgents {
            pending_ids: pending_ids.clone(),
            completed_results: completed_results.clone(),
        },
        crate::db::ConversationState::Error {
            message,
            error_kind,
        } => ConvState::Error {
            message: message.clone(),
            error_kind: error_kind.clone(),
        },
    }
}

// ============================================================================
// Tests for Mocks
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, Usage};

    #[tokio::test]
    async fn test_mock_llm_client() {
        let mock = MockLlmClient::new("test-model");
        mock.queue_response(LlmResponse {
            content: vec![ContentBlock::text("Hello")],
            end_turn: true,
            usage: Usage::default(),
        });

        let request = LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            max_tokens: Some(100),
        };

        let response = mock.complete(&request).await.unwrap();
        assert_eq!(response.content.len(), 1);
        assert!(response.end_turn);

        // Second call should fail (no more responses)
        let result = mock.complete(&request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_tool_executor() {
        let executor = MockToolExecutor::new().with_tool("bash", ToolOutput::success("output"));

        let result = executor
            .execute("bash", serde_json::json!({ "cmd": "ls" }))
            .await;
        assert!(result.is_some());
        assert!(result.unwrap().success);

        let result = executor.execute("unknown", serde_json::json!({})).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_storage() {
        let storage = InMemoryStorage::new();

        let msg = storage
            .add_message(
                "conv-1",
                MessageType::User,
                &serde_json::json!({ "text": "hello" }),
                None,
                None,
            )
            .await
            .unwrap();

        assert!(msg.id.starts_with("msg-"));

        let messages = storage.get_messages("conv-1").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content["text"], "hello");
    }
}
