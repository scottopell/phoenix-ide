//! Conversation runtime executor

use super::traits::{LlmClient, Storage, ToolExecutor};
use super::SseEvent;
use crate::db::{MessageContent, ToolResult};
use crate::llm::{ContentBlock, LlmMessage, LlmRequest, MessageRole, SystemContent};
use crate::state_machine::state::{ToolCall, ToolInput};
use crate::state_machine::{transition, ConvContext, ConvState, Effect, Event};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// System prompt for conversations
const SYSTEM_PROMPT: &str = r"You are a helpful AI assistant with access to tools for executing code, editing files, and searching codebases. Use tools when appropriate to accomplish tasks.";

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
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    broadcast_tx: broadcast::Sender<SseEvent>,
    /// Token to cancel running tool execution
    tool_cancel_token: Option<CancellationToken>,
    /// Token to cancel running LLM request
    llm_cancel_token: Option<CancellationToken>,
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    pub fn new(
        context: ConvContext,
        state: ConvState,
        storage: S,
        llm_client: L,
        tool_executor: T,
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
            event_rx,
            event_tx,
            broadcast_tx,
            tool_cancel_token: None,
            llm_cancel_token: None,
        }
    }

    pub async fn run(mut self) {
        tracing::info!(conv_id = %self.context.conversation_id, "Starting conversation runtime");

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
            self.state = result.new_state.clone();

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

    /// Execute an effect and optionally return a generated event
    #[allow(clippy::too_many_lines)] // Effect handling is inherently complex
    async fn execute_effect(&mut self, effect: Effect) -> Result<Option<Event>, String> {
        match effect {
            Effect::PersistMessage {
                content,
                display_data,
                usage_data,
            } => {
                let msg = self
                    .storage
                    .add_message(
                        &self.context.conversation_id,
                        &content,
                        display_data.as_ref(),
                        usage_data.as_ref(),
                    )
                    .await?;

                // Broadcast to clients
                let msg_json = serde_json::to_value(&msg).unwrap_or(Value::Null);
                let _ = self
                    .broadcast_tx
                    .send(SseEvent::Message { message: msg_json });
                Ok(None)
            }

            Effect::PersistState => {
                let state_data = match &self.state {
                    ConvState::LlmRequesting { attempt } => Some(json!({ "attempt": attempt })),
                    ConvState::ToolExecuting {
                        current_tool,
                        remaining_tools,
                        ..
                    } => Some(json!({
                        "current_tool_id": current_tool.id,
                        "current_tool_name": current_tool.name(),
                        "remaining_count": remaining_tools.len()
                    })),
                    ConvState::Error {
                        message,
                        error_kind,
                    } => Some(json!({
                        "message": message,
                        "error_kind": format!("{:?}", error_kind)
                    })),
                    _ => None,
                };

                self.storage
                    .update_state(
                        &self.context.conversation_id,
                        &to_db_state(&self.state),
                        state_data.as_ref(),
                    )
                    .await?;

                // Broadcast state change
                let _ = self.broadcast_tx.send(SseEvent::StateChange {
                    state: self.state.to_db_state().to_string(),
                    state_data: state_data.unwrap_or(Value::Null),
                });
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
                let current_attempt = match &self.state {
                    ConvState::LlmRequesting { attempt } => *attempt,
                    _ => 1,
                };

                tokio::spawn(async move {
                    tracing::info!("Making LLM request (background)");

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

                    // Build request
                    let request = LlmRequest {
                        system: vec![SystemContent::cached(SYSTEM_PROMPT)],
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
                // Create cancellation token for this tool execution
                let cancel_token = CancellationToken::new();
                self.tool_cancel_token = Some(cancel_token.clone());

                // Spawn tool execution as background task
                let tool_executor = self.tool_executor.clone();
                let event_tx = self.event_tx.clone();
                let tool_use_id = tool.id.clone();
                let tool_name = tool.name().to_string();
                let tool_input = tool.input.to_value();

                tokio::spawn(async move {
                    tracing::info!(tool = %tool_name, id = %tool_use_id, "Executing tool (background)");

                    // Pass cancellation token to tool for subprocess management
                    let output = tool_executor
                        .execute(&tool_name, tool_input, cancel_token)
                        .await;

                    let result = match output {
                        Some(out) => {
                            // Check if the tool was cancelled
                            if out.output.contains("[command cancelled]") {
                                tracing::info!(tool_id = %tool_use_id, "Tool cancelled");
                                let _ = event_tx.send(Event::ToolAborted { tool_use_id }).await;
                                return;
                            }
                            ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                success: out.success,
                                output: out.output,
                                is_error: !out.success,
                            }
                        }
                        None => ToolResult::error(
                            tool_use_id.clone(),
                            format!("Unknown tool: {tool_name}"),
                        ),
                    };
                    let _ = event_tx
                        .send(Event::ToolComplete { tool_use_id, result })
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
                        if let Some(state) = data.get("state").and_then(|s| s.as_str()) {
                            let _ = self.broadcast_tx.send(SseEvent::StateChange {
                                state: state.to_string(),
                                state_data: data.get("state_data").cloned().unwrap_or(Value::Null),
                            });
                        }
                    }
                    _ => {}
                }
                Ok(None)
            }

            Effect::PersistToolResults { results } => {
                for result in results {
                    let content = MessageContent::tool(
                        &result.tool_use_id,
                        &result.output,
                        result.is_error,
                    );
                    let msg = self
                        .storage
                        .add_message(
                            &self.context.conversation_id,
                            &content,
                            None,
                            None,
                        )
                        .await?;

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

            Effect::SpawnSubAgent(spec) => {
                // TODO: Implement sub-agent spawning
                tracing::info!(agent_id = %spec.agent_id, task = %spec.task, "Spawning sub-agent");
                // For now, just log - full implementation requires runtime manager
                Ok(None)
            }

            Effect::CancelSubAgents { ids } => {
                // TODO: Implement sub-agent cancellation
                tracing::info!(?ids, "Cancelling sub-agents");
                // For now, just log - full implementation requires runtime manager
                Ok(None)
            }

            Effect::NotifyParent { outcome } => {
                // TODO: Implement parent notification
                tracing::info!(?outcome, "Notifying parent of sub-agent completion");
                // For now, just log - full implementation requires parent event channel
                Ok(None)
            }

            Effect::PersistSubAgentResults { results } => {
                // Persist aggregated sub-agent results as a system message
                let aggregated = serde_json::json!({
                    "sub_agent_results": results
                });
                let content = crate::db::MessageContent::system(
                    serde_json::to_string_pretty(&aggregated).unwrap_or_default()
                );
                self.storage
                    .add_message(
                        &self.context.conversation_id,
                        &content,
                        None,
                        None,
                    )
                    .await?;
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
        use crate::db::{MessageContent, UserContent, ToolContent};
        
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

                MessageContent::Tool(ToolContent { tool_use_id, content, is_error }) => {
                    // Tool results go in user message
                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::tool_result(
                            tool_use_id,
                            content,
                            *is_error,
                        )],
                    });
                }

                // Ignore system and error messages
                MessageContent::System(_) | MessageContent::Error(_) => {}
            }
        }

        Ok(messages)
    }
}

fn to_db_state(state: &ConvState) -> crate::db::ConversationState {
    match state {
        ConvState::Idle => crate::db::ConversationState::Idle,
        ConvState::AwaitingLlm => crate::db::ConversationState::AwaitingLlm,
        ConvState::LlmRequesting { attempt } => {
            crate::db::ConversationState::LlmRequesting { attempt: *attempt }
        }
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            completed_results,
            pending_sub_agents,
        } => crate::db::ConversationState::ToolExecuting {
            current_tool: current_tool.clone(),
            remaining_tools: remaining_tools.clone(),
            completed_results: completed_results.clone(),
            pending_sub_agents: pending_sub_agents.clone(),
        },
        ConvState::CancellingLlm => crate::db::ConversationState::CancellingLlm,
        ConvState::CancellingTool {
            tool_use_id,
            skipped_tools,
            completed_results,
        } => crate::db::ConversationState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped_tools.clone(),
            completed_results: completed_results.clone(),
        },
        ConvState::AwaitingSubAgents {
            pending_ids,
            completed_results,
        } => crate::db::ConversationState::AwaitingSubAgents {
            pending_ids: pending_ids.clone(),
            completed_results: completed_results.clone(),
        },
        ConvState::CancellingSubAgents {
            pending_ids,
            completed_results,
        } => crate::db::ConversationState::CancellingSubAgents {
            pending_ids: pending_ids.clone(),
            completed_results: completed_results.clone(),
        },
        ConvState::Completed { result } => crate::db::ConversationState::Completed {
            result: result.clone(),
        },
        ConvState::Failed { error, error_kind } => crate::db::ConversationState::Failed {
            error: error.clone(),
            error_kind: error_kind.clone(),
        },
        ConvState::Error {
            message,
            error_kind,
        } => crate::db::ConversationState::Error {
            message: message.clone(),
            error_kind: error_kind.clone(),
        },
    }
}

fn llm_error_to_db_error(kind: crate::llm::LlmErrorKind) -> crate::db::ErrorKind {
    match kind {
        crate::llm::LlmErrorKind::Auth => crate::db::ErrorKind::Auth,
        crate::llm::LlmErrorKind::RateLimit => crate::db::ErrorKind::RateLimit,
        crate::llm::LlmErrorKind::Network => crate::db::ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => crate::db::ErrorKind::InvalidRequest,
        _ => crate::db::ErrorKind::Unknown,
    }
}
