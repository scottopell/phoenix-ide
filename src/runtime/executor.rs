//! Conversation runtime executor

use super::traits::{LlmClient, Storage, ToolExecutor};
use super::SseEvent;
use crate::db::{MessageType, ToolResult};
use crate::llm::{ContentBlock, LlmMessage, LlmRequest, MessageRole, SystemContent};
use crate::state_machine::state::{ToolCall, ToolInput};
use crate::state_machine::{transition, ConvContext, ConvState, Effect, Event};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

/// System prompt for conversations
const SYSTEM_PROMPT: &str = r"You are a helpful AI assistant with access to tools for executing code, editing files, and searching codebases. Use tools when appropriate to accomplish tasks.";

/// Generic conversation runtime that can work with any storage, LLM, and tool implementations
pub struct ConversationRuntime<S, L, T>
where
    S: Storage,
    L: LlmClient,
    T: ToolExecutor,
{
    context: ConvContext,
    state: ConvState,
    storage: S,
    llm_client: L,
    tool_executor: T,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    broadcast_tx: broadcast::Sender<SseEvent>,
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage,
    L: LlmClient,
    T: ToolExecutor,
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
            llm_client,
            tool_executor,
            event_rx,
            event_tx,
            broadcast_tx,
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
                msg_type,
                content,
                display_data,
                usage_data,
            } => {
                let msg = self
                    .storage
                    .add_message(
                        &self.context.conversation_id,
                        msg_type,
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
                // Make LLM request and return generated event
                Ok(Some(self.make_llm_request_event().await))
            }

            Effect::ExecuteTool { tool } => {
                // Execute tool and return generated event
                Ok(Some(self.execute_tool_event(tool).await))
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
                    let content = json!({
                        "tool_use_id": result.tool_use_id,
                        "content": result.output,
                        "is_error": result.is_error
                    });
                    let msg = self
                        .storage
                        .add_message(
                            &self.context.conversation_id,
                            MessageType::Tool,
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

            Effect::SpawnSubAgent { .. } => {
                // Sub-agent spawning is not implemented in MVP
                tracing::warn!("Sub-agent spawning not implemented");
                Ok(None)
            }

            Effect::AbortTool { tool_use_id } => {
                // Signal abort to running tool and return ToolAborted event
                tracing::info!(tool_id = %tool_use_id, "Aborting tool execution");
                // TODO: Actually abort the running task when we implement spawned execution
                // For now, just return the aborted event
                Ok(Some(Event::ToolAborted { tool_use_id }))
            }
        }
    }

    /// Make LLM request and return the resulting event
    async fn make_llm_request_event(&mut self) -> Event {
        // Build messages from history
        let messages = match self.build_llm_messages().await {
            Ok(m) => m,
            Err(e) => {
                return Event::LlmError {
                    message: e,
                    error_kind: crate::db::ErrorKind::Unknown,
                    attempt: 1,
                };
            }
        };

        // Build request
        let request = LlmRequest {
            system: vec![SystemContent::cached(SYSTEM_PROMPT)],
            messages,
            tools: self.tool_executor.definitions(),
            max_tokens: Some(8192),
        };

        // Make request
        match self.llm_client.complete(&request).await {
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
            Err(e) => {
                // Determine attempt from current state
                let attempt = match &self.state {
                    ConvState::LlmRequesting { attempt } => *attempt,
                    _ => 1,
                };

                Event::LlmError {
                    message: e.message.clone(),
                    error_kind: llm_error_to_db_error(e.kind),
                    attempt,
                }
            }
        }
    }

    async fn build_llm_messages(&self) -> Result<Vec<LlmMessage>, String> {
        let db_messages = self
            .storage
            .get_messages(&self.context.conversation_id)
            .await?;

        let mut messages = Vec::new();

        for msg in db_messages {
            match msg.message_type {
                MessageType::User => {
                    let mut content = vec![];

                    // Extract text
                    if let Some(text) = msg.content.get("text").and_then(|t| t.as_str()) {
                        content.push(ContentBlock::text(text));
                    }

                    // Extract images (REQ-BED-013)
                    if let Some(images) = msg.content.get("images").and_then(|i| i.as_array()) {
                        for img in images {
                            if let (Some(data), Some(media_type)) = (
                                img.get("data").and_then(|d| d.as_str()),
                                img.get("media_type").and_then(|m| m.as_str()),
                            ) {
                                content.push(ContentBlock::Image {
                                    source: crate::llm::ImageSource::Base64 {
                                        media_type: media_type.to_string(),
                                        data: data.to_string(),
                                    },
                                });
                            }
                        }
                    }

                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content,
                    });
                }

                MessageType::Agent => {
                    // Parse content blocks from stored JSON
                    let content: Vec<ContentBlock> = serde_json::from_value(msg.content.clone())
                        .unwrap_or_else(|_| vec![ContentBlock::text(msg.content.to_string())]);

                    messages.push(LlmMessage {
                        role: MessageRole::Assistant,
                        content,
                    });
                }

                MessageType::Tool => {
                    // Tool results go in user message
                    let tool_use_id = msg
                        .content
                        .get("tool_use_id")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let content_str = msg
                        .content
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let is_error = msg
                        .content
                        .get("is_error")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);

                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::tool_result(
                            tool_use_id,
                            content_str,
                            is_error,
                        )],
                    });
                }

                _ => {} // Ignore system and error messages
            }
        }

        Ok(messages)
    }

    /// Execute tool and return the resulting event
    async fn execute_tool_event(&self, tool: ToolCall) -> Event {
        let tool_use_id = tool.id.clone();
        let name = tool.name().to_string();
        let input = tool.input.to_value();

        tracing::info!(tool = %name, id = %tool_use_id, "Executing tool");

        let output = self.tool_executor.execute(&name, input).await;

        let result = match output {
            Some(out) => ToolResult {
                tool_use_id: tool_use_id.clone(),
                success: out.success,
                output: out.output,
                is_error: !out.success,
            },
            None => ToolResult::error(tool_use_id.clone(), format!("Unknown tool: {name}")),
        };

        Event::ToolComplete {
            tool_use_id,
            result,
        }
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
        } => crate::db::ConversationState::ToolExecuting {
            current_tool: current_tool.clone(),
            remaining_tools: remaining_tools.clone(),
            completed_results: completed_results.clone(),
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
