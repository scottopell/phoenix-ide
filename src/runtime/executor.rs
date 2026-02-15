//! Conversation runtime executor

use super::traits::{LlmClient, Storage, ToolExecutor};
use super::{SseEvent, SubAgentCancelRequest, SubAgentSpawnRequest};

use crate::db::{MessageContent, ToolResult};
use crate::llm::{ContentBlock, LlmMessage, LlmRequest, MessageRole, ModelRegistry, SystemContent};
use crate::state_machine::state::{ToolCall, ToolInput};
use crate::state_machine::{transition, ConvContext, ConvState, Effect, Event};
use crate::system_prompt::build_system_prompt;
use crate::tools::{BrowserSessionManager, ToolContext};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// Generic conversation runtime that can work with any storage, LLM, and tool implementations
pub struct ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    context: ConvContext,
    state: ConvState,
    storage: S,
    llm_client: Arc<L>,
    tool_executor: Arc<T>,
    /// Browser session manager for `ToolContext`
    browser_sessions: Arc<BrowserSessionManager>,
    /// LLM registry for `ToolContext`
    llm_registry: Arc<ModelRegistry>,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    broadcast_tx: broadcast::Sender<SseEvent>,
    /// Token to cancel running tool execution
    tool_cancel_token: Option<CancellationToken>,
    /// Token to cancel running LLM request
    llm_cancel_token: Option<CancellationToken>,
    /// Channel to notify parent of sub-agent completion (sub-agent only)
    parent_event_tx: Option<mpsc::Sender<Event>>,
    /// Channel to request sub-agent spawning (parent only)
    spawn_tx: Option<mpsc::Sender<SubAgentSpawnRequest>>,
    /// Channel to request sub-agent cancellation (parent only)
    cancel_tx: Option<mpsc::Sender<SubAgentCancelRequest>>,
    /// Buffer for `SubAgentResult` events received before entering `AwaitingSubAgents`
    sub_agent_result_buffer: Vec<Event>,
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: ConvContext,
        state: ConvState,
        storage: S,
        llm_client: L,
        tool_executor: T,
        browser_sessions: Arc<BrowserSessionManager>,
        llm_registry: Arc<ModelRegistry>,
        event_rx: mpsc::Receiver<Event>,
        event_tx: mpsc::Sender<Event>,
        broadcast_tx: broadcast::Sender<SseEvent>,
    ) -> Self {
        Self {
            context,
            state,
            storage,
            llm_client: Arc::new(llm_client),
            tool_executor: Arc::new(tool_executor),
            browser_sessions,
            llm_registry,
            event_rx,
            event_tx,
            broadcast_tx,
            tool_cancel_token: None,
            llm_cancel_token: None,
            parent_event_tx: None,
            spawn_tx: None,
            cancel_tx: None,
            sub_agent_result_buffer: Vec::new(),
        }
    }

    /// Set the parent event channel (for sub-agents)
    pub fn with_parent(mut self, parent_tx: mpsc::Sender<Event>) -> Self {
        self.parent_event_tx = Some(parent_tx);
        self
    }

    /// Set the spawn/cancel channels (for parent conversations)
    pub fn with_spawn_channels(
        mut self,
        spawn_tx: mpsc::Sender<SubAgentSpawnRequest>,
        cancel_tx: mpsc::Sender<SubAgentCancelRequest>,
    ) -> Self {
        self.spawn_tx = Some(spawn_tx);
        self.cancel_tx = Some(cancel_tx);
        self
    }

    pub async fn run(mut self) {
        tracing::info!(conv_id = %self.context.conversation_id, "Starting conversation runtime");

        // Check if we need to resume an interrupted operation
        // This handles crash recovery for in-flight LLM requests
        if let ConvState::LlmRequesting { .. } = &self.state {
            tracing::info!(conv_id = %self.context.conversation_id, "Resuming interrupted LLM request");
            if let Err(e) = self.execute_effect(Effect::RequestLlm).await {
                tracing::error!(error = %e, "Failed to resume LLM request");
                let _ = self.broadcast_tx.send(SseEvent::Error {
                    message: format!("Failed to resume: {e}"),
                });
            }
        }

        // Process events in a loop - no recursion
        loop {
            tokio::select! {
                Some(event) = self.event_rx.recv() => {
                    if let Err(e) = self.process_event(event).await {
                        tracing::error!(error = %e, "Error handling event");
                        let _ = self.broadcast_tx.send(SseEvent::Error {
                            message: e.clone(),
                        });
                    }
                }
                else => break,
            }
        }

        tracing::info!(conv_id = %self.context.conversation_id, "Conversation runtime stopped");
    }

    async fn process_event(&mut self, event: Event) -> Result<(), String> {
        // Check if this is a SubAgentResult that needs buffering
        if let Event::SubAgentResult { .. } = &event {
            if !self.can_handle_sub_agent_result() {
                tracing::debug!("Buffering SubAgentResult, parent not in AwaitingSubAgents");
                self.sub_agent_result_buffer.push(event);
                return Ok(());
            }
        }

        // We need to process events in a loop to handle chained effects
        let mut events_to_process = vec![event];

        while let Some(current_event) = events_to_process.pop() {
            // Pure state transition
            let result = match transition(&self.state, &self.context, current_event) {
                Ok(r) => r,
                Err(e) => {
                    // Transition errors are user-facing (e.g., "agent is busy")
                    let _ = self.broadcast_tx.send(SseEvent::Error {
                        message: e.to_string(),
                    });
                    return Err(e.to_string());
                }
            };

            // Update state
            let old_state = std::mem::replace(&mut self.state, result.new_state.clone());

            // Check if we just entered AwaitingSubAgents - drain the buffer
            if !matches!(
                old_state,
                ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
            ) && matches!(
                self.state,
                ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
            ) {
                // Just entered a state that can handle SubAgentResult events
                // Drain the buffer and add to events to process
                let buffered = std::mem::take(&mut self.sub_agent_result_buffer);
                if !buffered.is_empty() {
                    tracing::debug!(count = buffered.len(), "Draining buffered SubAgentResults");
                    events_to_process.extend(buffered);
                }
            }

            // Execute effects and collect generated events
            for effect in result.effects {
                if let Some(generated_event) = self.execute_effect(effect).await? {
                    events_to_process.push(generated_event);
                }
            }

            // Note: Tool execution is now handled by Effect::ExecuteTool from the state machine,
            // so we no longer need to check and execute tools here.
        }

        Ok(())
    }

    /// Check if the current state can handle `SubAgentResult` events
    fn can_handle_sub_agent_result(&self) -> bool {
        matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        )
    }

    /// Handle the `spawn_agents` tool specially:
    /// 1. Parse tasks and generate agent IDs
    /// 2. Send spawn requests to `RuntimeManager` for each task
    /// 3. Return `SpawnAgentsComplete` event
    async fn handle_spawn_agents_tool(&mut self, tool: ToolCall) -> Result<Option<Event>, String> {
        use crate::state_machine::state::{PendingSubAgent, SpawnAgentsInput, SubAgentSpec};

        let tool_use_id = tool.id.clone();
        let input_value = tool.input.to_value();

        // Parse the spawn_agents input
        let input: SpawnAgentsInput = match serde_json::from_value(input_value) {
            Ok(i) => i,
            Err(e) => {
                // Return error as regular tool completion
                let result = ToolResult::error(tool_use_id.clone(), format!("Invalid input: {e}"));
                return Ok(Some(Event::ToolComplete {
                    tool_use_id,
                    result,
                }));
            }
        };

        if input.tasks.is_empty() {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "At least one task is required".to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        // Generate agent IDs and prepare spawn specs
        let mut spawned = Vec::new();
        let parent_cwd = self.context.working_dir.to_string_lossy().to_string();

        for task in &input.tasks {
            let agent_id = uuid::Uuid::new_v4().to_string();
            let cwd = task.cwd.clone().unwrap_or_else(|| parent_cwd.clone());

            spawned.push(PendingSubAgent {
                agent_id: agent_id.clone(),
                task: task.task.clone(),
            });

            // Send spawn request to RuntimeManager
            if let Some(spawn_tx) = &self.spawn_tx {
                let spec = SubAgentSpec {
                    agent_id,
                    task: task.task.clone(),
                    cwd,
                    timeout: None, // TODO: Add timeout parameter to spawn_agents
                };
                let request = SubAgentSpawnRequest {
                    spec,
                    parent_conversation_id: self.context.conversation_id.clone(),
                    parent_event_tx: self.event_tx.clone(),
                    model_id: self.context.model_id.clone(),
                };
                if let Err(e) = spawn_tx.send(request).await {
                    tracing::error!(error = %e, "Failed to send spawn request");
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!("Failed to spawn sub-agents: {e}"),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
            } else {
                tracing::warn!("No spawn channel configured, cannot spawn sub-agents");
                let result = ToolResult::error(
                    tool_use_id.clone(),
                    "Sub-agent spawning not configured".to_string(),
                );
                return Ok(Some(Event::ToolComplete {
                    tool_use_id,
                    result,
                }));
            }
        }

        // Build success result
        let agent_ids: Vec<&str> = spawned.iter().map(|p| p.agent_id.as_str()).collect();
        let output = format!(
            "Spawning {} sub-agent(s): {}",
            spawned.len(),
            agent_ids.join(", ")
        );
        let result = ToolResult {
            tool_use_id: tool_use_id.clone(),
            success: true,
            output,
            is_error: false,
            display_data: None,
        };

        // Send SpawnAgentsComplete event (synchronously returned, not async)
        Ok(Some(Event::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        }))
    }

    /// Execute an effect and optionally return a generated event
    #[allow(clippy::too_many_lines)] // Effect handling is inherently complex
    async fn execute_effect(&mut self, effect: Effect) -> Result<Option<Event>, String> {
        match effect {
            Effect::PersistMessage {
                content,
                display_data,
                usage_data,
                message_id,
            } => {
                let msg = self
                    .storage
                    .add_message(
                        &message_id,
                        &self.context.conversation_id,
                        &content,
                        display_data.as_ref(),
                        usage_data.as_ref(),
                    )
                    .await?;

                // Broadcast to clients (display_data already computed at effect creation)
                let msg_json = serde_json::to_value(&msg).unwrap_or(Value::Null);
                let _ = self
                    .broadcast_tx
                    .send(SseEvent::Message { message: msg_json });
                Ok(None)
            }

            Effect::PersistState => {
                // Persist the full state as JSON
                self.storage
                    .update_state(&self.context.conversation_id, &self.state)
                    .await?;

                // Broadcast state change with full state data
                let state_json = serde_json::to_value(&self.state).unwrap_or(Value::Null);
                let _ = self
                    .broadcast_tx
                    .send(SseEvent::StateChange { state: state_json });
                Ok(None)
            }

            Effect::RequestLlm => {
                // Create cancellation token for this LLM request
                let cancel_token = CancellationToken::new();
                self.llm_cancel_token = Some(cancel_token.clone());

                // Spawn LLM request as background task
                let llm_client = self.llm_client.clone();
                let tool_executor = self.tool_executor.clone();
                let storage = self.storage.clone();
                let event_tx = self.event_tx.clone();
                let conv_id = self.context.conversation_id.clone();
                let working_dir = self.context.working_dir.clone();
                let is_sub_agent = self.context.is_sub_agent;
                let current_attempt = match &self.state {
                    ConvState::LlmRequesting { attempt } => *attempt,
                    _ => 1,
                };

                tokio::spawn(async move {
                    tracing::info!(
                        is_sub_agent = is_sub_agent,
                        "Making LLM request (background)"
                    );

                    // Build messages from history
                    let messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                        Ok(m) => m,
                        Err(e) => {
                            let _ = event_tx
                                .send(Event::LlmError {
                                    message: e,
                                    error_kind: crate::db::ErrorKind::Unknown,
                                    attempt: current_attempt,
                                })
                                .await;
                            return;
                        }
                    };

                    // Build system prompt with AGENTS.md content
                    let system_prompt = build_system_prompt(&working_dir, is_sub_agent);

                    // Build request
                    let request = LlmRequest {
                        system: vec![SystemContent::cached(&system_prompt)],
                        messages,
                        tools: tool_executor.definitions(),
                        max_tokens: Some(8192),
                    };

                    // Race LLM request against cancellation
                    tokio::select! {
                        biased;

                        () = cancel_token.cancelled() => {
                            tracing::info!("LLM request cancelled");
                            let _ = event_tx.send(Event::LlmAborted).await;
                        }

                        result = llm_client.complete(&request) => {
                            let event = match result {
                                Ok(response) => {
                                    // Extract tool calls from content and convert to typed ToolCall
                                    let tool_calls: Vec<ToolCall> = response
                                        .tool_uses()
                                        .into_iter()
                                        .map(|(id, name, input)| {
                                            let typed_input = ToolInput::from_name_and_value(name, input.clone());
                                            ToolCall::new(id.to_string(), typed_input)
                                        })
                                        .collect();

                                    Event::LlmResponse {
                                        content: response.content,
                                        tool_calls,
                                        end_turn: response.end_turn,
                                        usage: response.usage,
                                    }
                                }
                                Err(e) => Event::LlmError {
                                    message: e.message.clone(),
                                    error_kind: llm_error_to_db_error(e.kind),
                                    attempt: current_attempt,
                                },
                            };
                            let _ = event_tx.send(event).await;
                        }
                    }
                });

                // Return None - the event will come from the spawned task
                Ok(None)
            }

            Effect::ExecuteTool { tool } => {
                // Special handling for spawn_agents tool
                if tool.name() == "spawn_agents" {
                    return self.handle_spawn_agents_tool(tool).await;
                }

                // Create cancellation token for this tool execution
                let cancel_token = CancellationToken::new();
                self.tool_cancel_token = Some(cancel_token.clone());
                // Clone token to check cancellation state after tool completes
                let cancel_token_check = cancel_token.clone();

                // Create ToolContext for this invocation
                let tool_ctx = ToolContext::new(
                    cancel_token,
                    self.context.conversation_id.clone(),
                    self.context.working_dir.clone(),
                    self.browser_sessions.clone(),
                    self.llm_registry.clone(),
                );

                // Spawn tool execution as background task
                let tool_executor = self.tool_executor.clone();
                let event_tx = self.event_tx.clone();
                let tool_use_id = tool.id.clone();
                let tool_name = tool.name().to_string();
                let tool_input = tool.input.to_value();

                tokio::spawn(async move {
                    tracing::info!(tool = %tool_name, id = %tool_use_id, "Executing tool (background)");

                    // Execute tool with context
                    let output = tool_executor
                        .execute(&tool_name, tool_input, tool_ctx)
                        .await;

                    // Check if the tool was cancelled via the cancellation token.
                    // IMPORTANT: We check the token state, NOT the output string.
                    // The state machine only accepts ToolAborted from CancellingTool state,
                    // which is entered when AbortTool effect cancels the token.
                    // Checking output strings would cause spurious ToolAborted events
                    // that violate the state machine contract.
                    if cancel_token_check.is_cancelled() {
                        tracing::info!(tool_id = %tool_use_id, "Tool cancelled (token signaled)");
                        let _ = event_tx.send(Event::ToolAborted { tool_use_id }).await;
                        return;
                    }

                    let result = match output {
                        Some(out) => ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            success: out.success,
                            output: out.output,
                            is_error: !out.success,
                            display_data: out.display_data,
                        },
                        None => ToolResult::error(
                            tool_use_id.clone(),
                            format!("Unknown tool: {tool_name}"),
                        ),
                    };
                    let _ = event_tx
                        .send(Event::ToolComplete {
                            tool_use_id,
                            result,
                        })
                        .await;
                });

                // Return None - the event will come from the spawned task
                Ok(None)
            }

            Effect::ScheduleRetry { delay, attempt } => {
                let event_tx = self.event_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = event_tx.send(Event::RetryTimeout { attempt }).await;
                });
                Ok(None)
            }

            Effect::NotifyClient { event_type, data } => {
                match event_type.as_str() {
                    "agent_done" => {
                        let _ = self.broadcast_tx.send(SseEvent::AgentDone);
                    }
                    "state_change" => {
                        // data should contain the full state object
                        if let Some(state) = data.get("state") {
                            let _ = self.broadcast_tx.send(SseEvent::StateChange {
                                state: state.clone(),
                            });
                        }
                    }
                    _ => {}
                }
                Ok(None)
            }

            Effect::PersistToolResults { results } => {
                for result in results {
                    let content =
                        MessageContent::tool(&result.tool_use_id, &result.output, result.is_error);
                    let tool_msg_id = uuid::Uuid::new_v4().to_string();
                    let msg = self
                        .storage
                        .add_message(
                            &tool_msg_id,
                            &self.context.conversation_id,
                            &content,
                            None,
                            None,
                        )
                        .await?;

                    // Tool results don't contain bash tool_use blocks, no enrichment needed
                    let msg_json = serde_json::to_value(&msg).unwrap_or(Value::Null);
                    let _ = self
                        .broadcast_tx
                        .send(SseEvent::Message { message: msg_json });
                }
                Ok(None)
            }

            Effect::AbortTool { tool_use_id } => {
                // Signal abort to running tool
                tracing::info!(tool_id = %tool_use_id, "Aborting tool execution");
                if let Some(token) = self.tool_cancel_token.take() {
                    token.cancel();
                }
                // The spawned task will send ToolAborted event when it sees cancellation
                Ok(None)
            }

            Effect::AbortLlm => {
                // Signal abort to running LLM request
                tracing::info!("Aborting LLM request");
                if let Some(token) = self.llm_cancel_token.take() {
                    token.cancel();
                }
                // The spawned task will send LlmAborted event when it sees cancellation
                Ok(None)
            }

            Effect::CancelSubAgents { ids } => {
                tracing::info!(?ids, "Cancelling sub-agents");

                if let Some(cancel_tx) = &self.cancel_tx {
                    let request = SubAgentCancelRequest {
                        ids,
                        parent_conversation_id: self.context.conversation_id.clone(),
                        parent_event_tx: self.event_tx.clone(),
                    };
                    if let Err(e) = cancel_tx.send(request).await {
                        tracing::error!(error = %e, "Failed to send cancel request");
                    }
                } else {
                    tracing::warn!("No cancel channel configured, cannot cancel sub-agents");
                }
                Ok(None)
            }

            Effect::NotifyParent { outcome } => {
                tracing::info!(?outcome, "Notifying parent of sub-agent completion");

                if let Some(parent_tx) = &self.parent_event_tx {
                    let event = Event::SubAgentResult {
                        agent_id: self.context.conversation_id.clone(),
                        outcome,
                    };
                    if let Err(e) = parent_tx.send(event).await {
                        // Parent may have terminated - that's OK
                        tracing::warn!(error = %e, "Failed to notify parent (may have terminated)");
                    }
                } else {
                    tracing::warn!("No parent channel configured for sub-agent");
                }
                Ok(None)
            }

            Effect::PersistSubAgentResults {
                results,
                spawn_tool_id,
            } => {
                // Build the display_data for subagent results
                let display_data = serde_json::json!({
                    "type": "subagent_summary",
                    "results": results
                });

                // If we have a spawn_tool_id, update its message's display_data
                // The message was persisted as "{spawn_tool_id}-result" by persist_tool_message
                if let Some(tool_id) = spawn_tool_id {
                    let message_id = format!("{tool_id}-result");
                    if let Err(e) = self
                        .storage
                        .update_message_display_data(&message_id, &display_data)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            message_id = %message_id,
                            "Failed to update spawn_agents message display_data"
                        );
                    } else {
                        // Fetch the updated message and broadcast it
                        // This allows the frontend to update its message state
                        match self.storage.get_message_by_id(&message_id).await {
                            Ok(updated_msg) => {
                                // This is a tool result message, not an agent message
                                // No bash enrichment needed
                                let msg_json =
                                    serde_json::to_value(&updated_msg).unwrap_or(Value::Null);
                                let _ = self
                                    .broadcast_tx
                                    .send(SseEvent::Message { message: msg_json });
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    message_id = %message_id,
                                    "Failed to fetch updated message for broadcast"
                                );
                            }
                        }
                    }
                } else {
                    // No spawn_tool_id - create a standalone summary message
                    // This happens when spawn_agents wasn't the last tool in a batch
                    let summary_text = format!("{} sub-agent(s) completed", results.len());
                    let content = crate::db::MessageContent::tool(
                        uuid::Uuid::new_v4().to_string(),
                        &summary_text,
                        false,
                    );
                    let msg_id = uuid::Uuid::new_v4().to_string();
                    let message = self
                        .storage
                        .add_message(
                            &msg_id,
                            &self.context.conversation_id,
                            &content,
                            Some(&display_data),
                            None,
                        )
                        .await?;

                    // Broadcast the new message (tool message, no bash enrichment needed)
                    let msg_json = serde_json::to_value(&message).unwrap_or(Value::Null);
                    let _ = self
                        .broadcast_tx
                        .send(SseEvent::Message { message: msg_json });
                }

                Ok(None)
            }

            Effect::RequestContinuation {
                rejected_tool_calls,
            } => {
                // REQ-BED-020: Request continuation summary (tool-less LLM request)
                self.request_continuation(rejected_tool_calls);
                Ok(None)
            }

            Effect::NotifyContextExhausted { summary } => {
                // REQ-BED-021: Notify client of context exhaustion
                let _ = self.broadcast_tx.send(SseEvent::StateChange {
                    state: serde_json::json!({
                        "type": "context_exhausted",
                        "summary": summary
                    }),
                });
                Ok(None)
            }
        }
    }

    /// Build LLM messages from conversation history (instance method)
    #[allow(dead_code)] // May be useful for non-spawned code paths
    async fn build_llm_messages(&self) -> Result<Vec<LlmMessage>, String> {
        Self::build_llm_messages_static(&self.storage, &self.context.conversation_id).await
    }

    /// Build LLM messages from conversation history (static, for spawned tasks)
    async fn build_llm_messages_static(
        storage: &S,
        conv_id: &str,
    ) -> Result<Vec<LlmMessage>, String> {
        use crate::db::{MessageContent, ToolContent, UserContent};

        let db_messages = storage.get_messages(conv_id).await?;

        let mut messages = Vec::new();

        for msg in db_messages {
            match &msg.content {
                MessageContent::User(UserContent { text, images }) => {
                    let mut content = vec![ContentBlock::text(text)];

                    // Add images (REQ-BED-013)
                    for img in images {
                        content.push(ContentBlock::Image {
                            source: img.to_image_source(),
                        });
                    }

                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content,
                    });
                }

                MessageContent::Agent(blocks) => {
                    messages.push(LlmMessage {
                        role: MessageRole::Assistant,
                        content: blocks.clone(),
                    });
                }

                MessageContent::Tool(ToolContent {
                    tool_use_id,
                    content,
                    is_error,
                }) => {
                    // Tool results go in user message
                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::tool_result(tool_use_id, content, *is_error)],
                    });
                }

                // Ignore system, error, and continuation messages
                MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_) => {}
            }
        }

        Ok(messages)
    }

    /// Request continuation summary from LLM (REQ-BED-020)
    #[allow(clippy::needless_pass_by_value)] // Consistent with Effect signature
    fn request_continuation(&mut self, rejected_tool_calls: Vec<ToolCall>) {
        let llm_client = Arc::clone(&self.llm_client);
        let storage = self.storage.clone();
        let event_tx = self.event_tx.clone();
        let conv_id = self.context.conversation_id.clone();

        // Build continuation prompt
        let continuation_prompt = build_continuation_prompt(&rejected_tool_calls);

        // Create cancellation token for the continuation request
        let cancel_token = CancellationToken::new();
        self.llm_cancel_token = Some(cancel_token.clone());

        tokio::spawn(async move {
            // Build messages from history and add continuation request
            let mut messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to build messages for continuation");
                    let _ = event_tx.send(Event::ContinuationFailed { error: e }).await;
                    return;
                }
            };

            // Add the continuation request as a user message
            messages.push(LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(&continuation_prompt)],
            });

            // Build a tool-less request
            let request = LlmRequest {
                messages,
                system: vec![SystemContent::new(
                    "You are wrapping up a conversation that has reached its context limit. \
                    Provide a concise summary to help continue in a new conversation.",
                )],
                tools: vec![],          // No tools for continuation
                max_tokens: Some(2000), // Limit summary length
            };

            // Make the request
            let result: Result<crate::llm::LlmResponse, crate::llm::LlmError> = tokio::select! {
                result = llm_client.complete(&request) => result,
                () = cancel_token.cancelled() => {
                    tracing::info!("Continuation request cancelled");
                    let _ = event_tx.send(Event::ContinuationFailed {
                        error: "Cancelled by user".to_string(),
                    }).await;
                    return;
                }
            };

            match result {
                Ok(response) => {
                    // Extract the text content as summary
                    let summary = response
                        .content
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::Text { text } = block {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let _ = event_tx.send(Event::ContinuationResponse { summary }).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Continuation LLM request failed");
                    let _ = event_tx
                        .send(Event::ContinuationFailed {
                            error: e.to_string(),
                        })
                        .await;
                }
            }
        });
    }
}

