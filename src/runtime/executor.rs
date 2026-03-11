//! Conversation runtime executor
//!
//! The executor loop receives inputs from two sources:
//! - User events via `event_rx` (`UserMessage`, `UserCancel`, etc.) → routed to `transition()`
//! - Effect outcomes via `outcome_rx` (`LlmOutcome`, `ToolOutcome`, etc.) → routed to `handle_outcome()`
//!
//! Background tasks receive typed `oneshot::Sender<T>` for their outcome type.
//! A `Sender<ToolOutcome>` physically cannot send an `LlmOutcome`.
//! The executor wraps received outcomes in `EffectOutcome` for `handle_outcome()`.

use super::traits::{LlmClient, Storage, ToolExecutor};
use super::{SseEvent, SubAgentCancelRequest, SubAgentSpawnRequest};

use crate::db::{MessageContent, ToolResult};
use crate::llm::{ContentBlock, LlmMessage, LlmRequest, MessageRole, ModelRegistry, SystemContent};
use crate::state_machine::outcome::{EffectOutcome, LlmOutcome, ToolOutcome};
use crate::state_machine::state::{ToolCall, ToolInput};
use crate::state_machine::{
    handle_outcome, transition, ConvContext, ConvState, Effect, Event, StepResult,
};
use crate::system_prompt::build_system_prompt;
use crate::tools::{BrowserSessionManager, ToolContext};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Default timeout for sub-agents: 5 minutes (REQ-SA-006, FM-6 prevention).
/// Long enough for real work, short enough to catch stuck agents.
const DEFAULT_SUBAGENT_TIMEOUT: Duration = Duration::from_secs(300);

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
    /// Handle to the spawned LLM task — aborted on cancel to drop the HTTP connection
    llm_task_handle: Option<tokio::task::JoinHandle<()>>,
    /// Channel to notify parent of sub-agent completion (sub-agent only)
    parent_event_tx: Option<mpsc::Sender<Event>>,
    /// Channel to request sub-agent spawning (parent only)
    spawn_tx: Option<mpsc::Sender<SubAgentSpawnRequest>>,
    /// Channel to request sub-agent cancellation (parent only)
    cancel_tx: Option<mpsc::Sender<SubAgentCancelRequest>>,
    /// Buffer for `SubAgentResult` events received before entering `AwaitingSubAgents`.
    /// Pre-allocated with capacity = sub-agent count when spawning (FM-6 prevention).
    sub_agent_result_buffer: Vec<Event>,
    /// Deadline for sub-agent completion — set when entering `AwaitingSubAgents` (REQ-SA-006)
    sub_agent_deadline: Option<tokio::time::Instant>,
    /// Typed outcome channel — background tasks send `EffectOutcome` here.
    /// Each task gets a typed `oneshot::Sender<T>` that constrains what it can send,
    /// then the forwarder wraps the result in `EffectOutcome` for this channel.
    outcome_tx: mpsc::Sender<EffectOutcome>,
    outcome_rx: mpsc::Receiver<EffectOutcome>,
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
        // Outcome channel for typed effect results.
        // Background tasks send typed outcomes (LlmOutcome, ToolOutcome, etc.)
        // through oneshot channels, then forwarders wrap them in EffectOutcome
        // for this unified channel.
        let (outcome_tx, outcome_rx) = mpsc::channel::<EffectOutcome>(64);

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
            llm_task_handle: None,
            parent_event_tx: None,
            spawn_tx: None,
            cancel_tx: None,
            sub_agent_result_buffer: Vec::new(),
            sub_agent_deadline: None,
            outcome_tx,
            outcome_rx,
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

        // Process events and outcomes in a loop - no recursion
        // Three input sources:
        //   event_rx    — user events + legacy executor events (continuation, sub-agent results)
        //   outcome_rx  — typed effect outcomes (LLM, tool, persist, retry)
        //   deadline    — sub-agent timeout (REQ-SA-006, FM-6 prevention)
        loop {
            // Copy deadline before select to avoid borrow conflict
            let deadline = self.sub_agent_deadline;

            tokio::select! {
                Some(event) = self.event_rx.recv() => {
                    if let Err(e) = self.process_event(event).await {
                        tracing::error!(error = %e, "Error handling event");
                        let _ = self.broadcast_tx.send(SseEvent::Error {
                            message: e.clone(),
                        });
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        return;
                    }
                }
                Some(outcome) = self.outcome_rx.recv() => {
                    if let Err(e) = self.process_outcome(outcome).await {
                        tracing::warn!(error = %e, "Outcome rejected by state machine");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        return;
                    }
                }
                // REQ-SA-006: sub-agent deadline expired — cancel all pending agents
                () = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if deadline.is_some() => {
                    self.handle_sub_agent_timeout().await;
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        return;
                    }
                }
                else => break,
            }
        }

        tracing::info!(conv_id = %self.context.conversation_id, "Conversation runtime stopped");
    }

    /// Process a typed effect outcome from a background task.
    ///
    /// Routes through `handle_outcome()` (pure SM function). Invalid outcomes
    /// are logged and discarded — state unchanged.
    async fn process_outcome(&mut self, outcome: EffectOutcome) -> Result<(), String> {
        let result = match handle_outcome(&self.state, &self.context, outcome) {
            Ok(r) => r,
            Err(invalid) => {
                tracing::warn!(
                    reason = %invalid.reason,
                    state = ?std::mem::discriminant(&self.state),
                    "Rejected invalid outcome — state unchanged"
                );
                return Err(invalid.reason);
            }
        };

        // Apply transition result and process any generated events
        let mut events_to_process = self.apply_transition_result(result).await?;

        // Process chained events (e.g., SpawnAgentsComplete from execute_effect)
        while let Some(event) = events_to_process.pop() {
            let chained_result = match transition(&self.state, &self.context, event) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "Chained event from outcome rejected");
                    continue;
                }
            };
            let more_events = self.apply_transition_result(chained_result).await?;
            events_to_process.extend(more_events);
        }

        Ok(())
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

            let generated_events = self.apply_transition_result(result).await?;
            events_to_process.extend(generated_events);
        }

        Ok(())
    }

    /// Apply a `TransitionResult` from either `transition()` or `handle_outcome()`.
    ///
    /// Updates state, drains sub-agent buffer if entering `AwaitingSubAgents`,
    /// dispatches effects. Returns any synchronously generated events
    /// (e.g., from `SpawnAgentsComplete`).
    async fn apply_transition_result(
        &mut self,
        result: crate::state_machine::transition::TransitionResult,
    ) -> Result<Vec<Event>, String> {
        let mut generated_events = Vec::new();

        // Update state
        let old_state = std::mem::replace(&mut self.state, result.new_state.clone());

        // Log notable state transitions at INFO. "Notable" means transitions that cross
        // a meaningful phase boundary (idle↔active, entering/leaving tool execution,
        // terminal states). Internal bookkeeping transitions (e.g. AwaitingLlm→LlmRequesting)
        // are logged at DEBUG to keep steady-state noise low.
        {
            fn state_name(s: &ConvState) -> &'static str {
                match s {
                    ConvState::Idle => "Idle",
                    ConvState::AwaitingLlm => "AwaitingLlm",
                    ConvState::LlmRequesting { .. } => "LlmRequesting",
                    ConvState::ToolExecuting { .. } => "ToolExecuting",
                    ConvState::AwaitingSubAgents { .. } => "AwaitingSubAgents",
                    ConvState::CancellingSubAgents { .. } => "CancellingSubAgents",
                    ConvState::CancellingTool { .. } => "CancellingTool",
                    ConvState::AwaitingContinuation { .. } => "AwaitingContinuation",
                    ConvState::Completed { .. } => "Completed",
                    ConvState::Failed { .. } => "Failed",
                    ConvState::Error { .. } => "Error",
                    ConvState::ContextExhausted { .. } => "ContextExhausted",
                }
            }
            let from = state_name(&old_state);
            let to = state_name(&self.state);
            if from != to {
                let notable = matches!(
                    &self.state,
                    ConvState::Idle
                        | ConvState::ToolExecuting { .. }
                        | ConvState::AwaitingSubAgents { .. }
                        | ConvState::Completed { .. }
                        | ConvState::Failed { .. }
                        | ConvState::Error { .. }
                        | ConvState::ContextExhausted { .. }
                );
                if notable {
                    tracing::info!(
                        conv_id = %self.context.conversation_id,
                        from,
                        to,
                        "State transition"
                    );
                } else {
                    tracing::debug!(
                        conv_id = %self.context.conversation_id,
                        from,
                        to,
                        "State transition"
                    );
                }
            }
        }

        let entering_awaiting = !matches!(
            old_state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        ) && matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        );
        let leaving_awaiting = matches!(
            old_state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        ) && !matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        );

        // Drain buffer when entering AwaitingSubAgents
        if entering_awaiting {
            let buffered = std::mem::take(&mut self.sub_agent_result_buffer);
            if !buffered.is_empty() {
                tracing::debug!(count = buffered.len(), "Draining buffered SubAgentResults");
                generated_events.extend(buffered);
            }
            // Set deadline (REQ-SA-006): timeout starts when parent enters AwaitingSubAgents
            self.sub_agent_deadline = Some(tokio::time::Instant::now() + DEFAULT_SUBAGENT_TIMEOUT);
            tracing::debug!(
                timeout_secs = DEFAULT_SUBAGENT_TIMEOUT.as_secs(),
                "Sub-agent deadline set"
            );
        }

        // Clear deadline when leaving AwaitingSubAgents/CancellingSubAgents
        if leaving_awaiting {
            self.sub_agent_deadline = None;
        }

        // Execute effects and collect generated events
        for effect in result.effects {
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }

        Ok(generated_events)
    }

    /// Check if the current state can handle `SubAgentResult` events
    fn can_handle_sub_agent_result(&self) -> bool {
        matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        )
    }

    /// Handle sub-agent timeout: cancel all pending agents and inject `TimedOut` results.
    ///
    /// Called from the executor select loop when `sub_agent_deadline` fires (REQ-SA-006).
    async fn handle_sub_agent_timeout(&mut self) {
        use crate::state_machine::state::SubAgentOutcome;

        self.sub_agent_deadline = None;

        let pending_ids: Vec<(String, String)> =
            if let ConvState::AwaitingSubAgents { pending, .. } = &self.state {
                pending
                    .iter()
                    .map(|p| (p.agent_id.clone(), p.task.clone()))
                    .collect()
            } else {
                // Deadline fired but state already moved on — nothing to do
                return;
            };

        tracing::warn!(
            count = pending_ids.len(),
            "Sub-agent timeout reached, cancelling pending agents"
        );

        // Cancel the actual sub-agent runtimes
        if let Some(cancel_tx) = &self.cancel_tx {
            let ids: Vec<String> = pending_ids.iter().map(|(id, _)| id.clone()).collect();
            let request = SubAgentCancelRequest {
                ids,
                parent_conversation_id: self.context.conversation_id.clone(),
                parent_event_tx: self.event_tx.clone(),
            };
            if let Err(e) = cancel_tx.send(request).await {
                tracing::error!(error = %e, "Failed to send cancel request for timed-out agents");
            }
        }

        // Inject TimedOut results for each pending agent — transitions state normally
        for (agent_id, _task) in pending_ids {
            let event = Event::SubAgentResult {
                agent_id,
                outcome: SubAgentOutcome::TimedOut,
            };
            if let Err(e) = self.process_event(event).await {
                tracing::warn!(error = %e, "Failed to process timeout result for sub-agent");
            }
        }
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

        // Bounded buffer: pre-allocate with capacity = sub-agent count (FM-6 prevention)
        self.sub_agent_result_buffer = Vec::with_capacity(input.tasks.len());

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
                    timeout: DEFAULT_SUBAGENT_TIMEOUT,
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
            images: vec![],
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
                let _ = self
                    .broadcast_tx
                    .send(SseEvent::Message { message: msg });
                Ok(None)
            }

            Effect::PersistState => {
                // Persist the full state as JSON
                self.storage
                    .update_state(&self.context.conversation_id, &self.state)
                    .await?;

                // Broadcast state change with full state data
                let _ = self.broadcast_tx.send(SseEvent::StateChange {
                    state: self.state.clone(),
                    display_state: self.state.display_state().as_str().to_string(),
                });
                Ok(None)
            }

            Effect::RequestLlm => {
                // Typed oneshot channel: background task gets Sender<LlmOutcome>,
                // physically cannot send a ToolOutcome or other type.
                let (llm_tx, llm_rx) = oneshot::channel::<LlmOutcome>();
                let outcome_tx = self.outcome_tx.clone();

                let llm_client = self.llm_client.clone();
                let tool_executor = self.tool_executor.clone();
                let storage = self.storage.clone();
                let conv_id = self.context.conversation_id.clone();
                let working_dir = self.context.working_dir.clone();
                let is_sub_agent = self.context.is_sub_agent;

                // Token streaming channel (REQ-BED-025)
                // Broadcast so the forwarding task can subscribe before the LLM task starts.
                let (chunk_tx, chunk_rx) = broadcast::channel::<crate::llm::TokenChunk>(256);
                let request_id = uuid::Uuid::new_v4().to_string();

                // Spawn token forwarding task BEFORE the LLM task to avoid missing early tokens.
                // Reads TokenChunk::Text events and forwards them as SseEvent::Token.
                let broadcast_tx_for_tokens = self.broadcast_tx.clone();
                let request_id_for_fwd = request_id.clone();
                tokio::spawn(async move {
                    let mut rx = chunk_rx;
                    loop {
                        match rx.recv().await {
                            Ok(crate::llm::TokenChunk::Text(text)) => {
                                let _ = broadcast_tx_for_tokens.send(SseEvent::Token {
                                    text,
                                    request_id: request_id_for_fwd.clone(),
                                });
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::debug!(n, "Token forwarding lagged — some tokens dropped");
                            }
                        }
                    }
                });

                let handle = tokio::spawn(async move {
                    if is_sub_agent {
                        tracing::info!(
                            conv_id = %conv_id,
                            request_id = %request_id,
                            sub_agent = true,
                            "Making LLM request"
                        );
                    } else {
                        tracing::info!(
                            conv_id = %conv_id,
                            request_id = %request_id,
                            "Making LLM request"
                        );
                    }

                    // Build messages from history
                    let messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                        Ok(m) => m,
                        Err(e) => {
                            // Build error → treated as InvalidRequest
                            let _ = llm_tx.send(LlmOutcome::NetworkError { message: e });
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
                        max_tokens: Some(16_384),
                    };

                    // Use streaming — chunk_tx forwards text tokens to SSE clients.
                    // Dropping chunk_tx here (after await) closes the channel and
                    // terminates the forwarding task.
                    let llm_outcome = match llm_client.complete_streaming(&request, &chunk_tx).await
                    {
                        Ok(response) => {
                            // Extract tool calls from content and convert to typed ToolCall
                            let tool_calls: Vec<ToolCall> = response
                                .tool_uses()
                                .into_iter()
                                .map(|(id, name, input)| {
                                    let typed_input =
                                        ToolInput::from_name_and_value(name, input.clone());
                                    ToolCall::new(id.to_string(), typed_input)
                                })
                                .collect();

                            LlmOutcome::Response {
                                content: response.content,
                                tool_calls,
                                end_turn: response.end_turn,
                                usage: response.usage,
                            }
                        }
                        Err(e) => llm_error_to_outcome(e),
                    };
                    // chunk_tx dropped here — closes broadcast, forwarding task exits
                    // Send typed outcome through oneshot channel
                    let _ = llm_tx.send(llm_outcome);
                });
                self.llm_task_handle = Some(handle);

                // Forward the typed outcome to the unified outcome channel
                tokio::spawn(async move {
                    if let Ok(llm_outcome) = llm_rx.await {
                        let _ = outcome_tx.send(EffectOutcome::Llm(llm_outcome)).await;
                    }
                });

                Ok(None)
            }

            Effect::ExecuteTool { tool } => {
                // Special handling for spawn_agents tool
                if tool.name() == "spawn_agents" {
                    return self.handle_spawn_agents_tool(tool).await;
                }

                // Typed oneshot channel: background task gets Sender<ToolOutcome>,
                // physically cannot send an LlmOutcome or other type.
                let (tool_tx, tool_rx) = oneshot::channel::<ToolOutcome>();
                let outcome_tx = self.outcome_tx.clone();

                // Create cancellation token for this tool execution
                let cancel_token = CancellationToken::new();
                self.tool_cancel_token = Some(cancel_token.clone());
                let cancel_token_check = cancel_token.clone();

                // Create ToolContext for this invocation
                let tool_ctx = ToolContext::new(
                    cancel_token,
                    self.context.conversation_id.clone(),
                    self.context.working_dir.clone(),
                    self.browser_sessions.clone(),
                    self.llm_registry.clone(),
                );

                let conv_id = self.context.conversation_id.clone();
                let tool_executor = self.tool_executor.clone();
                let tool_use_id = tool.id.clone();
                let tool_name = tool.name().to_string();
                let tool_input = tool.input.to_value();

                tokio::spawn(async move {
                    tracing::info!(
                        conv_id = %conv_id,
                        tool = %tool_name,
                        id = %tool_use_id,
                        "Executing tool"
                    );
                    let tool_start = std::time::Instant::now();

                    let output = tool_executor
                        .execute(&tool_name, tool_input, tool_ctx)
                        .await;

                    // Check if the tool was cancelled via the cancellation token.
                    // IMPORTANT: We check the token state, NOT the output string.
                    // The state machine only accepts ToolAborted from CancellingTool state,
                    // which is entered when AbortTool effect cancels the token.
                    let tool_outcome = if cancel_token_check.is_cancelled() {
                        tracing::info!(
                            conv_id = %conv_id,
                            tool = %tool_name,
                            id = %tool_use_id,
                            "Tool cancelled"
                        );
                        ToolOutcome::Aborted {
                            tool_use_id,
                            reason: crate::state_machine::AbortReason::CancellationRequested,
                        }
                    } else {
                        use crate::db::ToolContentImage;
                        if let Some(out) = output {
                            tracing::info!(
                                conv_id = %conv_id,
                                tool = %tool_name,
                                id = %tool_use_id,
                                duration_ms = u64::try_from(tool_start.elapsed().as_millis()).unwrap_or(u64::MAX),
                                success = out.success,
                                "Tool completed"
                            );
                            let images = out
                                .images
                                .into_iter()
                                .map(|img| ToolContentImage {
                                    media_type: img.media_type,
                                    data: img.data,
                                })
                                .collect();
                            ToolOutcome::Completed(ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                success: out.success,
                                output: out.output,
                                is_error: !out.success,
                                display_data: out.display_data,
                                images,
                            })
                        } else {
                            tracing::warn!(
                                conv_id = %conv_id,
                                tool = %tool_name,
                                id = %tool_use_id,
                                "Tool not found"
                            );
                            ToolOutcome::Failed {
                                tool_use_id,
                                error: format!("Unknown tool: {tool_name}"),
                            }
                        }
                    };
                    // Send typed outcome through oneshot channel
                    let _ = tool_tx.send(tool_outcome);
                });

                // Forward the typed outcome to the unified outcome channel
                tokio::spawn(async move {
                    if let Ok(tool_outcome) = tool_rx.await {
                        let _ = outcome_tx.send(EffectOutcome::Tool(tool_outcome)).await;
                    }
                });

                Ok(None)
            }

            Effect::ScheduleRetry { delay, attempt } => {
                // Typed oneshot for retry timeout
                let outcome_tx = self.outcome_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = outcome_tx
                        .send(EffectOutcome::RetryTimeout { attempt })
                        .await;
                });
                Ok(None)
            }

            Effect::NotifyClient { event_type, data } => {
                match event_type.as_str() {
                    "agent_done" => {
                        let _ = self.broadcast_tx.send(SseEvent::AgentDone);
                    }
                    "state_change" => {
                        // data should contain the full state object; deserialize to typed ConvState
                        if let Some(state_val) = data.get("state") {
                            match serde_json::from_value::<ConvState>(state_val.clone()) {
                                Ok(typed_state) => {
                                    let _ = self.broadcast_tx.send(SseEvent::StateChange {
                                        state: typed_state,
                                        display_state: self.state.display_state().as_str().to_string(),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Failed to deserialize NotifyClient state_change into ConvState; \
                                         falling back to current executor state"
                                    );
                                    let _ = self.broadcast_tx.send(SseEvent::StateChange {
                                        state: self.state.clone(),
                                        display_state: self.state.display_state().as_str().to_string(),
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
                Ok(None)
            }

            Effect::PersistCheckpoint { data } => {
                use crate::state_machine::CheckpointData;
                match data {
                    CheckpointData::ToolRound {
                        assistant_message,
                        tool_results,
                    } => {
                        // Persist assistant message
                        let agent_content = MessageContent::agent(assistant_message.content);
                        let agent_msg = self
                            .storage
                            .add_message(
                                &assistant_message.message_id,
                                &self.context.conversation_id,
                                &agent_content,
                                assistant_message.display_data.as_ref(),
                                assistant_message.usage.as_ref(),
                            )
                            .await?;
                        let _ = self.broadcast_tx.send(SseEvent::Message {
                            message: agent_msg,
                        });

                        // Persist all tool results
                        for result in tool_results {
                            let tool_content = MessageContent::tool(
                                &result.tool_use_id,
                                &result.output,
                                result.is_error,
                            );
                            let tool_msg_id = format!("{}-result", result.tool_use_id);
                            let tool_msg = self
                                .storage
                                .add_message(
                                    &tool_msg_id,
                                    &self.context.conversation_id,
                                    &tool_content,
                                    result.display_data.as_ref(),
                                    None,
                                )
                                .await?;
                            let _ = self
                                .broadcast_tx
                                .send(SseEvent::Message { message: tool_msg });
                        }
                    }
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
                    let _ = self
                        .broadcast_tx
                        .send(SseEvent::Message { message: msg });
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
                tracing::info!("Aborting LLM request");
                if let Some(handle) = self.llm_task_handle.take() {
                    handle.abort();
                }
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

                // If we have a spawn_tool_id, update its message's content (for LLM history)
                // and display_data (for UI). The message was persisted as "{spawn_tool_id}-result".
                if let Some(tool_id) = spawn_tool_id {
                    use crate::state_machine::state::SubAgentOutcome;
                    let message_id = format!("{tool_id}-result");

                    // Build a human-readable summary of sub-agent outcomes for the LLM.
                    // This replaces the initial "Spawning N sub-agents..." acknowledgement so
                    // build_llm_messages_static feeds the actual results to the model.
                    let llm_content = results
                        .iter()
                        .map(|r| {
                            let outcome = match &r.outcome {
                                SubAgentOutcome::Success { result } => {
                                    format!("Result: {result}")
                                }
                                SubAgentOutcome::Failure { error, .. } => {
                                    format!("Failed: {error}")
                                }
                                SubAgentOutcome::TimedOut => {
                                    "Timed out: sub-agent exceeded its time limit".to_string()
                                }
                            };
                            format!("Task: \"{}\"\n{outcome}", r.task)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let llm_content = format!(
                        "Sub-agent results ({} completed):\n\n{llm_content}",
                        results.len()
                    );

                    if let Err(e) = self
                        .storage
                        .update_tool_message_content(&message_id, &llm_content)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            message_id = %message_id,
                            "Failed to update spawn_agents message content with sub-agent results"
                        );
                    }

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
                                let _ = self
                                    .broadcast_tx
                                    .send(SseEvent::Message { message: updated_msg });
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
                    let _ = self
                        .broadcast_tx
                        .send(SseEvent::Message { message });
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
                    state: ConvState::ContextExhausted { summary },
                    display_state: self.state.display_state().as_str().to_string(),
                });
                Ok(None)
            }

            Effect::ApproveTask {
                title,
                priority,
                plan,
            } => {
                self.execute_approve_task(title, priority, plan).await?;
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
        use crate::db::{MessageContent, ToolContent};
        use crate::llm::ImageSource;

        let db_messages = storage.get_messages(conv_id).await?;

        let mut messages = Vec::new();

        for msg in db_messages {
            match &msg.content {
                MessageContent::User(user_content) => {
                    // Use llm_text when expansion occurred (REQ-IR-001, REQ-IR-006):
                    // the model sees the fully resolved form while the DB stores the shorthand.
                    let text_for_llm = user_content.llm_text();
                    let mut content = vec![ContentBlock::text(text_for_llm)];

                    // Add images (REQ-BED-013)
                    for img in &user_content.images {
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
                    images,
                }) => {
                    // Convert stored ToolContentImages to LLM ImageSources
                    let image_sources: Vec<ImageSource> = images
                        .iter()
                        .map(|img| ImageSource::Base64 {
                            media_type: img.media_type.clone(),
                            data: img.data.clone(),
                        })
                        .collect();

                    // Tool results go in user message
                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content.clone(),
                            images: image_sources,
                            is_error: *is_error,
                        }],
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

        let handle = tokio::spawn(async move {
            // Build messages from history and add continuation request
            let mut messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to build messages for continuation");
                    let _ = event_tx.send(Event::ContinuationFailed { error: e }).await;
                    return;
                }
            };

            // Add synthetic tool results for rejected tool calls to maintain valid conversation
            // history. These tools were never executed because context was exhausted before
            // they could run.
            for rejected_tool in &rejected_tool_calls {
                messages.push(LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: rejected_tool.id.clone(),
                        content: "Tool execution was skipped — context limit reached before this tool could run.".to_string(),
                        images: vec![],
                        is_error: false,
                    }],
                });
            }

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

            match llm_client.complete(&request).await {
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
                    // Send LlmError so the state machine's AwaitingContinuation retry logic fires.
                    // The attempt field is ignored by that arm (tracked in state), so 0 is fine.
                    let _ = event_tx
                        .send(Event::LlmError {
                            message: e.message.clone(),
                            error_kind: llm_error_to_db_error(e.kind),
                            attempt: 0,
                        })
                        .await;
                }
            }
        });
        self.llm_task_handle = Some(handle);
    }

    /// REQ-BED-028: Execute git operations for task approval.
    ///
    /// Sequence: dirty tree check -> assign task ID -> mkdir tasks/ -> write task file ->
    /// git commit -> check branch collision -> create branch -> checkout -> update `conv_mode`.
    ///
    /// On failure: revert in-memory state to `AwaitingTaskApproval` so the user can retry.
    /// Collision check on retry handles partial state.
    async fn execute_approve_task(
        &mut self,
        title: String,
        priority: String,
        plan: String,
    ) -> Result<(), String> {
        let cwd = self.context.working_dir.clone();
        let conv_id = self.context.conversation_id.clone();
        let storage = self.storage.clone();

        // Clone for state revert on failure (originals moved into spawn_blocking)
        let title_backup = title.clone();
        let priority_backup = priority.clone();
        let plan_backup = plan.clone();

        // Run blocking git/fs operations on a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            execute_approve_task_blocking(&cwd, &conv_id, &title, &priority, &plan)
        })
        .await
        .map_err(|e| format!("Task approval join error: {e}"))?;

        match result {
            Ok(approval_result) => {
                // Update conversation mode to Work (includes worktree_path)
                let work_mode = crate::db::ConvMode::Work {
                    branch_name: approval_result.branch_name.clone(),
                    worktree_path: approval_result.worktree_path.clone(),
                };
                storage
                    .update_conversation_mode(&self.context.conversation_id, &work_mode)
                    .await?;

                // Update conversation CWD to the worktree path
                storage
                    .update_conversation_cwd(
                        &self.context.conversation_id,
                        &approval_result.worktree_path,
                    )
                    .await?;

                // Replace working_dir to point at the worktree directory.
                // Field-level mutation (not full replacement) so we don't lose
                // is_sub_agent, context_exhaustion_behavior, or future fields.
                self.context.working_dir =
                    std::path::PathBuf::from(&approval_result.worktree_path);

                // Upgrade tool registry from Explore to Work mode so the agent
                // gets bash, patch, etc. for the rest of this conversation.
                self.tool_executor.upgrade_to_work_mode();

                tracing::info!(
                    task_id = approval_result.task_number,
                    branch = %approval_result.branch_name,
                    worktree = %approval_result.worktree_path,
                    first_task = approval_result.first_task,
                    "Task approved — worktree created"
                );

                // Persist a system message with the branch + worktree path
                let branch_msg = format!(
                    "Task approved. You are on branch {} in {}.",
                    approval_result.branch_name, approval_result.worktree_path
                );
                let msg_id = uuid::Uuid::new_v4().to_string();
                let content = MessageContent::system(&branch_msg);
                let msg = self
                    .storage
                    .add_message(
                        &msg_id,
                        &self.context.conversation_id,
                        &content,
                        None,
                        None,
                    )
                    .await?;
                let _ = self
                    .broadcast_tx
                    .send(SseEvent::Message { message: msg });

                // Push updated conversation metadata to the client so it
                // reflects the new cwd, branch, worktree_path, and mode label
                // without requiring a reconnect.
                let _ = self.broadcast_tx.send(SseEvent::ConversationUpdate {
                    update: crate::runtime::ConversationMetadataUpdate {
                        cwd: Some(approval_result.worktree_path.clone()),
                        branch_name: Some(approval_result.branch_name.clone()),
                        worktree_path: Some(approval_result.worktree_path.clone()),
                        conv_mode_label: Some("Work".to_string()),
                    },
                });

                Ok(())
            }
            Err(e) => {
                tracing::error!(error = %e, "Task approval git operations failed");

                // Revert in-memory state to AwaitingTaskApproval so the user can retry.
                // The DB still has AwaitingTaskApproval (PersistState hasn't run for the
                // new Idle state yet), so this keeps memory and DB consistent.
                self.state = ConvState::AwaitingTaskApproval {
                    title: title_backup,
                    priority: priority_backup,
                    plan: plan_backup,
                };

                // Broadcast an error so the UI knows, but don't propagate — the
                // conversation stays in AwaitingTaskApproval for retry.
                let _ = self.broadcast_tx.send(SseEvent::Error {
                    message: format!("Task approval failed: {e}"),
                });

                Ok(())
            }
        }
    }
}

