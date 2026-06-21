//! Mock implementations for testing
//!
//! These mocks enable integration testing without real I/O.

use super::traits::*;
use crate::db::{Message, MessageContent, MessageType, UsageData};
use crate::state_machine::ConvState;
use crate::tools::browser::BrowserSessionManager;
use crate::tools::{ToolContext, ToolOutput};
use async_trait::async_trait;
use phoenix_llm::ModelRegistry;
use phoenix_llm::{LlmError, LlmRequest, LlmResponse, PromptCacheKey, ToolDefinition};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
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
// Streaming Mock LLM Client
// ============================================================================

/// Mock streaming LLM client used by the task 24683 regression test.
///
/// Emits a burst of `TokenChunk::Text` events on `complete_streaming` and
/// then returns a single text `LlmResponse`. Combined with the executor's
/// `forwarder_handle.await` barrier, every `SseEvent::Token` should arrive
/// on the outer broadcast channel *before* the resulting `SseEvent::Message`.
#[allow(dead_code)]
pub struct StreamingMockLlmClient {
    model_id: String,
    token_count: usize,
    final_text: String,
}

#[allow(dead_code)]
impl StreamingMockLlmClient {
    pub fn new(token_count: usize, final_text: impl Into<String>) -> Self {
        Self {
            model_id: "streaming-mock".to_string(),
            token_count,
            final_text: final_text.into(),
        }
    }
}

#[async_trait]
impl LlmClient for StreamingMockLlmClient {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        // Not used — complete_streaming is the intended path.
        Err(LlmError::network(
            "StreamingMockLlmClient only supports complete_streaming",
        ))
    }

    async fn complete_streaming(
        &self,
        _request: &LlmRequest,
        chunk_tx: &tokio::sync::broadcast::Sender<phoenix_llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        // Emit the burst. Each send is a fire-and-forget into the broadcast
        // channel; the forwarder task reads and re-broadcasts as
        // `SseEvent::Token`. The count is deliberately large so the forwarder
        // provably has pending work when `complete_streaming` returns.
        for i in 0..self.token_count {
            let _ = chunk_tx.send(phoenix_llm::TokenChunk::Text(format!("chunk{i} ")));
        }
        Ok(LlmResponse {
            content: vec![phoenix_llm::ContentBlock::text(self.final_text.clone())],
            end_turn: true,
            usage: phoenix_llm::Usage::default(),
        })
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
    clearable: std::collections::HashSet<String>,
    /// Record of tool executions
    pub executions: Mutex<Vec<(String, Value)>>,
}

#[allow(dead_code)]
impl MockToolExecutor {
    pub fn new() -> Self {
        Self {
            outputs: HashMap::new(),
            definitions: Vec::new(),
            clearable: std::collections::HashSet::new(),
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
            defer_loading: false,
        });
        self.outputs.insert(name, output);
        self
    }

    /// Mark `name` as a clearable tool (its stale results may be cleared).
    pub fn with_clearable_tool(mut self, name: impl Into<String>) -> Self {
        self.clearable.insert(name.into());
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
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        _ctx: ToolContext,
    ) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        self.executions.lock().unwrap().push((name.clone(), input));
        self.outputs.get(&name).cloned()
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions.clone()
    }

    fn clearable_tool_names(&self) -> std::collections::HashSet<String> {
        self.clearable.clone()
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
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        self.inner
            .executions
            .lock()
            .unwrap()
            .push((name.clone(), input));
        self.execution_started.notify_waiters();

        // Race between delay and cancellation
        tokio::select! {
            () = tokio::time::sleep(self.delay) => {
                self.inner.outputs.get(&name).cloned()
            }
            () = ctx.cancel.cancelled() => {
                Some(ToolOutput::error("[command cancelled]"))
            }
        }
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions().await
    }
}

// ============================================================================
// Uncooperative Mock Tool Executor (for liveness testing)
// ============================================================================

/// Mock tool executor that DELIBERATELY ignores its cancellation token,
/// modelling a tool stuck in a blocking child process or syscall whose
/// `execute()` never returns until externally released.
///
/// Unlike [`DelayedMockToolExecutor`] — which cooperatively races
/// `ctx.cancel.cancelled()` — this executor never selects on the cancel
/// token. It notifies `execution_started` then awaits `release` (or, if
/// `release` is never fired, sleeps a long fixed duration). This pins the
/// liveness invariant: cancelling such a tool must still drive the
/// conversation back to `Idle`.
#[allow(dead_code)]
pub struct UncooperativeMockToolExecutor {
    inner: MockToolExecutor,
    /// Notified when execution starts (mirrors `DelayedMockToolExecutor`).
    pub execution_started: Arc<Notify>,
    /// When notified, lets a stuck `execute()` return. Tests may leave this
    /// un-fired to model a permanently blocked tool.
    pub release: Arc<Notify>,
}

#[allow(dead_code)]
impl UncooperativeMockToolExecutor {
    pub fn new() -> Self {
        Self {
            inner: MockToolExecutor::new(),
            execution_started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }

    pub fn with_tool(mut self, name: impl Into<String>, output: ToolOutput) -> Self {
        self.inner = self.inner.with_tool(name, output);
        self
    }
}

impl Default for UncooperativeMockToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for UncooperativeMockToolExecutor {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        self.inner
            .executions
            .lock()
            .unwrap()
            .push((name.clone(), input));
        self.execution_started.notify_waiters();

        // Deliberately do NOT select on `ctx.cancel.cancelled()`. Hold a
        // reference so clippy doesn't flag the unused field, but never observe
        // it — that is the whole point of "uncooperative".
        let _ignored_cancel = &ctx.cancel;

        // Block until explicitly released, with a long backstop so a forgotten
        // release can't hang the suite indefinitely.
        tokio::select! {
            () = self.release.notified() => {}
            () = tokio::time::sleep(Duration::from_secs(3600)) => {}
        }
        self.inner.outputs.get(&name).cloned()
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions().await
    }
}

// ============================================================================
// First-Call-Uncooperative Tool Executor (for stale-handle QA)
// ============================================================================

/// Tool executor that is uncooperative on its FIRST `execute()` (blocks forever,
/// ignoring its cancel token — modelling a wedged tool) and cooperative on every
/// subsequent call (returns the configured output immediately).
///
/// Used to pin the `tool_task_handle` lifecycle (task 08692, vector 3): after the
/// `CancellingTool` backstop aborts the wedged first task and forces Idle, a new
/// turn's new tool task must NOT be aborted by any stale handle/deadline.
#[allow(dead_code)]
pub struct FirstCallUncooperativeToolExecutor {
    inner: MockToolExecutor,
    call_count: std::sync::atomic::AtomicUsize,
    /// Notified each time `execute()` starts.
    pub execution_started: Arc<Notify>,
    /// Notified each time a cooperative (call >= 2) `execute()` returns.
    pub cooperative_completed: Arc<Notify>,
}

#[allow(dead_code)]
impl FirstCallUncooperativeToolExecutor {
    pub fn new() -> Self {
        Self {
            inner: MockToolExecutor::new(),
            call_count: std::sync::atomic::AtomicUsize::new(0),
            execution_started: Arc::new(Notify::new()),
            cooperative_completed: Arc::new(Notify::new()),
        }
    }

    pub fn with_tool(mut self, name: impl Into<String>, output: ToolOutput) -> Self {
        self.inner = self.inner.with_tool(name, output);
        self
    }
}

impl Default for FirstCallUncooperativeToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for FirstCallUncooperativeToolExecutor {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        use std::sync::atomic::Ordering;
        let (name, input) = call.into_parts();
        self.inner
            .executions
            .lock()
            .unwrap()
            .push((name.clone(), input));
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        self.execution_started.notify_waiters();

