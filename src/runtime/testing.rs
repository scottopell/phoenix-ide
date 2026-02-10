//! Mock implementations for testing
//!
//! These mocks enable integration testing without real I/O.

use super::traits::*;
use crate::db::{Message, MessageContent, MessageType, UsageData};
use crate::llm::ModelRegistry;
use crate::llm::{LlmError, LlmRequest, LlmResponse, ToolDefinition};
use crate::state_machine::ConvState;
use crate::tools::browser::BrowserSessionManager;
use crate::tools::{ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

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
    async fn execute(&self, name: &str, input: Value, _ctx: ToolContext) -> Option<ToolOutput> {
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
// Delayed Mock LLM Client (for cancellation testing)
// ============================================================================

use std::time::Duration;
use tokio::sync::Notify;

/// Mock LLM client with configurable delay (for testing cancellation)
pub struct DelayedMockLlmClient {
    inner: MockLlmClient,
    delay: Duration,
    /// Notified when request starts (for test synchronization)
    pub request_started: Arc<Notify>,
}

impl DelayedMockLlmClient {
    pub fn new(model_id: impl Into<String>, delay: Duration) -> Self {
        Self {
            inner: MockLlmClient::new(model_id),
            delay,
            request_started: Arc::new(Notify::new()),
        }
    }

    pub fn queue_response(&self, response: LlmResponse) {
        self.inner.queue_response(response);
    }
}

#[async_trait]
impl LlmClient for DelayedMockLlmClient {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.inner.requests.lock().unwrap().push(request.clone());
        self.request_started.notify_waiters();
        tokio::time::sleep(self.delay).await;
        self.inner
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(LlmError::network("No mock response queued")))
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
}

// ============================================================================
// Delayed Mock Tool Executor (for cancellation testing)
// ============================================================================

/// Mock tool executor with configurable delay
pub struct DelayedMockToolExecutor {
    inner: MockToolExecutor,
    delay: Duration,
    /// Notified when execution starts
    pub execution_started: Arc<Notify>,
}

impl DelayedMockToolExecutor {
    pub fn new(delay: Duration) -> Self {
        Self {
            inner: MockToolExecutor::new(),
            delay,
            execution_started: Arc::new(Notify::new()),
        }
    }

    pub fn with_tool(mut self, name: impl Into<String>, output: ToolOutput) -> Self {
        self.inner = self.inner.with_tool(name, output);
        self
    }
}

#[async_trait]
impl ToolExecutor for DelayedMockToolExecutor {
    async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput> {
        self.inner
            .executions
            .lock()
            .unwrap()
            .push((name.to_string(), input));
        self.execution_started.notify_waiters();

        // Race between delay and cancellation
        tokio::select! {
            _ = tokio::time::sleep(self.delay) => {
                self.inner.outputs.get(name).cloned()
            }
            _ = ctx.cancel.cancelled() => {
                Some(ToolOutput::error("[command cancelled]"))
            }
        }
    }

    fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions()
    }
}

// ============================================================================
// In-Memory Storage
// ============================================================================