/// Result of a successful task approval
struct TaskApprovalResult {
    task_number: u32,
    branch_name: String,
    first_task: bool,
    /// Absolute path to the git worktree created for this conversation
    worktree_path: String,
}

/// Derive a slug from a task title: lowercase, spaces to hyphens, strip non-alphanumeric
/// except hyphens, truncate at 40 chars, trim trailing hyphens.
fn derive_slug(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let truncated = if slug.len() > 40 {
        // Truncate at last hyphen within 40 chars to avoid cutting mid-word
        let s = &slug[..40];
        s.rfind('-').map_or(s, |i| &s[..i]).to_string()
    } else {
        slug
    };
    truncated.trim_end_matches('-').to_string()
}

/// Scan `tasks/` directory for the highest existing task number (NNNN prefix).
fn scan_highest_task_number(tasks_dir: &std::path::Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(tasks_dir) else {
        return 0;
    };
    let mut max_num: u32 = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Extract leading digits (up to 4)
        if let Some(num_str) = name.split('-').next() {
            if let Ok(n) = num_str.parse::<u32>() {
                if n > max_num {
                    max_num = n;
                }
            }
        }
    }
    max_num
}

/// Run a git command in the given directory, returning stdout on success or an error message.
fn run_git(cwd: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Failed to run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}