        if n == 0 {
            // First call: uncooperative — never observe the token, block on a
            // long backstop so a forgotten test can't hang the suite.
            let _ignored_cancel = &ctx.cancel;
            tokio::time::sleep(Duration::from_secs(3600)).await;
            self.inner.outputs.get(&name).cloned()
        } else {
            // Subsequent calls: cooperative and immediate.
            let out = self.inner.outputs.get(&name).cloned();
            self.cooperative_completed.notify_waiters();
            out
        }
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions().await
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
    modes: Mutex<HashMap<String, crate::db::ConvMode>>,
    next_msg_id: Mutex<u64>,
    steering_queues: Mutex<HashMap<String, Vec<crate::state_machine::event::SteerEntry>>>,
    fork_proposals: Mutex<Vec<crate::db::ForkProposal>>,
    clear_watermarks: Mutex<HashMap<String, i64>>,
    last_prompt_tokens: Mutex<HashMap<String, i64>>,
    // Fault injection for the clearing-assembly failure paths (REQ-STR-007).
    fail_watermark_read: Mutex<bool>,
    fail_watermark_write: Mutex<bool>,
}

#[allow(dead_code)]
impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            messages: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            modes: Mutex::new(HashMap::new()),
            next_msg_id: Mutex::new(1),
            steering_queues: Mutex::new(HashMap::new()),
            fork_proposals: Mutex::new(Vec::new()),
            clear_watermarks: Mutex::new(HashMap::new()),
            last_prompt_tokens: Mutex::new(HashMap::new()),
            fail_watermark_read: Mutex::new(false),
            fail_watermark_write: Mutex::new(false),
        }
    }

    /// Seed the most-recent-turn prompt size (the clearing pressure signal).
    pub fn set_last_prompt_tokens(&self, conv_id: &str, tokens: i64) {
        self.last_prompt_tokens
            .lock()
            .unwrap()
            .insert(conv_id.to_string(), tokens);
    }

    /// Make `get_clear_watermark` return an error (test the read-failure path).
    pub fn set_fail_watermark_read(&self, fail: bool) {
        *self.fail_watermark_read.lock().unwrap() = fail;
    }

    /// Make `set_clear_watermark` return an error (test the write-failure path).
    pub fn set_fail_watermark_write(&self, fail: bool) {
        *self.fail_watermark_write.lock().unwrap() = fail;
    }

    /// Read back the persisted fork proposals (test-only).
    pub fn get_fork_proposals(&self) -> Vec<crate::db::ForkProposal> {
        self.fork_proposals.lock().unwrap().clone()
    }

    /// Read back the persisted steering queue for a conversation (test-only).
    pub fn get_steering_queue(
        &self,
        conv_id: &str,
    ) -> Vec<crate::state_machine::event::SteerEntry> {
        self.steering_queues
            .lock()
            .unwrap()
            .get(conv_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Seed the `conv_mode` for a conversation (used by tests that need to
    /// exercise mode-aware effect handlers like `NotifyContextExhausted`).
    pub fn set_mode(&self, conv_id: &str, mode: crate::db::ConvMode) {
        self.modes.lock().unwrap().insert(conv_id.to_string(), mode);
    }

    /// Read back the stored `conv_mode` (test-only).
    pub fn get_mode(&self, conv_id: &str) -> Option<crate::db::ConvMode> {
        self.modes.lock().unwrap().get(conv_id).cloned()
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

    async fn add_message_with_seq(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        // Keep the monotonic id counter at least as high as the provided
        // seq so any subsequent `add_message` call produces a strictly
        // greater id. Mirrors DB.add_message_with_seq semantics.
        {
            let mut id_guard = self.next_msg_id.lock().unwrap();
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let floor = (sequence_id as u64).saturating_add(1);
            if *id_guard < floor {
                *id_guard = floor;
            }
        }

        let msg = Message {
            message_id: message_id.to_string(),
            conversation_id: conv_id.to_string(),
            sequence_id,
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

    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_at(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Message, String> {
        // Mirror add_message_with_seq's seq-floor bump.
        {
            let mut id_guard = self.next_msg_id.lock().unwrap();
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let floor = (sequence_id as u64).saturating_add(1);
            if *id_guard < floor {
                *id_guard = floor;
            }
        }

        let msg = Message {
            message_id: message_id.to_string(),
            conversation_id: conv_id.to_string(),
            sequence_id,
            message_type: content.message_type(),
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at,
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

    async fn message_exists(&self, message_id: &str) -> Result<bool, String> {
        let messages = self.messages.lock().unwrap();
        Ok(messages
            .values()
            .any(|msgs| msgs.iter().any(|m| m.message_id == message_id)))
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

    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<(), String> {
        let mut messages = self.messages.lock().unwrap();
        for msgs in messages.values_mut() {
            for msg in msgs.iter_mut() {
                if msg.message_id == message_id {
                    if let crate::db::MessageContent::Tool(ref mut tool) = msg.content {
                        tool.content = content.to_string();
                        return Ok(());
                    }
                    return Err(format!("Message {message_id} is not a tool message"));
                }
            }
        }
        Err(format!("Message not found: {message_id}"))
    }

    async fn persist_fork_proposal_with_tool_round(
        &self,
        origin_conv_id: &str,
        assistant: &Message,
        tool_results: &[Message],
        proposal: &crate::db::ForkProposal,
    ) -> Result<(), String> {
        {
            let mut messages = self.messages.lock().unwrap();
            let bucket = messages.entry(origin_conv_id.to_string()).or_default();
            bucket.push(assistant.clone());
            for msg in tool_results {
                bucket.push(msg.clone());
            }
        }
        self.fork_proposals.lock().unwrap().push(proposal.clone());
        Ok(())
    }

    async fn persist_tool_round(
        &self,
        conv_id: &str,
        assistant: &Message,
        tool_results: &[Message],
    ) -> Result<(), String> {
        let mut messages = self.messages.lock().unwrap();
        let bucket = messages.entry(conv_id.to_string()).or_default();
        bucket.push(assistant.clone());
        for msg in tool_results {
            bucket.push(msg.clone());
        }
        Ok(())
    }
}

#[async_trait]
impl StateStore for InMemoryStorage {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        _state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String> {
        // In-memory test storage tracks only the state value, not its entry
        // timestamp, so the threaded stamp is intentionally unused here.
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

    async fn update_conversation_mode(
        &self,
        conv_id: &str,
        mode: &crate::db::ConvMode,
    ) -> Result<(), String> {
        self.modes
            .lock()
            .unwrap()
            .insert(conv_id.to_string(), mode.clone());
        Ok(())
    }

    async fn get_conversation_mode(&self, conv_id: &str) -> Result<crate::db::ConvMode, String> {
        Ok(self
            .modes
            .lock()
            .unwrap()
            .get(conv_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn update_conversation_cwd_recovery_only(
        &self,
        _conv_id: &str,
        _cwd: &str,
    ) -> Result<(), String> {
        // In-memory storage doesn't track cwd separately
        Ok(())
    }

    async fn get_clear_watermark(&self, conv_id: &str) -> Result<i64, String> {
        if *self.fail_watermark_read.lock().unwrap() {
            return Err("injected watermark read failure".to_string());
        }
        Ok(self
            .clear_watermarks
            .lock()
            .unwrap()
            .get(conv_id)
            .copied()
            .unwrap_or(0))
    }

    async fn set_clear_watermark(&self, conv_id: &str, watermark: i64) -> Result<(), String> {
        if *self.fail_watermark_write.lock().unwrap() {
            return Err("injected watermark write failure".to_string());
        }
        // Structurally monotonic, mirroring the production `MAX(...)` write: a
        // value below the stored watermark never regresses it (REQ-STR-007).
        let mut map = self.clear_watermarks.lock().unwrap();
        let entry = map.entry(conv_id.to_string()).or_insert(0);
        *entry = (*entry).max(watermark);
        Ok(())
    }

    async fn get_last_turn_prompt_tokens(&self, conv_id: &str) -> Result<Option<i64>, String> {
        Ok(self
            .last_prompt_tokens
            .lock()
            .unwrap()
            .get(conv_id)
            .copied())
    }

    async fn insert_turn_usage(
        &self,
        _conversation_id: &str,
        _root_conversation_id: &str,
        _model: &str,
        _usage: &phoenix_llm::Usage,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn update_steering_queue(
        &self,
        conv_id: &str,
        queue: &[crate::state_machine::event::SteerEntry],
    ) -> Result<(), String> {
        self.steering_queues
            .lock()
            .unwrap()
            .insert(conv_id.to_string(), queue.to_vec());
        Ok(())
    }

    async fn remove_steering_entries(
        &self,
        conv_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        if message_ids.is_empty() {
            return Ok(());
        }
        let mut guard = self.steering_queues.lock().unwrap();
        if let Some(queue) = guard.get_mut(conv_id) {
            let to_remove: std::collections::HashSet<&str> =
                message_ids.iter().map(String::as_str).collect();
            queue.retain(|e| !to_remove.contains(e.message_id.as_str()));
        }
        Ok(())
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
    #[allow(dead_code)]
    pub llm: Arc<L>,
    #[allow(dead_code)]
    pub tools: Arc<T>,
    _runtime_handle: tokio::task::JoinHandle<()>,
}

impl TestRuntime<MockLlmClient, MockToolExecutor> {
    /// Create a simple test runtime with instant mocks
    #[allow(clippy::new_ret_no_self)]
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

    #[allow(dead_code)]
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
        let tools = Arc::new(self.tools.unwrap_or_default());

        let context = ConvContext::new(&self.conv_id, self.working_dir, "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcaster = crate::runtime::SseBroadcaster::new(128, 0);
        let broadcast_rx = broadcaster.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm.clone(),
            tools.clone(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcaster,
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
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .expect("Failed to send message");
    }

    /// Send cancel event
    #[allow(dead_code)]
    pub async fn send_cancel(&self) {
        self.event_tx
            .send(Event::UserCancel { reason: None })
            .await
            .expect("Failed to send cancel");
    }

    /// Wait for `AgentDone` event with timeout
    pub async fn wait_for_done(&mut self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), self.broadcast_rx.recv()).await
            {
                return true;
            }
        }
        false
    }

    /// Wait for a specific state type with timeout
    pub async fn wait_for_state(&mut self, expected_type: &str, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::StateChange { state, .. })) =
                tokio::time::timeout(Duration::from_millis(50), self.broadcast_rx.recv()).await
            {
                if let Ok(val) = serde_json::to_value(&state) {
                    if let Some(state_type) = val.get("type").and_then(|v| v.as_str()) {
                        if state_type == expected_type {
                            return true;
                        }
                    }
                }
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
    use phoenix_llm::{ContentBlock, Usage};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_context() -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            PathBuf::from("/tmp"),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            Arc::new(crate::tools::TmuxRegistry::new()),
            None,
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
            cache_key: PromptCacheKey::ephemeral(),
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
        use crate::runtime::deny_gate::CheckedToolCall;
        let executor = MockToolExecutor::new().with_tool("bash", ToolOutput::success("output"));

        let result = executor
            .execute(
                CheckedToolCall::cleared_for_test("bash", serde_json::json!({ "cmd": "ls" })),
                test_context(),
            )
            .await;
        assert!(result.is_some());
        assert!(result.unwrap().is_success());

        let result = executor
            .execute(
                CheckedToolCall::cleared_for_test("unknown", serde_json::json!({})),
                test_context(),
            )
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
        use phoenix_llm::ContentBlock;

        let llm = MockLlmClient::new("test-model");
        // First response: tool call
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "ls"}),
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
    /// immediately via `CancellationToken`.
    #[tokio::test]
    async fn test_cancel_during_llm_request() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

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

        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
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
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        // Wait for LLM request to start
        tokio::time::timeout(Duration::from_secs(1), request_started.notified())
            .await
            .expect("LLM request should start");

        // Cancel immediately after request starts
        event_tx
            .send(Event::UserCancel { reason: None })
            .await
            .unwrap();

        // Wait for idle state (cancellation complete)
        let mut done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await {
                Ok(Ok(SseEvent::AgentDone { .. })) => {
                    done = true;
                    break;
                }
                Ok(Ok(SseEvent::StateChange { state, .. })) => {
                    if matches!(state, crate::state_machine::ConvState::Idle) {
                        done = true;
                        break;
                    }
                }
                _ => {}
            }
        }

        let elapsed = start.elapsed();

        assert!(done, "Should complete");
        // Should complete in < 1 second, not wait for the 5 second LLM delay
        assert!(
            elapsed < Duration::from_secs(2),
            "Cancellation should be fast, took {elapsed:?}"
        );

        // Should only have user message - LLM response was discarded
        let msgs = storage.get_all_messages("test-conv");
        assert_eq!(msgs.len(), 1, "Should only have user message, got {msgs:?}");
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
        use tokio::sync::mpsc;

        // Fast LLM, long tool delay that we'll cancel
        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "echo hi"}),
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
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
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
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        // Wait for tool execution to start
        tokio::time::timeout(Duration::from_secs(2), execution_started.notified())
            .await
            .expect("Tool execution should start");

        // Cancel immediately
        event_tx
            .send(Event::UserCancel { reason: None })
            .await
            .unwrap();

        // Wait for AgentDone
        let mut done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                done = true;
                break;
            }
        }

        let elapsed = start.elapsed();

        assert!(done, "Should complete");
        // Should complete in < 1 second, not wait for the 5 second tool delay
        assert!(
            elapsed < Duration::from_secs(2),
            "Cancellation should be fast, took {elapsed:?}"
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
        use tokio::sync::mpsc;

        // 5 second tool delay - we should NOT wait for this
        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "sleep 100"}),
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
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        // Send user message to trigger tool execution
        event_tx
            .send(Event::UserMessage {
                text: "Run slow command".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
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
        event_tx
            .send(Event::UserCancel { reason: None })
            .await
            .unwrap();

        // Wait for AgentDone event
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        let mut agent_done = false;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(10), broadcast_rx.recv()).await
            {
                agent_done = true;
                break;
            }
        }

        let cancel_elapsed = cancel_start.elapsed();

        assert!(agent_done, "Should receive AgentDone event");
        assert!(
            cancel_elapsed < Duration::from_millis(200),
            "Cancellation should complete in < 200ms, took {cancel_elapsed:?}"
        );
    }

    /// Liveness invariant (task 08692, P0): cancelling a conversation whose
    /// running tool ignores its cancellation token MUST still drive the
    /// conversation back to `Idle` (and emit `AgentDone`).
    ///
    /// `UncooperativeMockToolExecutor::execute()` never returns until released,
    /// modelling a tool blocked in a child process/syscall. Against current
    /// code this wedges in `CancellingTool` forever: `Effect::AbortTool` only
    /// flips the cooperative token and the spawned tool task checks
    /// `is_cancelled()` *after* `execute().await` returns — which it never does.
    ///
    /// Timing approach: a REAL bounded `tokio::time::timeout(3s)`, not a paused
    /// clock. The runtime runs on a spawned background task and the mock parks
    /// on a 3600s backstop sleep; a paused clock would require the test to
    /// drive `advance` deterministically across that spawned task, which does
    /// not compose cleanly here. The real timeout fails fast (≤3s) against
    /// current code and never hangs the suite.
    #[tokio::test]
    async fn cancel_with_uncooperative_tool_still_reaches_idle() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "sleep 100"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });

        let tools = Arc::new(
            UncooperativeMockToolExecutor::new().with_tool("bash", ToolOutput::success("done")),
        );
        let execution_started = tools.execution_started.clone();

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        event_tx
            .send(Event::UserMessage {
                text: "Run blocking command".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        // Wait for the uncooperative tool to start executing.
        tokio::time::timeout(Duration::from_secs(2), execution_started.notified())
            .await
            .expect("Tool execution should start");

        // Cancel while the tool is wedged.
        event_tx
            .send(Event::UserCancel { reason: None })
            .await
            .unwrap();

        // Liveness assertion: AgentDone within a bounded deadline. The executor's
        // cancellation backstop (CANCELLATION_DEADLINE) is 3s; this assertion
        // window is deliberately longer so the backstop fires strictly before the
        // test gives up — the backstop deadline is the spec'd 3s, not this wait.
        let mut agent_done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                agent_done = true;
                break;
            }
        }

        assert!(
            agent_done,
            "Cancelling an uncooperative tool must still reach Idle / emit AgentDone, \
             but the conversation stayed wedged in CancellingTool"
        );

        // And the persisted state must be Idle, not stuck mid-cancel.
        let final_state = storage.get_current_state("test-conv");
        assert!(
            matches!(final_state, Some(ConvState::Idle)),
            "Conversation should return to Idle after cancel, got {final_state:?}"
        );
    }

    /// REQ-BED-005a `CancellingSubAgentsDeadlineFires`: a parent wedged in
    /// `CancellingSubAgents` because a cancelled sub-agent never reported back
    /// must still reach `Idle` within the bounded cancellation deadline.
    ///
    /// This mirrors the incident: the parent enters `CancellingSubAgents` with a
    /// pending sub-agent, but no `SubAgentResult` ever arrives. The runtime is
    /// constructed directly in `CancellingSubAgents` (no event drives a result),
    /// so the only path to Idle is the liveness backstop. The assertion window is
    /// deliberately longer than the 3s `CANCELLATION_DEADLINE` so the backstop
    /// fires strictly before the test gives up.
    #[tokio::test]
    async fn cancelling_sub_agents_with_silent_sub_agent_still_reaches_idle() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::state::{PendingSubAgent, SubAgentMode};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx = mpsc::channel::<Event>(32).0;
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let initial_state = ConvState::CancellingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "sub-1".to_string(),
                task: "do thing".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
        };

        let runtime = ConversationRuntime::new(
            context,
            initial_state,
            storage.clone(),
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx,
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        // Liveness assertion: AgentDone within a bounded deadline. The backstop
        // is CANCELLATION_DEADLINE (3s); this window is longer so it fires first.
        let mut agent_done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                agent_done = true;
                break;
            }
        }

        assert!(
            agent_done,
            "A parent wedged in CancellingSubAgents with a silent sub-agent must still \
             reach Idle / emit AgentDone via the liveness backstop"
        );

        let final_state = storage.get_current_state("test-conv");
        assert!(
            matches!(final_state, Some(ConvState::Idle)),
            "Conversation should return to Idle after the backstop fires, got {final_state:?}"
        );
    }

    /// Test that state machine cancel logic produces synthetic results
    /// (tests the state machine directly, not through runtime)
    #[tokio::test]
    async fn test_state_machine_cancel_produces_synthetic_results() {
        use crate::state_machine::state::{AssistantMessage, ToolCall, ToolInput};
        use crate::state_machine::{transition, CheckpointData, Effect};
        use phoenix_llm::ContentBlock;
        use std::path::PathBuf;

        let context = ConvContext::new("test", PathBuf::from("/tmp"), "model", 200_000);

        // Build AssistantMessage with tool_use blocks for all 3 tools
        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "t2".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "t3".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
            ],
            None,
            None,
        );

        // State: executing tool with 2 more remaining
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "t1",
                ToolInput::Bash(crate::tools::BashToolInput::run("cmd1")),
            ),
            remaining_tools: vec![
                ToolCall::new(
                    "t2",
                    ToolInput::Bash(crate::tools::BashToolInput::run("cmd2")),
                ),
                ToolCall::new(
                    "t3",
                    ToolInput::Bash(crate::tools::BashToolInput::run("cmd3")),
                ),
            ],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message,
        };

        // Phase 1: UserCancel -> CancellingTool with AbortTool
        let result = transition(&state, &context, Event::UserCancel { reason: None }).unwrap();

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

        // Phase 2: ToolAborted -> Idle with PersistCheckpoint (atomic)
        let result2 = transition(
            &result.new_state,
            &context,
            Event::ToolAborted {
                tool_use_id: "t1".to_string(),
            },
        )
        .unwrap();

        assert!(matches!(result2.new_state, ConvState::Idle));

        // Should have PersistCheckpoint effect with 3 synthetic results
        let persist = result2
            .effects
            .iter()
            .find(|e| matches!(e, Effect::PersistCheckpoint { .. }));
        assert!(persist.is_some(), "Should have PersistCheckpoint effect");

        if let Some(Effect::PersistCheckpoint { data }) = persist {
            let CheckpointData::ToolRound { tool_results, .. } = data;
            assert_eq!(tool_results.len(), 3, "Should have results for all 3 tools");
            assert!(
                tool_results.iter().all(|r| !r.is_success()),
                "All should be marked as failed/cancelled"
            );
        }
    }

    // ========================================================================
    // Sub-Agent Integration Tests
    // ========================================================================

    /// Test sub-agent terminal tool: `submit_result` transitions to Completed
    #[tokio::test]
    async fn test_subagent_submit_result_transitions_to_completed() {
        use crate::state_machine::state::{SubmitResultInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Effect, Event};
        use std::path::PathBuf;

        // Create sub-agent context
        let context = ConvContext::sub_agent(
            "sub-agent-1",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "test-root",
        );

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
            request_id: "test-req-id".to_string(),
        };

        let result = transition(&state, &context, event).unwrap();

        // Should transition to Completed
        match &result.new_state {
            ConvState::Completed { result } => {
                assert_eq!(result, "Found 3 bugs");
            }
            other => panic!("Expected Completed, got {other:?}"),
        }

        // Should have NotifyParent effect
        let notify = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::NotifyParent { .. }));
        assert!(notify, "Should have NotifyParent effect");
    }

    /// Test sub-agent terminal tool: `submit_error` transitions to Failed
    #[tokio::test]
    async fn test_subagent_submit_error_transitions_to_failed() {
        use crate::state_machine::state::{SubmitErrorInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Effect, Event};
        use std::path::PathBuf;

        let context = ConvContext::sub_agent(
            "sub-agent-1",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "test-root",
        );

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
            request_id: "test-req-id".to_string(),
        };

        let result = transition(&state, &context, event).unwrap();

        // Should transition to Failed
        match &result.new_state {
            ConvState::Failed { error, error_kind } => {
                assert_eq!(error, "File not found");
                assert!(matches!(error_kind, crate::db::ErrorKind::SubAgentError));
            }
            other => panic!("Expected Failed, got {other:?}"),
        }

        // Should have NotifyParent effect
        let notify = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::NotifyParent { .. }));
        assert!(notify, "Should have NotifyParent effect");
    }

    /// Test sub-agent cancellation: `UserCancel` transitions to Failed
    #[tokio::test]
    async fn test_subagent_cancel_transitions_to_failed() {
        use crate::state_machine::{transition, ConvContext, Effect, Event};
        use std::path::PathBuf;

        let context = ConvContext::sub_agent(
            "sub-agent-1",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "test-root",
        );

        // Can be in various states when cancelled
        let states = [ConvState::Idle, ConvState::LlmRequesting { attempt: 1 }];

        for state in states {
            let result = transition(&state, &context, Event::UserCancel { reason: None }).unwrap();

            match &result.new_state {
                ConvState::Failed { error, error_kind } => {
                    assert!(error.contains("Cancelled"));
                    assert!(matches!(error_kind, crate::db::ErrorKind::Cancelled));
                }
                other => panic!("Expected Failed from {state:?}, got {other:?}"),
            }

            // Should have NotifyParent effect
            let notify = result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. }));
            assert!(
                notify,
                "Should have NotifyParent effect for cancel from {state:?}"
            );
        }
    }

    /// Test terminal tool validation: must be sole tool in response
    #[tokio::test]
    async fn test_subagent_terminal_tool_must_be_alone() {
        use crate::state_machine::state::{SubmitResultInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        let context = ConvContext::sub_agent(
            "sub-agent-1",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "test-root",
        );

        let state = ConvState::LlmRequesting { attempt: 1 };

        // Two tools, one of which is terminal
        let bash_call = ToolCall::new(
            "tool-1",
            ToolInput::Bash(crate::tools::BashToolInput::run("ls")),
        );
        let submit_call = ToolCall::new(
            "tool-2",
            ToolInput::SubmitResult(SubmitResultInput {
                result: "done".to_string(),
            }),
        );

        let event = Event::LlmResponse {
            content: vec![
                ContentBlock::tool_use(
                    "tool-1",
                    "bash",
                    serde_json::json!({ "op": "run", "cmd": "ls" }),
                ),
                ContentBlock::tool_use(
                    "tool-2",
                    "submit_result",
                    serde_json::json!({ "result": "done" }),
                ),
            ],
            tool_calls: vec![bash_call, submit_call],
            end_turn: true,
            usage: Usage::default(),
            request_id: "test-req-id".to_string(),
        };

        let result = transition(&state, &context, event);

        // Should feed error results back to LLM (not transition to Failed)
        let result = result.expect("Should produce Ok transition");
        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { .. }),
            "Should transition back to LlmRequesting to feed errors to LLM, got {:?}",
            result.new_state
        );
        assert!(
            result.effects.iter().any(|e| matches!(
                e,
                crate::state_machine::effect::Effect::PersistCheckpoint { .. }
            )),
            "Should have PersistCheckpoint with error results"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, crate::state_machine::effect::Effect::RequestLlm)),
            "Should have RequestLlm effect"
        );
    }

    /// Test that parent conversations don't handle terminal tools specially
    #[tokio::test]
    async fn test_parent_ignores_terminal_tools() {
        use crate::state_machine::state::{SubmitResultInput, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        // Parent context (not sub-agent)
        let context = ConvContext::new("parent-conv", PathBuf::from("/tmp"), "test-model", 200_000);

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
            request_id: "test-req-id".to_string(),
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
        use tokio::sync::mpsc;

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
        let context = ConvContext::new("parent-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let _broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
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

    /// `spawn_agents` rejects a batch that exceeds the per-call sub-agent cap
    /// (`MAX_SUB_AGENTS_PER_SPAWN`), realising the bedrock `SpawnLimit`
    /// invariant. The whole call is rejected with a tool error — no truncation —
    /// before any spawn request is sent.
    #[tokio::test]
    async fn test_spawn_agents_rejects_over_cap() {
        use crate::runtime::ConversationRuntime;
        use crate::state_machine::state::{SpawnAgentsInput, SubAgentTask, ToolCall, ToolInput};
        use crate::state_machine::{ConvContext, Event};
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let llm = Arc::new(MockLlmClient::new("test-model"));
        let tools = Arc::new(MockToolExecutor::new());
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("parent-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let _broadcast_rx = broadcast_tx.subscribe();

        let mut runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage,
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx,
            broadcast_tx,
        );

        // 11 tasks: one over the cap of 10.
        let tasks: Vec<SubAgentTask> = (0..11)
            .map(|i| SubAgentTask {
                task: format!("task {i}"),
                cwd: None,
                mode: None,
                model: None,
                max_turns: None,
                agent_type: None,
            })
            .collect();
        let call = ToolCall::new(
            "spawn-1",
            ToolInput::SpawnAgents(SpawnAgentsInput { tasks }),
        );

        let event = runtime
            .handle_spawn_agents_tool(call)
            .await
            .expect("handler must not error out");

        match event {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(
                    result.is_error(),
                    "over-cap spawn must produce an error result"
                );
                assert!(
                    result.output().contains("at most 10"),
                    "error should name the cap, got: {}",
                    result.output()
                );
            }
            other => panic!("expected ToolComplete error, got {other:?}"),
        }
    }

    /// Test that tool output containing "[command cancelled]" does NOT trigger `ToolAborted`
    /// when the cancellation token was NOT signaled.
    ///
    /// This is a regression test for a bug where the executor checked the output string
    /// instead of the cancellation token state, causing spurious `ToolAborted` events that
    /// violated the state machine contract (`ToolAborted` is only valid from `CancellingTool`).
    #[tokio::test]
    async fn test_cancelled_output_without_token_sends_tool_complete() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        // LLM returns a single tool call
        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "echo test"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });
        // After tool completes, LLM returns text
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("Done")],
            end_turn: true,
            usage: Usage::default(),
        });

        // Tool executor returns output containing "[command cancelled]" string
        // BUT the cancellation token is NOT signaled (this is the key)
        let tools = Arc::new(MockToolExecutor::new().with_tool(
            "bash",
            ToolOutput::error("[command cancelled]"), // The problematic string!
        ));

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        // Send user message
        event_tx
            .send(Event::UserMessage {
                text: "Run command".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        // Wait for completion with timeout
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut agent_done = false;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                agent_done = true;
                break;
            }
        }

        // Should complete successfully (not get stuck)
        assert!(
            agent_done,
            "Should receive AgentDone - tool output containing '[command cancelled]' \
             without token signal should produce ToolComplete, not ToolAborted"
        );

        // Verify conversation completed normally - check final state via storage
        let messages = storage.get_messages("test-conv").await.unwrap();
        assert!(
            messages.len() >= 3,
            "Should have user message, tool result, and agent response"
        );
    }

    /// Regression test for task 24680: a parent conversation whose LLM
    /// keeps issuing tool calls without ever producing a final answer must
    /// be capped, not loop forever.
    ///
    /// Simulates a stuck provider: every LLM response calls the valid
    /// `bash` tool, the mock tool executor reports success, and the next
    /// LLM turn repeats the same call instead of emitting `end_turn`. The
    /// "valid tool + success" shape is a strictly stronger test than an
    /// "unknown tool" loop — it proves the cap halts *correct* tool usage
    /// that simply never terminates, not just pathological error cases.
    ///
    /// With no cap this would fill the DB; with `parent_tool_cycle_cap = 3`
    /// the runtime halts after 3 completed LLM calls (the 4th attempt trips
    /// the guard), persists a system message, and returns to Idle so the
    /// user can send a follow-up.
    #[tokio::test]
    async fn test_parent_tool_cycle_cap_halts_runaway_loop() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let llm = Arc::new(MockLlmClient::new("test-model"));
        // Queue enough responses to outrun the cap. The cap is 3, so the
        // 4th RequestLlm should trip it. Queue 10 for headroom.
        for _ in 0..10 {
            llm.queue_response(LlmResponse {
                content: vec![ContentBlock::tool_use(
                    "tool-x",
                    "bash",
                    serde_json::json!({ "op": "run", "cmd": "echo loop" }),
                )],
                end_turn: false,
                usage: Usage::default(),
            });
        }

        // Every call reports success — this models the shape of the 24684 bug
        // (tool exists and runs cleanly, LLM just never stops calling it).
        // Note: 24684 was originally 24679 in git history; renumbered during
        // rebase to avoid collision with main's shell-integration task.
        let tools = Arc::new(MockToolExecutor::new().with_tool("bash", ToolOutput::success("ok")));
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new(
            "cap-test-conv",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
        );
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(256, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm.clone(),
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        )
        .with_parent_tool_cycle_cap(3);

        tokio::spawn(async move { runtime.run().await });

        // Send an initial user message. The LLM then tool-loops forever
        // against the cap.
        event_tx
            .send(Event::UserMessage {
                text: "Start looping".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        // Wait for AgentDone, which fires when the runtime halts back to Idle.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut agent_done = false;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                agent_done = true;
                break;
            }
        }
        assert!(
            agent_done,
            "Runtime should have emitted AgentDone after hitting the cap"
        );

        // The DB should contain the user message + system cap message +
        // a bounded number of agent/tool rows — NOT an unbounded loop.
        // Exact row count depends on how many cycles squeezed in before the
        // cancel lands; the cap is 3, so the upper bound is ~3 agent + 3 tool
        // messages plus bookkeeping. We assert "small" rather than an exact
        // number to stay robust against scheduling.
        let messages = storage.get_messages("cap-test-conv").await.unwrap();
        assert!(
            messages.len() < 20,
            "DB should contain a bounded set of messages, got {}",
            messages.len()
        );

        // The system cap message must be present and mention the cap.
        let has_cap_message = messages.iter().any(|m| {
            matches!(m.message_type, MessageType::System)
                && match &m.content {
                    MessageContent::System(s) => s.text.contains("Tool-use iteration limit"),
                    _ => false,
                }
        });
        assert!(
            has_cap_message,
            "Expected a system message explaining the cap, got: {:#?}",
            messages.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
    }

    /// Regression test for task 24683: every `SseEvent::Token` for a given
    /// LLM turn must land on the broadcast channel before the corresponding
    /// `SseEvent::Message`.
    ///
    /// Before the fix: the executor's main task sent the LLM outcome as soon
    /// as `complete_streaming` returned, but the token-forwarder ran in its
    /// own `tokio::spawn`. With enough tokens buffered in the inner broadcast
    /// channel, the forwarder could still be draining when the outer
    /// broadcast channel had already seen `SseEvent::Message` — producing
    /// phantom streaming content in the client ("same message stuck
    /// repeatedly delivering itself").
    ///
    /// After the fix: the LLM task `drop`s the chunk sender and `awaits` the
    /// forwarder's `JoinHandle` before sending the outcome, guaranteeing the
    /// order we assert below.
    ///
    /// Uses the multi-thread runtime because on `current_thread` the race is
    /// not reachable: tasks spawned on the same thread are polled FIFO and
    /// the forwarder always drains before the main loop runs. Multi-thread
    /// allows genuine parallel scheduling between the forwarder and the
    /// outcome-routing / main-loop tasks, which is what real production
    /// looks like.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    // A single ordering scenario: streaming token deltas must arrive before the
    // finalized message; splitting the linear assert sequence would obscure it.
    #[allow(clippy::too_many_lines)]
    async fn test_streaming_tokens_ordered_before_message() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        // The race is inherently scheduler-dependent. Without the fix, a
        // single iteration catches the bug ~1 in 5 runs on a 4-worker
        // runtime — not reliable enough for CI. We run many independent
        // iterations back-to-back; with the fix all succeed, without it at
        // least one fails with overwhelming probability.
        const ITERATIONS: usize = 30;

        for iteration in 0..ITERATIONS {
            // 500 tokens is overkill but makes it vanishingly unlikely that
            // the forwarder finishes synchronously inside its spawn before
            // the LLM task progresses.
            let llm = Arc::new(StreamingMockLlmClient::new(500, "final text"));
            let tools = Arc::new(MockToolExecutor::new());
            let storage = Arc::new(InMemoryStorage::new());
            let context = ConvContext::new(
                format!("ordering-test-{iteration}"),
                PathBuf::from("/tmp"),
                "streaming-mock",
                200_000,
            );
            let (event_tx, event_rx) = mpsc::channel(32);
            let broadcast_tx = crate::runtime::SseBroadcaster::new(4096, 0);
            let mut broadcast_rx = broadcast_tx.subscribe();

            let runtime = ConversationRuntime::new(
                context,
                ConvState::Idle,
                storage.clone(),
                llm,
                tools,
                Arc::new(BrowserSessionManager::default()),
                Arc::new(crate::tools::BashHandleRegistry::new()),
                Arc::new(crate::tools::TmuxRegistry::new()),
                Arc::new(ModelRegistry::new_empty()),
                crate::terminal::ActiveTerminals::new(),
                event_rx,
                event_tx.clone(),
                broadcast_tx,
            );

            // Runtime runs until its outcome_rx/event_rx both become empty
            // or it hits a terminal state. We don't join it — the runtime
            // holds its own internal event_tx clone, so dropping our test
            // clone isn't enough to make it exit, and waiting here would
            // hang the test. Letting it leak is fine at test scope because
            // `#[tokio::test]` gives each test function its own tokio
            // runtime and aborts all spawned tasks when that runtime drops
            // at the end of the test — the leaked runtimes never survive
            // into other tests.
            //
            // The design gap (runtime has no graceful-shutdown path) is
            // tracked as task 24685. Once that lands, this test can join
            // the handle instead of fire-and-forgetting it.
            tokio::spawn(async move { runtime.run().await });

            event_tx
                .send(Event::UserMessage {
                    text: "stream please".to_string(),
                    llm_text: None,
                    images: vec![],
                    files: vec![],
                    message_id: uuid::Uuid::new_v4().to_string(),
                    user_agent: None,
                    skill_invocation: None,
                })
                .await
                .unwrap();

            // Collect events until AgentDone or timeout.
            let mut last_token_idx: Option<usize> = None;
            let mut first_agent_msg_idx: Option<usize> = None;
            let mut seen_agent_done = false;

            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut idx = 0usize;
            while tokio::time::Instant::now() < deadline && !seen_agent_done {
                if let Ok(Ok(evt)) =
                    tokio::time::timeout(Duration::from_millis(100), broadcast_rx.recv()).await
                {
                    match &evt {
                        SseEvent::Token { .. } => {
                            last_token_idx = Some(idx);
                        }
                        SseEvent::Message { message } => {
                            if matches!(message.message_type, MessageType::Agent)
                                && first_agent_msg_idx.is_none()
                            {
                                first_agent_msg_idx = Some(idx);
                            }
                        }
                        SseEvent::AgentDone { .. } => {
                            seen_agent_done = true;
                        }
                        _ => {}
                    }
                    idx += 1;
                } else { /* keep polling until deadline */
                }
            }

            drop(event_tx);

            assert!(
                seen_agent_done,
                "iteration {iteration}: runtime should emit AgentDone"
            );
            let last_token = last_token_idx.unwrap_or_else(|| {
                panic!(
                    "iteration {iteration}: streaming mock emitted 500 tokens; at least one \
                     SseEvent::Token expected"
                )
            });
            let first_agent_msg = first_agent_msg_idx.unwrap_or_else(|| {
                panic!(
                    "iteration {iteration}: streaming mock should produce an Agent \
                     SseEvent::Message"
                )
            });

            assert!(
                last_token < first_agent_msg,
                "task 24683 regression (iteration {iteration}): last Token event (index \
                 {last_token}) arrived AT OR AFTER the Agent Message event (index \
                 {first_agent_msg}). The token forwarder's JoinHandle is no longer being \
                 awaited before the LLM outcome is sent, so trailing tokens can race past \
                 the Message — producing phantom streaming buffers in the client."
            );
        }
    }

    // ========================================================================
    // Adversarial cancellation-liveness QA (task 08692, P0)
    //
    // These pin the subtle state-machine-level invariants the cancellation
    // backstops rely on. They are pure-transition tests (no timing) so they are
    // deterministic and fast.
    // ========================================================================

    /// Vector 2: synthetic-vs-real result race in `CancellingSubAgents`.
    ///
    /// A real `SubAgentResult` for agent X drains X out of `pending` (last one →
    /// Idle). The backstop then injects a *duplicate* synthetic `TimedOut` for X.
    /// That duplicate must be rejected as `InvalidTransition` — no panic, no
    /// double-drain, and (separately verified) no negative pending count.
    #[tokio::test]
    async fn cancelling_sub_agents_duplicate_result_for_drained_agent_is_rejected() {
        use crate::state_machine::state::{PendingSubAgent, SubAgentMode, SubAgentOutcome};
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        let context = ConvContext::new("test", PathBuf::from("/tmp"), "model", 200_000);

        // Single pending agent: the real result drives the last-one → Idle arm.
        let state = ConvState::CancellingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "X".to_string(),
                task: "t".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
        };

        // Real result for X → Idle.
        let result = transition(
            &state,
            &context,
            Event::SubAgentResult {
                agent_id: "X".to_string(),
                outcome: SubAgentOutcome::Success {
                    result: "real".to_string(),
                },
            },
        )
        .unwrap();
        assert!(
            matches!(result.new_state, ConvState::Idle),
            "Real last result should drain CancellingSubAgents → Idle, got {:?}",
            result.new_state
        );

        // Now Idle. The backstop's duplicate synthetic TimedOut for the
        // already-drained X must be rejected, not mis-applied. (Idle absorbs a
        // sub-agent-only event via the terminal-absorb path, so assert it does
        // NOT leave Idle and produces no effects.)
        let dup = transition(
            &result.new_state,
            &context,
            Event::SubAgentResult {
                agent_id: "X".to_string(),
                outcome: SubAgentOutcome::TimedOut,
            },
        );
        // Ok(absorb-in-Idle with no effects) or Err(hard rejection) both acceptable.
        if let Ok(r) = dup {
            assert!(
                matches!(r.new_state, ConvState::Idle),
                "Duplicate result in Idle must not leave Idle, got {:?}",
                r.new_state
            );
            assert!(
                r.effects.is_empty(),
                "Duplicate result in Idle must produce no effects, got {:?}",
                r.effects
            );
        }
    }

    /// Vector 2 (mid-drain variant): with two pending, a duplicate result for an
    /// agent that was already drained (no longer in `pending`) is an
    /// `InvalidTransition` — it must not underflow the pending set nor double-add
    /// to `completed_results`.
    #[tokio::test]
    async fn cancelling_sub_agents_duplicate_mid_drain_is_invalid_transition() {
        use crate::state_machine::state::{PendingSubAgent, SubAgentMode, SubAgentOutcome};
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        let context = ConvContext::new("test", PathBuf::from("/tmp"), "model", 200_000);

        // Two pending: X already drained, only Y remains.
        let state = ConvState::CancellingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "Y".to_string(),
                task: "ty".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
        };

        // Duplicate TimedOut for X (not in pending) → InvalidTransition.
        let dup = transition(
            &state,
            &context,
            Event::SubAgentResult {
                agent_id: "X".to_string(),
                outcome: SubAgentOutcome::TimedOut,
            },
        );
        assert!(
            dup.is_err(),
            "A result for an agent not in pending must be rejected, got {:?}",
            dup.map(|r| r.new_state)
        );
    }

    /// Vector 4: a late stale tool outcome carrying the OLD `tool_use_id` must
    /// NOT affect a NEW `ToolExecuting` round started with a DIFFERENT id after
    /// the backstop forced Idle.
    ///
    /// The forwarder captures the tool's id at spawn (`forwarder_tool_use_id`),
    /// so a stale `ToolComplete`/`ToolAborted` carries the old id. The SM guards
    /// every tool-completion arm on `tool_use_id == current_tool.id`, so the
    /// stale id falls through to `InvalidTransition`.
    #[tokio::test]
    async fn stale_tool_outcome_with_old_id_does_not_affect_new_round() {
        use crate::state_machine::state::{AssistantMessage, ToolCall, ToolInput};
        use crate::state_machine::{transition, ConvContext, Event};
        use phoenix_llm::ContentBlock;
        use std::path::PathBuf;

        let context = ConvContext::new("test", PathBuf::from("/tmp"), "model", 200_000);

        // New round with a DIFFERENT tool_use_id ("new-id").
        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::ToolUse {
                id: "new-id".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            }],
            None,
            None,
        );
        let new_state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "new-id",
                ToolInput::Bash(crate::tools::BashToolInput::run("cmd")),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message,
        };

        // Stale ToolComplete for the OLD id arrives.
        let stale = transition(
            &new_state,
            &context,
            Event::ToolComplete {
                tool_use_id: "old-id".to_string(),
                result: crate::db::ToolResult::cancelled("old-id".to_string(), "stale"),
            },
        );
        assert!(
            stale.is_err(),
            "Stale ToolComplete for an old tool_use_id must be rejected in a new \
             ToolExecuting round, got {:?}",
            stale.map(|r| r.new_state)
        );

        // Stale ToolAborted for the OLD id also rejected.
        let stale_abort = transition(
            &new_state,
            &context,
            Event::ToolAborted {
                tool_use_id: "old-id".to_string(),
            },
        );
        assert!(
            stale_abort.is_err(),
            "Stale ToolAborted for an old tool_use_id must be rejected, got {:?}",
            stale_abort.map(|r| r.new_state)
        );
    }

    /// Vector 4 (Idle variant): after the `CancellingTool` backstop forces Idle,
    /// the orphaned task's forwarded late `ToolComplete` must be absorbed/rejected
    /// in Idle without mis-applying.
    #[tokio::test]
    async fn late_tool_outcome_in_idle_is_harmless() {
        use crate::state_machine::{transition, ConvContext, Event};
        use std::path::PathBuf;

        let context = ConvContext::new("test", PathBuf::from("/tmp"), "model", 200_000);

        let res = transition(
            &ConvState::Idle,
            &context,
            Event::ToolComplete {
                tool_use_id: "orphan".to_string(),
                result: crate::db::ToolResult::cancelled("orphan".to_string(), "late"),
            },
        );
        // Either a hard rejection or an absorb that stays in Idle is acceptable;
        // a transition OUT of Idle would be a bug.
        if let Ok(r) = res {
            assert!(
                matches!(r.new_state, ConvState::Idle),
                "Late ToolComplete in Idle must not leave Idle, got {:?}",
                r.new_state
            );
        }
    }

    /// Vector 1: the unified deadline must NOT restart on an
    /// `AwaitingSubAgents → AwaitingSubAgents` self-transition (one sub-agent
    /// resolves while others remain). `manage_deadline` keys re-arming on a
    /// `std::mem::discriminant` change, so a self-transition is the *same*
    /// discriminant and the in-flight deadline is preserved. This pins that
    /// discriminant equality directly — a regression to field-wise comparison or
    /// unconditional re-arm would let a stuck sub-agent run ~40min instead of 20.
    #[test]
    fn awaiting_sub_agents_self_transition_preserves_deadline_discriminant() {
        use crate::state_machine::state::{PendingSubAgent, SubAgentMode};

        let two = ConvState::AwaitingSubAgents {
            pending: vec![
                PendingSubAgent {
                    agent_id: "a".to_string(),
                    task: "ta".to_string(),
                    mode: SubAgentMode::Work,
                },
                PendingSubAgent {
                    agent_id: "b".to_string(),
                    task: "tb".to_string(),
                    mode: SubAgentMode::Work,
                },
            ],
            completed_results: vec![],
            spawn_tool_id: None,
        };
        let one = ConvState::AwaitingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "b".to_string(),
                task: "tb".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
            spawn_tool_id: None,
        };

        // Same variant → discriminant equal → manage_deadline keeps the clock.
        assert_eq!(
            std::mem::discriminant(&two),
            std::mem::discriminant(&one),
            "AwaitingSubAgents self-transition must keep the same discriminant so \
             manage_deadline preserves the original 20-minute deadline"
        );

        // Cross-check: AwaitingSubAgents → CancellingSubAgents IS a variant
        // change, so the deadline re-arms (to the 3s cancellation window).
        let cancelling = ConvState::CancellingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "b".to_string(),
                task: "tb".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
        };
        assert_ne!(
            std::mem::discriminant(&one),
            std::mem::discriminant(&cancelling),
            "AwaitingSubAgents → CancellingSubAgents must change discriminant so the \
             cancellation backstop replaces the long completion deadline"
        );
    }

    /// Vector 3 (end-to-end): after the `CancellingTool` backstop aborts a wedged
    /// tool task and forces Idle, a NEW user turn whose NEW tool task is started
    /// must run to completion — proving no stale `tool_task_handle` aborts the
    /// new task and no stale deadline interferes.
    ///
    /// Turn 1: uncooperative tool wedges → `UserCancel` → backstop fires (≤3s) →
    /// Idle. Turn 2: same executor is now cooperative → its tool completes and
    /// the second turn ends with `AgentDone` from a clean text response.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn new_tool_round_after_backstop_is_not_aborted_by_stale_handle() {
        use crate::runtime::{ConversationRuntime, SseEvent};
        use crate::state_machine::ConvContext;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let llm = Arc::new(MockLlmClient::new("test-model"));
        // Turn 1: a tool call (will wedge).
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "sleep 100"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });
        // Turn 2: a tool call (cooperative now), then...
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::tool_use(
                "tool-2",
                "bash",
                serde_json::json!({"op": "run", "cmd": "echo hi"}),
            )],
            end_turn: false,
            usage: Usage::default(),
        });
        // ...the post-tool LLM round returns a plain text answer → AgentDone.
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("done with second tool")],
            end_turn: true,
            usage: Usage::default(),
        });

        let tools = Arc::new(
            FirstCallUncooperativeToolExecutor::new().with_tool("bash", ToolOutput::success("ok")),
        );
        let execution_started = tools.execution_started.clone();
        let cooperative_completed = tools.cooperative_completed.clone();

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000);
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(256, 0);
        let mut broadcast_rx = broadcast_tx.subscribe();

        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage.clone(),
            llm,
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcast_tx,
        );

        tokio::spawn(async move { runtime.run().await });

        // Turn 1.
        event_tx
            .send(Event::UserMessage {
                text: "wedge".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), execution_started.notified())
            .await
            .expect("first (wedging) tool should start");

        // Cancel; backstop must drive to Idle.
        event_tx
            .send(Event::UserCancel { reason: None })
            .await
            .unwrap();

        let mut reached_idle = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                reached_idle = true;
                break;
            }
        }
        assert!(reached_idle, "cancel must reach Idle via backstop");
        assert!(
            matches!(
                storage.get_current_state("test-conv"),
                Some(ConvState::Idle)
            ),
            "state must be Idle after backstop before starting turn 2"
        );

        // Turn 2: the NEW tool task must run cooperatively to completion. If a
        // stale handle/deadline aborted it, cooperative_completed never fires and
        // no second AgentDone arrives.
        event_tx
            .send(Event::UserMessage {
                text: "second".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        // The new tool task completes cooperatively.
        tokio::time::timeout(Duration::from_secs(3), cooperative_completed.notified())
            .await
            .expect(
                "second (cooperative) tool task must complete — a stale handle must not \
                 abort the new round's tool task",
            );

        // And the second turn reaches AgentDone (clean Idle).
        let mut second_done = false;
        let deadline2 = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline2 {
            if let Ok(Ok(SseEvent::AgentDone { .. })) =
                tokio::time::timeout(Duration::from_millis(50), broadcast_rx.recv()).await
            {
                second_done = true;
                break;
            }
        }
        assert!(
            second_done,
            "second turn must reach AgentDone after the cooperative tool completes"
        );
        assert!(
            matches!(
                storage.get_current_state("test-conv"),
                Some(ConvState::Idle)
            ),
            "second turn must end in Idle"
        );
    }

    /// Vector 8 (the production incident — sub-agent half): a SUB-AGENT wedged in
    /// `CancellingTool` (its tool never observes the token) must, via its own
    /// `CancellingTool` backstop, reach the terminal `Failed` state AND emit
    /// `NotifyParent` — delivering a real `SubAgentResult(Failure/Cancelled)` to
    /// the parent so the parent's `CancellingSubAgents` fan-in stays correct.
    ///
    /// Constructed directly in `CancellingTool` (no tool task handle — modelling
    /// a tool that is already wedged/gone). The only path out is the backstop,
    /// which injects a synthetic `ToolAborted` → the sub-agent's
    /// `CancellingTool + ToolAborted -> Failed + NotifyParent` arm.
    #[tokio::test]
    async fn wedged_sub_agent_in_cancelling_tool_notifies_parent_via_backstop() {
        use crate::runtime::ConversationRuntime;
        use crate::state_machine::state::{AssistantMessage, SubAgentOutcome};
        use crate::state_machine::ConvContext;
        use phoenix_llm::ContentBlock;
        use std::path::PathBuf;
        use tokio::sync::mpsc;

        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::sub_agent(
            "sub-1",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "parent-conv",
        );

        // The sub-agent's own event loop channel.
        let (event_tx, event_rx) = mpsc::channel::<Event>(32);
        // The parent's inbound channel — NotifyParent sends a SubAgentResult here.
        let (parent_tx, mut parent_rx) = mpsc::channel::<Event>(32);
        let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);

        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::ToolUse {
                id: "wedged-tool".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            }],
            None,
            None,
        );
        let initial_state = ConvState::CancellingTool {
            tool_use_id: "wedged-tool".to_string(),
            skipped_tools: vec![],
            completed_results: vec![],
            assistant_message,
            pending_sub_agents: vec![],
        };
        let runtime = ConversationRuntime::new(
            context,
            initial_state,
            storage.clone(),
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(UncooperativeMockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx,
            broadcast_tx,
        )
        .with_parent(parent_tx);

        tokio::spawn(async move { runtime.run().await });

        // The sub-agent's CancellingTool backstop (3s) must drive it to Failed and
        // emit NotifyParent → a SubAgentResult on the parent channel. Window is
        // longer than the 3s deadline so the backstop fires first.
        let received = tokio::time::timeout(Duration::from_secs(6), parent_rx.recv())
            .await
            .expect("parent must receive a SubAgentResult within the backstop deadline");

        match received {
            Some(Event::SubAgentResult { agent_id, outcome }) => {
                assert_eq!(
                    agent_id, "sub-1",
                    "result must identify the wedged sub-agent"
                );
                assert!(
                    matches!(outcome, SubAgentOutcome::Failure { .. }),
                    "a cancelled/backstopped sub-agent must report Failure to the parent, \
                     got {outcome:?}"
                );
            }
            other => panic!("expected SubAgentResult to parent, got {other:?}"),
        }

        // The sub-agent's persisted state must be terminal Failed, not stuck.
        let final_state = storage.get_current_state("sub-1");
        assert!(
            matches!(final_state, Some(ConvState::Failed { .. })),
            "wedged sub-agent must reach terminal Failed, got {final_state:?}"
        );
    }
}