/// In-memory storage for testing
#[allow(dead_code)]
pub struct InMemoryStorage {
    messages: Mutex<HashMap<String, Vec<Message>>>,
    states: Mutex<HashMap<String, ConvState>>,
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
    pub fn get_current_state(&self, conv_id: &str) -> Option<ConvState> {
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
        message_id: &str,
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        let mut id_guard = self.next_msg_id.lock().unwrap();
        #[allow(clippy::cast_possible_wrap)]
        let seq_id = *id_guard as i64;
        *id_guard += 1;
        drop(id_guard);

        let msg = Message {
            message_id: message_id.to_string(),
            conversation_id: conv_id.to_string(),
            sequence_id: seq_id,
            message_type: content.message_type(),
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

    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String> {
        let messages = self.messages.lock().unwrap();
        for msgs in messages.values() {
            for msg in msgs {
                if msg.message_id == message_id {
                    return Ok(msg.clone());
                }
            }
        }
        Err(format!("Message not found: {message_id}"))
    }

    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<(), String> {
        let mut messages = self.messages.lock().unwrap();
        for msgs in messages.values_mut() {
            for msg in msgs.iter_mut() {
                if msg.message_id == message_id {
                    msg.display_data = Some(display_data.clone());
                    return Ok(());
                }
            }
        }
        Err(format!("Message not found: {message_id}"))
    }
}

#[async_trait]
impl StateStore for InMemoryStorage {
    async fn update_state(&self, conv_id: &str, state: &ConvState) -> Result<(), String> {
        self.states
            .lock()
            .unwrap()
            .insert(conv_id.to_string(), state.clone());
        Ok(())
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        Ok(self
            .states
            .lock()
            .unwrap()
            .get(conv_id)
            .cloned()
            .unwrap_or_default())
    }
}

// ============================================================================
// Test Runtime Builder
// ============================================================================

use crate::runtime::{ConversationRuntime, SseEvent};
use crate::state_machine::{ConvContext, Event};
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};

/// Helper for building test runtimes with minimal boilerplate
pub struct TestRuntime<L: LlmClient + 'static, T: ToolExecutor + 'static> {
    pub storage: Arc<InMemoryStorage>,
    pub event_tx: mpsc::Sender<Event>,
    pub broadcast_rx: broadcast::Receiver<SseEvent>,
    pub llm: Arc<L>,
    pub tools: Arc<T>,
    _runtime_handle: tokio::task::JoinHandle<()>,
}

impl TestRuntime<MockLlmClient, MockToolExecutor> {
    /// Create a simple test runtime with instant mocks
    pub fn new() -> TestRuntimeBuilder<MockLlmClient, MockToolExecutor> {
        TestRuntimeBuilder::new()
    }
}

pub struct TestRuntimeBuilder<L, T> {
    conv_id: String,
    working_dir: PathBuf,
    llm: Option<L>,
    tools: Option<T>,
}

impl<L: LlmClient + 'static, T: ToolExecutor + 'static> TestRuntimeBuilder<L, T> {
    pub fn llm(mut self, llm: L) -> Self {
        self.llm = Some(llm);
        self
    }

    pub fn tools(mut self, tools: T) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn conv_id(mut self, id: impl Into<String>) -> Self {
        self.conv_id = id.into();
        self
    }
}

impl TestRuntimeBuilder<MockLlmClient, MockToolExecutor> {
    pub fn new() -> Self {
        Self {
            conv_id: "test-conv".to_string(),
            working_dir: PathBuf::from("/tmp"),
            llm: None,
            tools: None,
        }
    }

    pub fn build(self) -> TestRuntime<MockLlmClient, MockToolExecutor> {
        let storage = Arc::new(InMemoryStorage::new());
        let llm = Arc::new(self.llm.unwrap_or_else(|| MockLlmClient::new("test-model")));
        let tools = Arc::new(self.tools.unwrap_or_else(MockToolExecutor::new));

        let context = ConvContext::new(&self.conv_id, self.working_dir, "test-model");
        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(128);

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm.clone(),
            tools.clone(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(ModelRegistry::new_empty()),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        let handle = tokio::spawn(async move {
            runtime.run().await;
        });

        TestRuntime {
            storage,
            event_tx,
            broadcast_rx,
            llm,
            tools,
            _runtime_handle: handle,
        }
    }
}

impl Default for TestRuntimeBuilder<MockLlmClient, MockToolExecutor> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L: LlmClient + 'static, T: ToolExecutor + 'static> TestRuntime<L, T> {
    /// Send user message to the runtime
    pub async fn send_message(&self, text: &str) {
        self.event_tx
            .send(Event::UserMessage {
                text: text.to_string(),
                images: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
            })
            .await
            .expect("Failed to send message");
    }