/// Global mutex serializing the scan-tasks + write + commit sequence.
/// Task approval is rare; a single mutex is sufficient.
static TASK_APPROVAL_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Blocking implementation of approve task git operations.
/// Runs on a blocking thread via `spawn_blocking`.
#[allow(clippy::too_many_lines)] // Sequential git flow; splitting hurts readability
fn execute_approve_task_blocking(
    cwd: &std::path::Path,
    conv_id: &str,
    title: &str,
    priority: &str,
    plan: &str,
) -> Result<TaskApprovalResult, String> {
    use std::io::Write;

    // Serialize the entire scan + write + commit sequence to prevent
    // concurrent approvals from getting the same task number.
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // 1. Dirty tree check FIRST — before any filesystem changes to avoid partial state
    let status = run_git(cwd, &["status", "--porcelain"])?;
    if !status.is_empty() {
        return Err("Please commit or stash your changes before approving. \
             The working tree has uncommitted modifications."
            .to_string());
    }

    let tasks_dir = cwd.join("tasks");

    // Track whether tasks/ existed before we create it
    let first_task = !tasks_dir.exists();

    // 2. Assign task ID (scan for highest existing number)
    let next_number = scan_highest_task_number(&tasks_dir) + 1;

    // 3. Create tasks/ directory if needed
    if first_task {
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks/ directory: {e}"))?;
        tracing::info!("Created tasks/ directory (first task for this project)");
    }

    // 4. Derive slug from title
    let slug = derive_slug(title);
    if slug.is_empty() {
        return Err("Cannot derive a valid slug from the task title".to_string());
    }

    // 5. Write task file
    let filename = format!("{next_number:04}-{priority}-in-progress--{slug}.md");
    let filepath = tasks_dir.join(&filename);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let branch_name = format!("task-{next_number:04}-{slug}");

    // Escape double quotes in title for YAML frontmatter
    let escaped_title = title.replace('"', r#"\""#);

    let task_content = format!(
        "---\n\
         created: {today}\n\
         number: {next_number}\n\
         priority: {priority}\n\
         status: in-progress\n\
         slug: {slug}\n\
         title: \"{escaped_title}\"\n\
         branch: {branch_name}\n\
         conversation: {conv_id}\n\
         ---\n\
         \n\
         # {title}\n\
         \n\
         ## Plan\n\
         \n\
         {plan}\n\
         \n\
         ## Progress\n\
         \n"
    );

    let mut file = std::fs::File::create(&filepath)
        .map_err(|e| format!("Failed to create task file {}: {e}", filepath.display()))?;
    file.write_all(task_content.as_bytes())
        .map_err(|e| format!("Failed to write task file: {e}"))?;
    tracing::info!(file = %filepath.display(), "Task file written");

    // 5. Ensure .gitignore contains .phoenix/ (worktree dir lives there)
    let gitignore_path = cwd.join(".gitignore");
    let gitignore_needs_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)
            .map_err(|e| format!("Failed to read .gitignore: {e}"))?;
        !content.lines().any(|line| line.trim() == ".phoenix/")
    } else {
        true
    };

    if gitignore_needs_update {
        use std::io::Write as _;
        // Ensure we don't corrupt the last line if .gitignore lacks a trailing newline
        let needs_leading_newline = gitignore_path.exists()
            && std::fs::read(&gitignore_path)
                .ok()
                .is_some_and(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to open .gitignore: {e}"))?;
        if needs_leading_newline {
            writeln!(f).map_err(|e| format!("Failed to write .gitignore: {e}"))?;
        }
        writeln!(f, ".phoenix/")
            .map_err(|e| format!("Failed to write .gitignore: {e}"))?;
        run_git(cwd, &["add", ".gitignore"])?;
        tracing::info!("Added .phoenix/ to .gitignore");
    }

    // 6. Git commit the task file (and .gitignore if modified)
    let relative_path = format!("tasks/{filename}");
    run_git(cwd, &["add", &relative_path])?;
    let commit_msg = format!("task {next_number:04}: {title}");
    // Use `git commit` without `--only` so both staged files are included
    run_git(cwd, &["commit", "-m", &commit_msg])?;
    tracing::info!(commit_msg = %commit_msg, "Task file committed");

    // 7. Branch collision check
    let branch_exists = run_git(cwd, &["rev-parse", "--verify", &branch_name]).is_ok();
    if branch_exists {
        // Check if fully merged into current branch
        let merge_base = run_git(cwd, &["merge-base", "--is-ancestor", &branch_name, "HEAD"]);
        if merge_base.is_ok() {
            // Fully merged — safe to delete
            run_git(cwd, &["branch", "-d", &branch_name])?;
            tracing::info!(branch = %branch_name, "Deleted stale fully-merged branch");
        } else {
            return Err(format!(
                "Branch '{branch_name}' already exists and is not fully merged. \
                 Please resolve this manually before approving."
            ));
        }
    }

    // 8. Create worktree with new branch (atomic: creates branch + attaches worktree)
    let phoenix_dir = cwd.join(".phoenix").join("worktrees");
    std::fs::create_dir_all(&phoenix_dir)
        .map_err(|e| format!("Failed to create .phoenix/worktrees/: {e}"))?;

    let worktree_path = phoenix_dir.join(conv_id);
    let worktree_path_str = worktree_path.to_string_lossy().to_string();
    run_git(
        cwd,
        &["worktree", "add", &worktree_path_str, "-b", &branch_name],
    )?;
    tracing::info!(
        branch = %branch_name,
        worktree = %worktree_path_str,
        "Created git worktree"
    );

    Ok(TaskApprovalResult {
        task_number: next_number,
        branch_name,
        first_task,
        worktree_path: worktree_path_str,
    })
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
    // Explicit match arms — no catch-all. The compiler enforces exhaustiveness.
    match kind {
        crate::llm::LlmErrorKind::Auth => crate::db::ErrorKind::Auth,
        crate::llm::LlmErrorKind::RateLimit => crate::db::ErrorKind::RateLimit,
        crate::llm::LlmErrorKind::Network => crate::db::ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => crate::db::ErrorKind::InvalidRequest,
        crate::llm::LlmErrorKind::ServerError => crate::db::ErrorKind::ServerError,
        crate::llm::LlmErrorKind::ContentFilter => crate::db::ErrorKind::ContentFilter,
        crate::llm::LlmErrorKind::ContextWindowExceeded => crate::db::ErrorKind::ContextExhausted,
    }
}

/// Convert an LLM error into a typed `LlmOutcome`.
/// Explicit match arms — the compiler enforces exhaustiveness.
fn llm_error_to_outcome(error: crate::llm::LlmError) -> LlmOutcome {
    use crate::llm::LlmErrorKind;
    match error.kind {
        LlmErrorKind::RateLimit => LlmOutcome::RateLimited { retry_after: None },
        LlmErrorKind::ServerError => LlmOutcome::ServerError {
            status: 500,
            body: error.message,
        },
        LlmErrorKind::Network => LlmOutcome::NetworkError {
            message: error.message,
        },
        LlmErrorKind::ContextWindowExceeded => LlmOutcome::TokenBudgetExceeded,
        LlmErrorKind::Auth => LlmOutcome::AuthError {
            message: error.message,
        },
        LlmErrorKind::InvalidRequest | LlmErrorKind::ContentFilter => LlmOutcome::RequestRejected {
            message: error.message,
        },
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
            "ServerError must map to ServerError"
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ContentFilter),
            crate::db::ErrorKind::ContentFilter
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ContextWindowExceeded),
            crate::db::ErrorKind::ContextExhausted
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