/// Build the continuation prompt (REQ-BED-020)
fn build_continuation_prompt(rejected_tool_calls: &[ToolCall]) -> String {
    let mut prompt = String::from(
        "The conversation context is nearly full. Please provide a brief continuation summary \
        that could seed a new conversation.\n\n\
        Include:\n\
        1. Current task status and progress\n\
        2. Key files, concepts, or decisions discussed\n\
        3. Suggested next steps to continue the work\n\n\
        Keep your response concise and actionable.",
    );

    if !rejected_tool_calls.is_empty() {
        use std::fmt::Write;
        prompt.push_str(
            "\n\nNote: The following tool calls were requested but not executed due to context limits:\n",
        );
        for tool in rejected_tool_calls {
            let _ = writeln!(prompt, "- {}", tool.name());
        }
        prompt.push_str("Include these pending actions in your summary.");
    }

    prompt
}

fn llm_error_to_db_error(kind: crate::llm::LlmErrorKind) -> crate::db::ErrorKind {
    // Explicit match arms to ensure new error kinds are handled (no catch-all)
    match kind {
        crate::llm::LlmErrorKind::Auth => crate::db::ErrorKind::Auth,
        crate::llm::LlmErrorKind::RateLimit => crate::db::ErrorKind::RateLimit,
        crate::llm::LlmErrorKind::Network => crate::db::ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => crate::db::ErrorKind::InvalidRequest,
        crate::llm::LlmErrorKind::ServerError => crate::db::ErrorKind::ServerError,
        crate::llm::LlmErrorKind::Unknown => crate::db::ErrorKind::Unknown,
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;
    use crate::llm::LlmErrorKind;

    #[test]
    fn test_llm_error_to_db_error_mapping() {
        // Test all mappings are explicit and correct
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Auth),
            crate::db::ErrorKind::Auth
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::RateLimit),
            crate::db::ErrorKind::RateLimit
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Network),
            crate::db::ErrorKind::Network
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::InvalidRequest),
            crate::db::ErrorKind::InvalidRequest
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ServerError),
            crate::db::ErrorKind::ServerError,
            "ServerError must map to ServerError, not Unknown!"
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Unknown),
            crate::db::ErrorKind::Unknown
        );
    }

    #[test]
    fn test_server_error_is_retryable_after_mapping() {
        // This is the critical test - ServerError from LLM must be retryable
        let llm_error = LlmErrorKind::ServerError;
        let db_error = llm_error_to_db_error(llm_error);
        assert!(
            db_error.is_retryable(),
            "ServerError must be retryable after mapping to db::ErrorKind"
        );
    }
}