    /// Send cancel event
    pub async fn send_cancel(&self) {
        self.event_tx
            .send(Event::UserCancel)
            .await
            .expect("Failed to send cancel");
    }

    /// Wait for AgentDone event with timeout
    pub async fn wait_for_done(&mut self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), self.broadcast_rx.recv()).await {
                Ok(Ok(SseEvent::AgentDone)) => return true,
                Ok(Ok(_)) => continue,
                _ => continue,
            }
        }
        false
    }

    /// Wait for a specific state type with timeout
    pub async fn wait_for_state(&mut self, expected_type: &str, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), self.broadcast_rx.recv()).await {
                Ok(Ok(SseEvent::StateChange { state })) => {
                    if let Some(state_type) = state.get("type").and_then(|v| v.as_str()) {
                        if state_type == expected_type {
                            return true;
                        }
                    }
                }
                Ok(Ok(_)) => continue,
                _ => continue,
            }
        }
        false
    }

    /// Get all messages from storage
    pub fn messages(&self) -> Vec<Message> {
        self.storage.get_all_messages("test-conv")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, Usage};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_context() -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            PathBuf::from("/tmp"),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(ModelRegistry::new_empty()),
        )
    }

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
            .execute("bash", serde_json::json!({ "cmd": "ls" }), test_context())
            .await;
        assert!(result.is_some());
        assert!(result.unwrap().success);

        let result = executor
            .execute("unknown", serde_json::json!({}), test_context())
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_storage() {
        let storage = InMemoryStorage::new();

        let msg = storage
            .add_message(
                "test-message-id",
                "conv-1",
                &MessageContent::user("hello"),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(msg.message_id, "test-message-id");
        assert_eq!(msg.message_type, MessageType::User);

        let messages = storage.get_messages("conv-1").await.unwrap();
        assert_eq!(messages.len(), 1);

        // Verify typed content
        match &messages[0].content {
            MessageContent::User(u) => assert_eq!(u.text, "hello"),
            _ => panic!("Expected User content"),
        }
    }

    /// Integration test: simple text response using builder
    #[tokio::test]
    async fn test_simple_text_response() {
        let llm = MockLlmClient::new("test-model");
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("Hello!")],
            end_turn: true,
            usage: Usage::default(),
        });

        let mut rt = TestRuntime::new().llm(llm).build();
        rt.send_message("Hi").await;

        assert!(rt.wait_for_done(Duration::from_secs(2)).await);

        let msgs = rt.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].message_type, MessageType::User);
        assert_eq!(msgs[1].message_type, MessageType::Agent);
    }

    /// Integration test: tool execution cycle
    #[tokio::test]
    async fn test_tool_execution_cycle() {
        use crate::llm::ContentBlock;

        let llm = MockLlmClient::new("test-model");
        // First response: tool call
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"command": "ls"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });
        // Second response: text after tool
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("Done!")],
            end_turn: true,
            usage: Usage::default(),
        });

        let tools = MockToolExecutor::new().with_tool("bash", ToolOutput::success("file1\nfile2"));

        let mut rt = TestRuntime::new().llm(llm).tools(tools).build();
        rt.send_message("List files").await;

        assert!(rt.wait_for_done(Duration::from_secs(2)).await);

        let msgs = rt.messages();
        // User + Agent(tool_use) + Tool(result) + Agent(text)
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].message_type, MessageType::User);
        assert_eq!(msgs[1].message_type, MessageType::Agent);
        assert_eq!(msgs[2].message_type, MessageType::Tool);
        assert_eq!(msgs[3].message_type, MessageType::Agent);
    }

    /// Integration test: LLM error triggers error state
    #[tokio::test]
    async fn test_llm_error_handling() {
        let llm = MockLlmClient::new("test-model");
        llm.queue_error(LlmError::auth("Invalid API key"));

        let mut rt = TestRuntime::new().llm(llm).build();
        rt.send_message("Hi").await;

        // Should transition to error state
        assert!(rt.wait_for_state("error", Duration::from_secs(2)).await);
    }

    /// Integration test: cancel during LLM request (REQ-BED-005)
    ///
    /// LLM requests are spawned as background tasks and can be cancelled
    /// immediately via CancellationToken.
    #[tokio::test]
    async fn test_cancel_during_llm_request() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::{broadcast, mpsc};

        // Use a longer delay - we'll cancel before it completes
        let llm = Arc::new(DelayedMockLlmClient::new(
            "test-model",
            Duration::from_secs(5),
        ));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("Response that should be discarded")],
            end_turn: true,
            usage: Usage::default(),
        });

        let storage = Arc::new(InMemoryStorage::new());
        let tools = Arc::new(MockToolExecutor::new());
        let request_started = llm.request_started.clone();

        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model");
        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, mut broadcast_rx) = broadcast::channel(128);

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(ModelRegistry::new_empty()),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        let start = tokio::time::Instant::now();

        // Send user message
        event_tx
            .send(Event::UserMessage {
                text: "Hello".to_string(),
                images: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
            })
            .await
            .unwrap();

        // Wait for LLM request to start
        tokio::time::timeout(Duration::from_secs(1), request_started.notified())
            .await
            .expect("LLM request should start");

        // Cancel immediately after request starts
        event_tx.send(Event::UserCancel).await.unwrap();

        // Wait for idle state (cancellation complete)
        let mut done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await {
                Ok(Ok(SseEvent::AgentDone)) => {
                    done = true;
                    break;
                }
                Ok(Ok(SseEvent::StateChange { state })) => {
                    if state.get("type").and_then(|v| v.as_str()) == Some("idle") {
                        done = true;
                        break;
                    }
                }
                _ => continue,
            }
        }

        let elapsed = start.elapsed();

        assert!(done, "Should complete");
        // Should complete in < 1 second, not wait for the 5 second LLM delay
        assert!(
            elapsed < Duration::from_secs(2),
            "Cancellation should be fast, took {:?}",
            elapsed
        );

        // Should only have user message - LLM response was discarded
        let msgs = storage.get_all_messages("test-conv");
        assert_eq!(
            msgs.len(),
            1,
            "Should only have user message, got {:?}",
            msgs
        );
        assert_eq!(msgs[0].message_type, MessageType::User);
    }

    /// Integration test: cancel during tool execution (REQ-BED-005)
    ///
    /// Tools are spawned as background tasks and can be cancelled immediately.
    #[tokio::test]
    async fn test_cancel_during_tool_execution() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::{broadcast, mpsc};

        // Fast LLM, long tool delay that we'll cancel
        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hi"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });
        // This response won't be used since tool is cancelled
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("Done")],
            end_turn: true,
            usage: Usage::default(),
        });

        let tools = Arc::new(
            DelayedMockToolExecutor::new(Duration::from_secs(5))
                .with_tool("bash", ToolOutput::success("hi")),
        );
        let execution_started = tools.execution_started.clone();

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model");
        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, mut broadcast_rx) = broadcast::channel(128);

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(ModelRegistry::new_empty()),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        let start = tokio::time::Instant::now();

        // Send user message
        event_tx
            .send(Event::UserMessage {
                text: "Run command".to_string(),
                images: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
            })
            .await
            .unwrap();

        // Wait for tool execution to start
        tokio::time::timeout(Duration::from_secs(2), execution_started.notified())
            .await
            .expect("Tool execution should start");

        // Cancel immediately
        event_tx.send(Event::UserCancel).await.unwrap();

        // Wait for AgentDone
        let mut done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await {
                Ok(Ok(SseEvent::AgentDone)) => {
                    done = true;
                    break;
                }
                _ => continue,
            }
        }

        let elapsed = start.elapsed();

        assert!(done, "Should complete");
        // Should complete in < 1 second, not wait for the 5 second tool delay
        assert!(
            elapsed < Duration::from_secs(2),
            "Cancellation should be fast, took {:?}",
            elapsed
        );
    }

    /// Integration test: Tool cancellation timing (Task 016)
    ///
    /// Verifies that tool cancellation happens quickly (< 200ms) as required
    /// by REQ-BED-005.
    #[tokio::test]
    async fn test_tool_cancellation_timing() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::{broadcast, mpsc};

        // 5 second tool delay - we should NOT wait for this
        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"command": "sleep 100"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });

        let tools = Arc::new(
            DelayedMockToolExecutor::new(Duration::from_secs(5))
                .with_tool("bash", ToolOutput::success("done")),
        );
        let execution_started = tools.execution_started.clone();

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model");
        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, mut broadcast_rx) = broadcast::channel(128);

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(ModelRegistry::new_empty()),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        // Send user message to trigger tool execution
        event_tx
            .send(Event::UserMessage {
                text: "Run slow command".to_string(),
                images: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
            })
            .await
            .unwrap();

        // Wait for tool execution to start
        tokio::time::timeout(Duration::from_secs(2), execution_started.notified())
            .await
            .expect("Tool execution should start");

        // Small delay to ensure tool is running
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Record time before cancel
        let cancel_start = tokio::time::Instant::now();

        // Send cancel
        event_tx.send(Event::UserCancel).await.unwrap();

        // Wait for AgentDone event
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        let mut agent_done = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(10), broadcast_rx.recv()).await {
                Ok(Ok(SseEvent::AgentDone)) => {
                    agent_done = true;
                    break;
                }
                _ => continue,
            }
        }

        let cancel_elapsed = cancel_start.elapsed();

        assert!(agent_done, "Should receive AgentDone event");
        assert!(
            cancel_elapsed < Duration::from_millis(200),
            "Cancellation should complete in < 200ms, took {:?}",
            cancel_elapsed
        );
    }

    /// Test that state machine cancel logic produces synthetic results
    /// (tests the state machine directly, not through runtime)
    #[tokio::test]
    async fn test_state_machine_cancel_produces_synthetic_results() {
        use crate::state_machine::state::{BashInput, BashMode, ToolCall, ToolInput};
        use crate::state_machine::{transition, Effect};
        use std::path::PathBuf;

        let context = ConvContext::new("test", PathBuf::from("/tmp"), "model");

        // State: executing tool with 2 more remaining
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "t1",
                ToolInput::Bash(BashInput {
                    command: "cmd1".to_string(),
                    mode: BashMode::Default,
                }),
            ),
            remaining_tools: vec![
                ToolCall::new(
                    "t2",
                    ToolInput::Bash(BashInput {
                        command: "cmd2".to_string(),
                        mode: BashMode::Default,
                    }),
                ),
                ToolCall::new(
                    "t3",
                    ToolInput::Bash(BashInput {
                        command: "cmd3".to_string(),
                        mode: BashMode::Default,
                    }),
                ),
            ],
            persisted_tool_ids: HashSet::new(),
            pending_sub_agents: vec![],
        };

        // Phase 1: UserCancel -> CancellingTool with AbortTool
        let result = transition(&state, &context, Event::UserCancel).unwrap();

        assert!(
            matches!(result.new_state, ConvState::CancellingTool { .. }),
            "Should go to CancellingTool"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::AbortTool { .. })),
            "Should have AbortTool effect"
        );

        // Phase 2: ToolAborted -> Idle with synthetic results
        let result2 = transition(
            &result.new_state,
            &context,
            Event::ToolAborted {
                tool_use_id: "t1".to_string(),
            },
        )
        .unwrap();

        assert!(matches!(result2.new_state, ConvState::Idle));

        // Should have PersistToolResults effect with 3 synthetic results
        let persist = result2
            .effects
            .iter()
            .find(|e| matches!(e, Effect::PersistToolResults { .. }));
        assert!(persist.is_some(), "Should have PersistToolResults effect");

        if let Some(Effect::PersistToolResults { results }) = persist {
            assert_eq!(results.len(), 3, "Should have results for all 3 tools");
            assert!(
                results.iter().all(|r| !r.success),
                "All should be marked as failed/cancelled"
            );
        }
    }

    // ========================================================================
    // Sub-Agent Integration Tests
    // ========================================================================

    /// Test sub-agent terminal tool: submit_result transitions to Completed
    #[tokio::test]
    async fn test_subagent_submit_result_transitions_to_completed() {
        use crate::state_machine::state::{SubmitResultInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Effect, Event};
        use std::path::PathBuf;

        // Create sub-agent context
        let context = ConvContext::sub_agent("sub-agent-1", PathBuf::from("/tmp"), "test-model");

        // Start from LlmRequesting
        let state = ConvState::LlmRequesting { attempt: 1 };

        // LLM returns submit_result
        let submit_result_call = ToolCall::new(
            "tool-1",
            ToolInput::SubmitResult(SubmitResultInput {
                result: "Found 3 bugs".to_string(),
            }),
        );

        let event = Event::LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "submit_result",
                serde_json::json!({ "result": "Found 3 bugs" }),
            )],
            tool_calls: vec![submit_result_call],
            end_turn: true,
            usage: Usage::default(),
        };

        let result = transition(&state, &context, event).unwrap();

        // Should transition to Completed
        match &result.new_state {
            ConvState::Completed { result } => {
                assert_eq!(result, "Found 3 bugs");
            }
            other => panic!("Expected Completed, got {:?}", other),
        }

        // Should have NotifyParent effect
        let notify = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::NotifyParent { .. }));
        assert!(notify, "Should have NotifyParent effect");
    }

    /// Test sub-agent terminal tool: submit_error transitions to Failed
    #[tokio::test]
    async fn test_subagent_submit_error_transitions_to_failed() {
        use crate::state_machine::state::{SubmitErrorInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Effect, Event};
        use std::path::PathBuf;

        let context = ConvContext::sub_agent("sub-agent-1", PathBuf::from("/tmp"), "test-model");

        let state = ConvState::LlmRequesting { attempt: 1 };

        let submit_error_call = ToolCall::new(
            "tool-1",
            ToolInput::SubmitError(SubmitErrorInput {
                error: "File not found".to_string(),
            }),
        );

        let event = Event::LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "submit_error",
                serde_json::json!({ "error": "File not found" }),
            )],
            tool_calls: vec![submit_error_call],
            end_turn: true,
            usage: Usage::default(),
        };

        let result = transition(&state, &context, event).unwrap();

        // Should transition to Failed
        match &result.new_state {
            ConvState::Failed { error, error_kind } => {
                assert_eq!(error, "File not found");
                assert!(matches!(error_kind, crate::db::ErrorKind::SubAgentError));
            }
            other => panic!("Expected Failed, got {:?}", other),
        }

        // Should have NotifyParent effect
        let notify = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::NotifyParent { .. }));
        assert!(notify, "Should have NotifyParent effect");
    }

    /// Test sub-agent cancellation: UserCancel transitions to Failed
    #[tokio::test]
    async fn test_subagent_cancel_transitions_to_failed() {
        use crate::state_machine::{transition, ConvContext, Effect, Event};
        use std::path::PathBuf;

        let context = ConvContext::sub_agent("sub-agent-1", PathBuf::from("/tmp"), "test-model");

        // Can be in various states when cancelled
        let states = [
            ConvState::Idle,
            ConvState::LlmRequesting { attempt: 1 },
            ConvState::AwaitingLlm,
        ];

        for state in states {
            let result = transition(&state, &context, Event::UserCancel).unwrap();

            match &result.new_state {
                ConvState::Failed { error, error_kind } => {
                    assert!(error.contains("Cancelled"));
                    assert!(matches!(error_kind, crate::db::ErrorKind::Cancelled));
                }
                other => panic!("Expected Failed from {:?}, got {:?}", state, other),
            }

            // Should have NotifyParent effect
            let notify = result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. }));
            assert!(
                notify,
                "Should have NotifyParent effect for cancel from {:?}",
                state
            );
        }
    }

    /// Test terminal tool validation: must be sole tool in response
    #[tokio::test]
    async fn test_subagent_terminal_tool_must_be_alone() {
        use crate::state_machine::state::{
            BashInput, BashMode, SubmitResultInput, ToolCall, ToolInput,
        };
        use crate::state_machine::transition::TransitionError;
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        let context = ConvContext::sub_agent("sub-agent-1", PathBuf::from("/tmp"), "test-model");

        let state = ConvState::LlmRequesting { attempt: 1 };

        // Two tools, one of which is terminal
        let bash_call = ToolCall::new(
            "tool-1",
            ToolInput::Bash(BashInput {
                command: "ls".to_string(),
                mode: BashMode::Default,
            }),
        );
        let submit_call = ToolCall::new(
            "tool-2",
            ToolInput::SubmitResult(SubmitResultInput {
                result: "done".to_string(),
            }),
        );

        let event = Event::LlmResponse {
            content: vec![
                ContentBlock::tool_use("tool-1", "bash", serde_json::json!({ "command": "ls" })),
                ContentBlock::tool_use(
                    "tool-2",
                    "submit_result",
                    serde_json::json!({ "result": "done" }),
                ),
            ],
            tool_calls: vec![bash_call, submit_call],
            end_turn: true,
            usage: Usage::default(),
        };

        let result = transition(&state, &context, event);

        // Should be rejected
        assert!(matches!(result, Err(TransitionError::InvalidTransition(_))));
    }

    /// Test that parent conversations don't handle terminal tools specially
    #[tokio::test]
    async fn test_parent_ignores_terminal_tools() {
        use crate::state_machine::state::{SubmitResultInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        // Parent context (not sub-agent)
        let context = ConvContext::new("parent-conv", PathBuf::from("/tmp"), "test-model");

        let state = ConvState::LlmRequesting { attempt: 1 };

        // Same terminal tool, but for parent
        let submit_call = ToolCall::new(
            "tool-1",
            ToolInput::SubmitResult(SubmitResultInput {
                result: "done".to_string(),
            }),
        );

        let event = Event::LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "submit_result",
                serde_json::json!({ "result": "done" }),
            )],
            tool_calls: vec![submit_call],
            end_turn: true,
            usage: Usage::default(),
        };

        let result = transition(&state, &context, event).unwrap();

        // Parent should go to ToolExecuting, not Completed
        assert!(
            matches!(result.new_state, ConvState::ToolExecuting { .. }),
            "Parent should go to ToolExecuting, got {:?}",
            result.new_state
        );
    }

    /// Test sub-agent result buffering (early completion)
    #[tokio::test]
    async fn test_subagent_result_buffering() {
        use crate::runtime::ConversationRuntime;
        use crate::state_machine::state::SubAgentOutcome;
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::{broadcast, mpsc};

        // Set up a parent runtime
        let llm = Arc::new(MockLlmClient::new("test-model"));
        // First response: spawn_agents tool
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("I'll spawn sub-agents")],
            end_turn: true,
            usage: Usage::default(),
        });

        let tools = Arc::new(MockToolExecutor::new());
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("parent-conv", PathBuf::from("/tmp"), "test-model");
        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, _broadcast_rx) = broadcast::channel(128);

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(ModelRegistry::new_empty()),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        // Send a SubAgentResult while parent is still in Idle
        // (simulates early completion)
        event_tx
            .send(Event::SubAgentResult {
                agent_id: "sub-1".to_string(),
                outcome: SubAgentOutcome::Success {
                    result: "early result".to_string(),
                },
            })
            .await
            .unwrap();

        // Give it time to process (should be buffered)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The event should have been received without error
        // (buffered since parent isn't in AwaitingSubAgents)
        // This is a basic smoke test - full integration would require more setup
    }
}
