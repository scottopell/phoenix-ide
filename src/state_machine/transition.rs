//! Pure state transition functions
//!
//! Two entry points:
//! - `transition()`: handles all events (user events + executor events)
//! - `handle_outcome()`: handles executor-produced outcomes via typed channels
//!
//! REQ-BED-001: Pure State Transitions
//! REQ-BED-002: User Message Handling
//! REQ-BED-003: LLM Response Processing
//! REQ-BED-004: Tool Execution Coordination
//! REQ-BED-005: Cancellation Handling
//! REQ-BED-006: Error Recovery

use super::effect::{compute_bash_display_data, CheckpointData};
use super::outcome::{EffectOutcome, InvalidOutcome, LlmOutcome, PersistOutcome, ToolOutcome};
use super::state::{
    AssistantMessage, ContextExhaustionBehavior, PendingSubAgent, SubAgentResult,
    TaskApprovalOutcome, ToolCall, ToolInput,
};
use super::{ConvContext, ConvState, Effect, Event};
use crate::db::{ErrorKind, ToolResult, UsageData};
use serde_json::{json, Value};
use std::time::Duration;
use thiserror::Error;

const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Result of a state transition
#[derive(Debug)]
pub struct TransitionResult {
    pub new_state: ConvState,
    pub effects: Vec<Effect>,
}

impl TransitionResult {
    pub fn new(state: ConvState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    pub fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    #[allow(dead_code)] // Builder method
    pub fn with_effects(mut self, effects: impl IntoIterator<Item = Effect>) -> Self {
        self.effects.extend(effects);
        self
    }
}

/// Errors that can occur during transition
#[derive(Debug, Error)]
pub enum TransitionError {
    #[error("Agent is busy, cannot accept message (cancel current operation first)")]
    AgentBusy,
    #[error("Cancellation in progress")]
    CancellationInProgress,
    #[error("Context window exhausted, please start a new conversation")]
    ContextExhausted,
    #[error("Conversation is awaiting task approval")]
    AwaitingTaskApproval,
    #[error("Conversation is awaiting user response to questions")]
    AwaitingUserResponse,
    #[error("Conversation has reached terminal state (completed or abandoned)")]
    ConversationTerminal,
    #[error("Invalid transition: {0}")]
    InvalidTransition(String),
}

/// Pure transition function
///
/// REQ-BED-001: This function is pure - given the same inputs, it always
/// produces the same outputs, with no I/O side effects.
#[allow(clippy::too_many_lines)] // State machine is inherently complex
pub fn transition(
    state: &ConvState,
    context: &ConvContext,
    event: Event,
) -> Result<TransitionResult, TransitionError> {
    match (state, event) {
        // ============================================================
        // User Message Handling (REQ-BED-002)
        // ============================================================

        // Idle or Error + UserMessage -> LlmRequesting (recovery from Error, REQ-BED-006)
        (
            ConvState::Idle | ConvState::Error { .. },
            Event::UserMessage {
                text,
                llm_text,
                images,
                message_id,
                user_agent,
                skill_invocation,
            },
        ) => Ok(
            TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_user_message(
                    text,
                    llm_text,
                    images,
                    message_id,
                    user_agent,
                    skill_invocation,
                ))
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        ),

        // Busy states + UserMessage -> Reject (REQ-BED-002)
        (
            ConvState::LlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::AwaitingSubAgents { .. },
            Event::UserMessage { .. },
        ) => Err(TransitionError::AgentBusy),

        // AwaitingTaskApproval + UserMessage/UserTriggerContinuation -> Reject (REQ-BED-028)
        (
            ConvState::AwaitingTaskApproval { .. },
            Event::UserMessage { .. } | Event::UserTriggerContinuation,
        ) => Err(TransitionError::AwaitingTaskApproval),

        // AwaitingUserResponse + UserMessage -> Reject (REQ-AUQ-001)
        (
            ConvState::AwaitingUserResponse { .. },
            Event::UserMessage { .. } | Event::UserTriggerContinuation,
        ) => Err(TransitionError::AwaitingUserResponse),

        (
            ConvState::CancellingTool { .. } | ConvState::CancellingSubAgents { .. },
            Event::UserMessage { .. },
        ) => Err(TransitionError::CancellationInProgress),

        // ============================================================
        // LLM Response Processing (REQ-BED-003)
        // ============================================================

        // LlmRequesting + LlmResponse with tools -> ToolExecuting (or terminal for sub-agents)
        (
            ConvState::LlmRequesting { .. },
            Event::LlmResponse {
                content,
                tool_calls,
                end_turn: _,
                usage: usage_data,
            },
        ) => {
            // REQ-BED-028: Intercept propose_task BEFORE context exhaustion check.
            // If the LLM proposes a task at >90% context, we still want to surface it
            // for approval rather than diverting to the continuation flow.
            let propose_task_tool = tool_calls
                .iter()
                .find(|t| matches!(t.input, ToolInput::ProposeTask(_)));
            if let Some(tool) = propose_task_tool {
                if tool_calls.len() > 1 {
                    let msg = "propose_task must be the only tool in response".to_string();
                    return Ok(TransitionResult::new(ConvState::Error {
                        message: msg.clone(),
                        error_kind: ErrorKind::InvalidRequest,
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change(
                        "error",
                        json!({ "message": msg }),
                    )));
                }
                if let ToolInput::ProposeTask(ref input) = tool.input {
                    let tool_result = ToolResult::success(
                        tool.id.clone(),
                        "Plan submitted for review".to_string(),
                    );
                    let display_data = compute_bash_display_data(&content, &context.working_dir);
                    let assistant_message =
                        AssistantMessage::new(content, Some(usage_data), display_data);
                    let checkpoint =
                        CheckpointData::tool_round(assistant_message, vec![tool_result])
                            .expect("propose_task produces exactly one tool_use and one result");

                    return Ok(TransitionResult::new(ConvState::AwaitingTaskApproval {
                        title: input.title.clone(),
                        priority: input.priority.clone(),
                        plan: input.plan.clone(),
                    })
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change(
                        "awaiting_task_approval",
                        json!({
                            "title": input.title,
                            "priority": input.priority,
                            "plan": input.plan
                        }),
                    )));
                }
                unreachable!("propose_task_tool matched but input was not ProposeTask");
            }

            // REQ-AUQ-001: Intercept ask_user_question
            let ask_question_tool = tool_calls
                .iter()
                .find(|t| matches!(t.input, ToolInput::AskUserQuestion(_)));
            if let Some(tool) = ask_question_tool {
                if tool_calls.len() > 1 {
                    let msg = "ask_user_question must be the only tool in response".to_string();
                    return Ok(TransitionResult::new(ConvState::Error {
                        message: msg.clone(),
                        error_kind: ErrorKind::InvalidRequest,
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change(
                        "error",
                        json!({ "message": msg }),
                    )));
                }
                if let ToolInput::AskUserQuestion(ref input) = tool.input {
                    let tool_result = ToolResult::success(
                        tool.id.clone(),
                        "Awaiting user response. See following message for answers.".to_string(),
                    );
                    let display_data = compute_bash_display_data(&content, &context.working_dir);
                    let assistant_message =
                        AssistantMessage::new(content, Some(usage_data), display_data);
                    let checkpoint =
                        CheckpointData::tool_round(assistant_message, vec![tool_result]).expect(
                            "ask_user_question produces exactly one tool_use and one result",
                        );

                    return Ok(TransitionResult::new(ConvState::AwaitingUserResponse {
                        questions: input.questions.clone(),
                        tool_use_id: tool.id.clone(),
                    })
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change(
                        "awaiting_user_response",
                        json!({ "questions": input.questions }),
                    )));
                }
                unreachable!("ask_question_tool matched but input was not AskUserQuestion");
            }

            // REQ-BED-019: Check context threshold BEFORE tool execution
            // (but after propose_task interception above)
            if should_trigger_continuation(&usage_data, context.context_window) {
                return Ok(handle_context_exhaustion(
                    context, content, tool_calls, usage_data,
                ));
            }

            if tool_calls.is_empty() && context.is_sub_agent {
                // Sub-agent returned text without calling submit_result.
                // Treat as implicit completion — the text IS the result.
                use crate::state_machine::state::SubAgentOutcome;
                let result_text = extract_text_from_content(&content);
                let mut tr = TransitionResult::new(ConvState::Completed {
                    result: result_text.clone(),
                });
                // Only persist the agent message if there's actual content
                // (empty content = model had nothing to say, don't poison history)
                if !content.is_empty() {
                    tr = tr.with_effect(Effect::persist_agent_message(
                        content,
                        Some(usage_data),
                        &context.working_dir,
                    ));
                }
                Ok(tr
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::NotifyParent {
                        outcome: SubAgentOutcome::Success {
                            result: result_text,
                        },
                    }))
            } else if tool_calls.is_empty() && content.is_empty() {
                // Empty content, no tools — model had nothing to say (documented
                // Anthropic behavior after tool results). Transition to Idle without
                // persisting an empty agent message that would poison the history.
                tracing::debug!("LLM returned end_turn with empty content — no message to persist");
                Ok(TransitionResult::new(ConvState::Idle)
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else if tool_calls.is_empty() {
                // No tools, text response -> Idle
                Ok(TransitionResult::new(ConvState::Idle)
                    .with_effect(Effect::persist_agent_message(
                        content,
                        Some(usage_data),
                        &context.working_dir,
                    ))
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else if context.is_sub_agent {
                // Check for terminal tools (submit_result/submit_error)
                let terminal_tool = tool_calls.iter().find(|t| t.input.is_terminal_tool());

                if let Some(tool) = terminal_tool {
                    // Terminal tool must be the only tool
                    if tool_calls.len() > 1 {
                        use crate::state_machine::state::SubAgentOutcome;
                        let msg = "submit_result/submit_error must be the only tool in response"
                            .to_string();
                        return Ok(TransitionResult::new(ConvState::Failed {
                            error: msg.clone(),
                            error_kind: ErrorKind::InvalidRequest,
                        })
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::NotifyParent {
                            outcome: SubAgentOutcome::Failure {
                                error: msg,
                                error_kind: ErrorKind::InvalidRequest,
                            },
                        }));
                    }

                    // Transition directly to terminal state
                    match &tool.input {
                        crate::state_machine::state::ToolInput::SubmitResult(input) => {
                            use crate::state_machine::state::SubAgentOutcome;
                            Ok(TransitionResult::new(ConvState::Completed {
                                result: input.result.clone(),
                            })
                            .with_effect(Effect::persist_agent_message(
                                content,
                                Some(usage_data),
                                &context.working_dir,
                            ))
                            .with_effect(Effect::PersistState)
                            .with_effect(Effect::NotifyParent {
                                outcome: SubAgentOutcome::Success {
                                    result: input.result.clone(),
                                },
                            }))
                        }
                        crate::state_machine::state::ToolInput::SubmitError(input) => {
                            use crate::state_machine::state::SubAgentOutcome;
                            Ok(TransitionResult::new(ConvState::Failed {
                                error: input.error.clone(),
                                error_kind: ErrorKind::SubAgentError,
                            })
                            .with_effect(Effect::persist_agent_message(
                                content,
                                Some(usage_data),
                                &context.working_dir,
                            ))
                            .with_effect(Effect::PersistState)
                            .with_effect(Effect::NotifyParent {
                                outcome: SubAgentOutcome::Failure {
                                    error: input.error.clone(),
                                    error_kind: ErrorKind::SubAgentError,
                                },
                            }))
                        }
                        _ => unreachable!("is_terminal_tool returned true for non-terminal tool"),
                    }
                } else {
                    // Normal tool execution for sub-agent
                    let first = tool_calls[0].clone();
                    let rest = tool_calls[1..].to_vec();
                    let remaining_count = rest.len();
                    let display_data = compute_bash_display_data(&content, &context.working_dir);
                    let assistant_message =
                        AssistantMessage::new(content, Some(usage_data), display_data);

                    Ok(TransitionResult::new(ConvState::ToolExecuting {
                        current_tool: first.clone(),
                        remaining_tools: rest,
                        completed_results: vec![],
                        pending_sub_agents: vec![],
                        assistant_message,
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(notify_tool_executing(
                        first.name(),
                        &first.id,
                        remaining_count,
                        0,
                    ))
                    .with_effect(Effect::execute_tool(first)))
                }
            } else {
                // Has tools -> ToolExecuting
                let first = tool_calls[0].clone();
                let rest = tool_calls[1..].to_vec();
                let remaining_count = rest.len();
                let display_data = compute_bash_display_data(&content, &context.working_dir);
                let assistant_message =
                    AssistantMessage::new(content, Some(usage_data), display_data);

                Ok(TransitionResult::new(ConvState::ToolExecuting {
                    current_tool: first.clone(),
                    remaining_tools: rest,
                    completed_results: vec![],
                    pending_sub_agents: vec![],
                    assistant_message,
                })
                .with_effect(Effect::PersistState)
                .with_effect(notify_tool_executing(
                    first.name(),
                    &first.id,
                    remaining_count,
                    0,
                ))
                .with_effect(Effect::execute_tool(first)))
            }
        }

        // ============================================================
        // Error Handling and Retry (REQ-BED-006)
        // ============================================================

        // LlmRequesting + LlmError (retryable) -> LlmRequesting with incremented attempt
        (
            ConvState::LlmRequesting { attempt },
            Event::LlmError {
                message: _,
                error_kind,
                ..
            },
        ) if error_kind.is_retryable() && *attempt < MAX_RETRY_ATTEMPTS => {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);

            Ok(TransitionResult::new(ConvState::LlmRequesting {
                attempt: new_attempt,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::ScheduleRetry {
                delay,
                attempt: new_attempt,
            })
            .with_effect(Effect::notify_state_change(
                "llm_requesting",
                json!({
                    "attempt": new_attempt,
                    "max_attempts": MAX_RETRY_ATTEMPTS,
                    "message": format!("Retrying... (attempt {new_attempt})")
                }),
            )))
        }

        // Sub-agent: LlmRequesting + LlmError (exhausted or non-retryable) -> Failed + NotifyParent
        (
            ConvState::LlmRequesting { attempt },
            Event::LlmError {
                message,
                error_kind,
                ..
            },
        ) if context.is_sub_agent => {
            use crate::state_machine::state::SubAgentOutcome;
            let error_message = if error_kind.is_retryable() {
                format!("Failed after {attempt} attempts: {message}")
            } else {
                message
            };
            Ok(TransitionResult::new(ConvState::Failed {
                error: error_message.clone(),
                error_kind: error_kind.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error: error_message,
                    error_kind,
                },
            }))
        }

        // LlmRequesting + LlmError (non-retryable or exhausted) -> Error
        (
            ConvState::LlmRequesting { attempt },
            Event::LlmError {
                message,
                error_kind,
                ..
            },
        ) => {
            let error_message = if error_kind.is_retryable() {
                format!("Failed after {attempt} attempts: {message}")
            } else {
                message
            };

            Ok(TransitionResult::new(ConvState::Error {
                message: error_message.clone(),
                error_kind,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change(
                "error",
                json!({
                    "message": error_message
                }),
            )))
        }

        // RetryTimeout -> Make LLM request
        (
            ConvState::LlmRequesting { attempt },
            Event::RetryTimeout {
                attempt: retry_attempt,
            },
        ) if *attempt == retry_attempt => {
            Ok(
                TransitionResult::new(ConvState::LlmRequesting { attempt: *attempt })
                    .with_effect(Effect::RequestLlm),
            )
        }

        // ============================================================
        // Tool Execution (REQ-BED-004)
        // ============================================================

        // ToolExecuting + ToolComplete (more tools remaining) -> ToolExecuting (next tool)
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            Event::ToolComplete {
                tool_use_id,
                result,
            },
        ) if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_results = completed_results.clone();
            new_results.push(result);
            let completed_count = new_results.len();

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();
            let remaining_count = new_remaining.len();

            Ok(TransitionResult::new(ConvState::ToolExecuting {
                current_tool: next_tool.clone(),
                remaining_tools: new_remaining,
                completed_results: new_results,
                pending_sub_agents: pending_sub_agents.clone(),
                assistant_message: assistant_message.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(notify_tool_executing(
                next_tool.name(),
                &next_tool.id,
                remaining_count,
                completed_count,
            ))
            .with_effect(Effect::execute_tool(next_tool)))
        }

        // ToolExecuting + ToolComplete (last tool, no sub-agents) -> LlmRequesting
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            Event::ToolComplete {
                tool_use_id,
                result,
            },
        ) if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && pending_sub_agents.is_empty() =>
        {
            let mut all_results = completed_results.clone();
            all_results.push(result);

            // Atomic persistence: assistant message + all tool results written together
            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in last-tool transition");

            Ok(
                TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(notify_llm_requesting(1))
                    .with_effect(Effect::RequestLlm),
            )
        }

        // ToolExecuting + ToolComplete (last tool, has sub-agents) -> AwaitingSubAgents
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            Event::ToolComplete {
                tool_use_id,
                result,
            },
        ) if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && !pending_sub_agents.is_empty() =>
        {
            let mut all_results = completed_results.clone();
            all_results.push(result);

            // Atomic persistence: assistant message + all tool results written together
            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect(
                    "tool_use/tool_result count mismatch in last-tool-with-subagents transition",
                );

            Ok(TransitionResult::new(ConvState::AwaitingSubAgents {
                pending: pending_sub_agents.clone(),
                completed_results: vec![],
                spawn_tool_id: None, // spawn_agents was earlier in the batch, tool_use_id lost
            })
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(notify_awaiting_sub_agents(pending_sub_agents, &[])))
        }

        // ToolExecuting + SpawnAgentsComplete (more tools) -> ToolExecuting (accumulate agents)
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            Event::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            },
        ) if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_results = completed_results.clone();
            new_results.push(result);
            let completed_count = new_results.len();

            let mut new_pending = pending_sub_agents.clone();
            new_pending.extend(spawned);

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();
            let remaining_count = new_remaining.len();

            Ok(TransitionResult::new(ConvState::ToolExecuting {
                current_tool: next_tool.clone(),
                remaining_tools: new_remaining,
                completed_results: new_results,
                pending_sub_agents: new_pending,
                assistant_message: assistant_message.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(notify_tool_executing(
                next_tool.name(),
                &next_tool.id,
                remaining_count,
                completed_count,
            ))
            .with_effect(Effect::execute_tool(next_tool)))
        }

        // ToolExecuting + SpawnAgentsComplete (last tool) -> AwaitingSubAgents
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            Event::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            },
        ) if tool_use_id == current_tool.id && remaining_tools.is_empty() => {
            let mut all_pending = pending_sub_agents.clone();
            all_pending.extend(spawned);

            let mut all_results = completed_results.clone();
            let spawn_id = result.tool_use_id.clone();
            all_results.push(result);

            // Atomic persistence: assistant message + all tool results written together
            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in spawn-agents-last transition");

            Ok(TransitionResult::new(ConvState::AwaitingSubAgents {
                pending: all_pending.clone(),
                completed_results: vec![],
                spawn_tool_id: Some(spawn_id),
            })
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(notify_awaiting_sub_agents(&all_pending, &[])))
        }

        // ============================================================
        // Cancellation (REQ-BED-005)
        // ============================================================

        // LlmRequesting + UserCancel -> Idle (fire-and-forget abort)
        (ConvState::LlmRequesting { .. }, Event::UserCancel { .. }) if !context.is_sub_agent => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::AbortLlm)
                .with_effect(Effect::notify_agent_done()))
        }

        // AwaitingSubAgents + UserCancel -> CancellingSubAgents
        (
            ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                ..
            },
            Event::UserCancel { .. },
        ) => {
            let ids: Vec<String> = pending.iter().map(|p| p.agent_id.clone()).collect();
            Ok(TransitionResult::new(ConvState::CancellingSubAgents {
                pending: pending.clone(),
                completed_results: completed_results.clone(),
            })
            .with_effect(Effect::CancelSubAgents { ids })
            .with_effect(Effect::PersistState))
        }

        // ToolExecuting + UserCancel -> CancellingTool (parent) or Failed (sub-agent)
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            Event::UserCancel { .. },
        ) if !context.is_sub_agent => {
            let mut result = TransitionResult::new(ConvState::CancellingTool {
                tool_use_id: current_tool.id.clone(),
                skipped_tools: remaining_tools.clone(),
                completed_results: completed_results.clone(),
                assistant_message: assistant_message.clone(),
                pending_sub_agents: pending_sub_agents.clone(),
            })
            .with_effect(Effect::AbortTool {
                tool_use_id: current_tool.id.clone(),
            })
            .with_effect(Effect::PersistState);

            // Also cancel any already-spawned sub-agents
            if !pending_sub_agents.is_empty() {
                let ids: Vec<String> = pending_sub_agents
                    .iter()
                    .map(|p| p.agent_id.clone())
                    .collect();
                result = result.with_effect(Effect::CancelSubAgents { ids });
            }

            Ok(result)
        }

        // CancellingTool + ToolAborted -> Idle or CancellingSubAgents
        (
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            Event::ToolAborted {
                tool_use_id: aborted_id,
            },
        ) if *tool_use_id == aborted_id => {
            // Build all results: previously completed + aborted + skipped
            let mut all_results = completed_results.clone();
            all_results.push(ToolResult::cancelled(
                tool_use_id.clone(),
                "Cancelled by user",
            ));
            for tool in skipped_tools {
                all_results.push(ToolResult::cancelled(
                    tool.id.clone(),
                    "Skipped due to cancellation",
                ));
            }

            // Atomic persistence: assistant message + all tool results
            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in cancellation transition");

            if pending_sub_agents.is_empty() {
                // No sub-agents to wait for -> go directly to Idle
                Ok(TransitionResult::new(ConvState::Idle)
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                // Phase 2: wait for cancelled sub-agents to report back
                Ok(TransitionResult::new(ConvState::CancellingSubAgents {
                    pending: pending_sub_agents.clone(),
                    completed_results: vec![],
                })
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState))
            }
        }

        // CancellingTool + ToolComplete -> Idle or CancellingSubAgents
        // (tool finished before abort, use synthetic anyway)
        (
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            Event::ToolComplete {
                tool_use_id: completed_id,
                result: _, // Discard actual result, use synthetic
            },
        ) if *tool_use_id == completed_id => {
            // Tool finished before we could abort it - still use synthetic result.
            let mut all_results = completed_results.clone();
            all_results.push(ToolResult::cancelled(
                tool_use_id.clone(),
                "Cancelled by user",
            ));
            for tool in skipped_tools {
                all_results.push(ToolResult::cancelled(
                    tool.id.clone(),
                    "Skipped due to cancellation",
                ));
            }

            // Atomic persistence: assistant message + all tool results
            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in cancellation-complete transition");

            if pending_sub_agents.is_empty() {
                Ok(TransitionResult::new(ConvState::Idle)
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                // Phase 2: wait for cancelled sub-agents to report back
                Ok(TransitionResult::new(ConvState::CancellingSubAgents {
                    pending: pending_sub_agents.clone(),
                    completed_results: vec![],
                })
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState))
            }
        }

        // CancellingTool + SubAgentResult -> CancellingTool (absorb early sub-agent results)
        // Sub-agents may report back before the tool abort completes.
        (
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            Event::SubAgentResult { agent_id, .. },
        ) if pending_sub_agents.iter().any(|p| p.agent_id == agent_id) => {
            let new_pending: Vec<_> = pending_sub_agents
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            Ok(TransitionResult::new(ConvState::CancellingTool {
                tool_use_id: tool_use_id.clone(),
                skipped_tools: skipped_tools.clone(),
                completed_results: completed_results.clone(),
                assistant_message: assistant_message.clone(),
                pending_sub_agents: new_pending,
            })
            .with_effect(Effect::PersistState))
        }

        // ============================================================
        // Sub-Agent Results (REQ-BED-008)
        // ============================================================

        // AwaitingSubAgents + SubAgentResult (more pending) -> AwaitingSubAgents
        (
            ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            Event::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() > 1 => {
            // Find the completed agent's task and remove it from pending
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let new_pending: Vec<_> = pending
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome,
            });

            // Build notification before moving values into state
            let notify = notify_awaiting_sub_agents(&new_pending, &new_results);

            Ok(TransitionResult::new(ConvState::AwaitingSubAgents {
                pending: new_pending,
                completed_results: new_results,
                spawn_tool_id: spawn_tool_id.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(notify))
        }

        // AwaitingSubAgents + SubAgentResult (last one) -> LlmRequesting
        (
            ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            Event::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() == 1 => {
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome,
            });

            Ok(
                TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistSubAgentResults {
                        results: new_results,
                        spawn_tool_id: spawn_tool_id.clone(),
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(notify_llm_requesting(1))
                    .with_effect(Effect::RequestLlm),
            )
        }

        // CancellingSubAgents + SubAgentResult (more pending) -> CancellingSubAgents
        (
            ConvState::CancellingSubAgents {
                pending,
                completed_results,
            },
            Event::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() > 1 => {
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let new_pending: Vec<_> = pending
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome,
            });

            Ok(TransitionResult::new(ConvState::CancellingSubAgents {
                pending: new_pending,
                completed_results: new_results,
            })
            .with_effect(Effect::PersistState))
        }

        // CancellingSubAgents + SubAgentResult (last one) -> Idle
        (
            ConvState::CancellingSubAgents { pending, .. },
            Event::SubAgentResult { agent_id, .. },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() == 1 => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // ============================================================
        // Sub-Agent Cancellation (wildcard for non-terminal states)
        // ============================================================
        (state, Event::UserCancel { reason }) if context.is_sub_agent && !state.is_terminal() => {
            use crate::state_machine::state::SubAgentOutcome;
            let error = reason
                .clone()
                .unwrap_or_else(|| "Cancelled by parent".to_string());
            Ok(TransitionResult::new(ConvState::Failed {
                error: error.clone(),
                error_kind: ErrorKind::Cancelled,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error,
                    error_kind: ErrorKind::Cancelled,
                },
            }))
        }

        // ============================================================
        // Task Approval (REQ-BED-028)
        // ============================================================

        // AwaitingTaskApproval + TaskApprovalResponse(Approved) -> LlmRequesting
        // After approval, the agent automatically begins executing the plan.
        // The system message about the branch + worktree is emitted by the
        // executor after git operations succeed, giving the agent context.
        (
            ConvState::AwaitingTaskApproval {
                title,
                priority,
                plan,
            },
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Approved,
            },
        ) => Ok(
            TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::ApproveTask {
                    title: title.clone(),
                    priority: priority.clone(),
                    plan: plan.clone(),
                })
                // System message with branch name is emitted by the executor after
                // git operations succeed (includes "You are on branch ...").
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        ),

        // AwaitingTaskApproval + TaskApprovalResponse(FeedbackProvided) -> LlmRequesting
        // The agent gets a new turn to revise the plan based on user feedback.
        (
            ConvState::AwaitingTaskApproval { .. },
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::FeedbackProvided { annotations },
            },
        ) => Ok(
            TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::PersistMessage {
                    content: crate::db::MessageContent::system(
                        "Plan not approved. The user provided feedback below. \
                         You must call propose_task again with a revised plan \
                         that addresses their feedback.",
                    ),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                })
                .with_effect(Effect::PersistMessage {
                    content: crate::db::MessageContent::user(annotations),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        ),

        // AwaitingTaskApproval + TaskApprovalResponse(Rejected) -> Idle (Explore)
        (
            ConvState::AwaitingTaskApproval { .. },
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Rejected,
            },
        ) => Ok(TransitionResult::new(ConvState::Idle)
            .with_effect(Effect::PersistMessage {
                content: crate::db::MessageContent::system("Task rejected."),
                display_data: None,
                usage_data: None,
                message_id: uuid::Uuid::new_v4().to_string(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_agent_done())),

        // AwaitingTaskApproval + UserCancel -> treat as Rejected
        (ConvState::AwaitingTaskApproval { .. }, Event::UserCancel { .. }) => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistMessage {
                    content: crate::db::MessageContent::system("Task rejected."),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // ============================================================
        // Ask User Question (REQ-AUQ-001)
        // ============================================================

        // AwaitingUserResponse + UserQuestionResponse -> LlmRequesting
        //
        // The tool result was already persisted in the checkpoint when the
        // state transitioned to AwaitingUserResponse ("Questions submitted
        // for user review"). The user's answers are delivered as a user
        // message so the LLM sees them on the next turn without creating
        // a duplicate tool_result for the same tool_use_id.
        (
            ConvState::AwaitingUserResponse { questions, .. },
            Event::UserQuestionResponse {
                answers,
                annotations,
            },
        ) => {
            // Format answers in question order (not HashMap iteration order)
            // to produce deterministic, readable output for the LLM.
            let answers_text = questions
                .iter()
                .filter_map(|q| {
                    let a = answers.get(&q.question)?;
                    let q = &q.question;
                    let mut parts = vec![format!("\"{}\" = \"{}\"", q, a)];
                    // Derive preview from the server-side question state
                    // (not from client-supplied annotation, which would be
                    // a parallel representation of the same data).
                    let question_data = questions.iter().find(|qq| qq.question == *q);
                    if let Some(qd) = question_data {
                        let selected_preview = qd
                            .options
                            .iter()
                            .find(|o| o.label == *a)
                            .and_then(|o| o.preview.as_deref());
                        if let Some(preview) = selected_preview {
                            parts.push(format!("selected preview:\n{preview}"));
                        }
                    }
                    // Notes are user-supplied (not duplicated from server state)
                    if let Some(ref anns) = annotations {
                        if let Some(ann) = anns.get(q.as_str()) {
                            if let Some(ref notes) = ann.notes {
                                parts.push(format!("user notes: {notes}"));
                            }
                        }
                    }
                    Some(parts.join(" "))
                })
                .collect::<Vec<_>>()
                .join("\n");

            let user_text = format!("Here are my answers:\n{answers_text}");

            Ok(
                TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistMessage {
                        content: crate::db::MessageContent::user(user_text),
                        display_data: None,
                        usage_data: None,
                        message_id: uuid::Uuid::new_v4().to_string(),
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(notify_llm_requesting(1))
                    .with_effect(Effect::RequestLlm),
            )
        }

        // AwaitingUserResponse + UserCancel -> Idle
        // Tool result already persisted in checkpoint. System message indicates decline.
        (ConvState::AwaitingUserResponse { .. }, Event::UserCancel { .. }) => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistMessage {
                    content: crate::db::MessageContent::system(
                        "User declined to answer questions.",
                    ),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // ============================================================
        // Context Continuation (REQ-BED-019 through REQ-BED-024)
        // ============================================================

        // ContinuationResponse -> ContextExhausted
        (ConvState::AwaitingContinuation { .. }, Event::ContinuationResponse { summary }) => {
            Ok(TransitionResult::new(ConvState::ContextExhausted {
                summary: summary.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&summary))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary }))
        }

        // ContinuationFailed -> ContextExhausted with fallback summary
        (ConvState::AwaitingContinuation { .. }, Event::ContinuationFailed { error }) => {
            let fallback = format!(
                "Context limit reached. The continuation summary could not be generated: {error}. \
                Please start a new conversation."
            );
            Ok(TransitionResult::new(ConvState::ContextExhausted {
                summary: fallback.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&fallback))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
        }

        // UserCancel during continuation -> ContextExhausted with cancelled message
        (ConvState::AwaitingContinuation { .. }, Event::UserCancel { .. }) => {
            let cancelled =
                "Continuation cancelled by user. Please start a new conversation.".to_string();
            Ok(TransitionResult::new(ConvState::ContextExhausted {
                summary: cancelled.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&cancelled))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::AbortLlm)
            .with_effect(Effect::NotifyContextExhausted { summary: cancelled }))
        }

        // LlmError during continuation - retry or fail
        (
            ConvState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
            Event::LlmError {
                message: _,
                error_kind,
                ..
            },
        ) if error_kind.is_retryable() && *attempt < MAX_RETRY_ATTEMPTS => {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);

            Ok(TransitionResult::new(ConvState::AwaitingContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
                attempt: new_attempt,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::ScheduleRetry {
                delay,
                attempt: new_attempt,
            })
            .with_effect(Effect::notify_state_change(
                "awaiting_continuation",
                json!({
                    "attempt": new_attempt,
                    "max_attempts": MAX_RETRY_ATTEMPTS,
                    "message": format!("Retrying continuation... (attempt {new_attempt})")
                }),
            )))
        }

        // LlmError during continuation - retries exhausted
        (ConvState::AwaitingContinuation { .. }, Event::LlmError { message, .. }) => {
            let fallback = format!(
                "Context limit reached. The continuation summary could not be generated: {message}. \
                Please start a new conversation."
            );
            Ok(TransitionResult::new(ConvState::ContextExhausted {
                summary: fallback.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&fallback))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
        }

        // RetryTimeout during continuation - retry the request
        (
            ConvState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
            Event::RetryTimeout {
                attempt: timeout_attempt,
            },
        ) if *attempt == timeout_attempt => {
            Ok(TransitionResult::new(ConvState::AwaitingContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
                attempt: *attempt,
            })
            .with_effect(Effect::RequestContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
            }))
        }

        // ContextExhausted rejects user messages (REQ-BED-021)
        (ConvState::ContextExhausted { .. }, Event::UserMessage { .. }) => {
            Err(TransitionError::ContextExhausted)
        }

        // ContextExhausted is terminal - ignore other events
        (state @ ConvState::ContextExhausted { .. }, _event) => {
            // Log but don't error - terminal states ignore events
            Ok(TransitionResult::new(state.clone()))
        }

        // Task resolution: Idle + TaskResolved -> Terminal (REQ-BED-029)
        // Git operations are completed by the API handler before this event is sent.
        // The state machine enforces the precondition (must be Idle).
        (
            ConvState::Idle,
            Event::TaskResolved {
                system_message,
                repo_root,
            },
        ) => Ok(
            TransitionResult::new(ConvState::Terminal).with_effect(Effect::ResolveTask {
                system_message,
                repo_root,
            }),
        ),

        // Terminal rejects ALL events (REQ-BED-029)
        (ConvState::Terminal, Event::UserMessage { .. }) => {
            Err(TransitionError::ConversationTerminal)
        }
        (ConvState::Terminal, _event) => {
            // Non-user events are silently absorbed (no error, no state change)
            Ok(TransitionResult::new(ConvState::Terminal))
        }

        // UserTriggerContinuation from Idle (REQ-BED-023)
        (ConvState::Idle, Event::UserTriggerContinuation) => {
            Ok(TransitionResult::new(ConvState::AwaitingContinuation {
                rejected_tool_calls: vec![],
                attempt: 1,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change(
                "awaiting_continuation",
                json!({ "manual_trigger": true }),
            ))
            .with_effect(Effect::RequestContinuation {
                rejected_tool_calls: vec![],
            }))
        }

        // ============================================================
        // Stale abort events (race between task abort and event delivery)
        // ============================================================
        (ConvState::Idle, Event::LlmResponse { .. }) => Ok(TransitionResult::new(ConvState::Idle)),

        // ============================================================
        // Stale Events (absorb silently)
        // ============================================================

        // TaskApprovalResponse arriving after the state has already moved on
        // (e.g., double-click approve, SSE reconnect resend). No-op.
        (_state, Event::TaskApprovalResponse { .. }) => {
            tracing::debug!("Absorbing stale TaskApprovalResponse");
            Ok(TransitionResult::new(_state.clone()))
        }

        // ============================================================
        // Invalid Transitions
        // ============================================================
        (state, event) => Err(TransitionError::InvalidTransition(format!(
            "No transition from {state:?} with event {event:?}"
        ))),
    }
}

// ============================================================================
// handle_outcome — second pure entry point for executor-produced outcomes
// ============================================================================

/// Entry point 2: Executor outcomes (from background tasks via typed channels).
///
/// This is the second layer of defense. Even with typed channels constraining
/// what CAN arrive, this function rejects outcomes that are invalid for the
/// current state. The executor logs and discards `Err` — state unchanged.
///
/// REQ-BED-001: Pure function — given the same inputs, always the same outputs.
pub fn handle_outcome(
    state: &ConvState,
    context: &ConvContext,
    outcome: EffectOutcome,
) -> Result<TransitionResult, InvalidOutcome> {
    let event = match outcome {
        EffectOutcome::Llm(llm) => llm_outcome_to_event(llm, state),
        EffectOutcome::Tool(tool) => tool_outcome_to_event(tool),
        EffectOutcome::SubAgent { agent_id, outcome } => {
            Event::SubAgentResult { agent_id, outcome }
        }
        EffectOutcome::Persist(persist) => {
            return handle_persist_outcome(state, persist);
        }
        EffectOutcome::RetryTimeout { attempt } => Event::RetryTimeout { attempt },
    };

    transition(state, context, event).map_err(|e| InvalidOutcome {
        reason: e.to_string(),
    })
}

/// Convert `LlmOutcome` to the equivalent `Event` for delegation to `transition()`.
fn llm_outcome_to_event(outcome: LlmOutcome, state: &ConvState) -> Event {
    match outcome {
        LlmOutcome::Response {
            content,
            tool_calls,
            end_turn,
            usage,
        } => Event::LlmResponse {
            content,
            tool_calls,
            end_turn,
            usage,
        },
        LlmOutcome::RateLimited { retry_after: _ } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Rate limited".to_string(),
                error_kind: ErrorKind::RateLimit,
                attempt,
            }
        }
        LlmOutcome::ServerError { status, body } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: format!("Server error {status}: {body}"),
                error_kind: ErrorKind::ServerError,
                attempt,
            }
        }
        LlmOutcome::NetworkError { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::Network,
                attempt,
            }
        }
        LlmOutcome::TokenBudgetExceeded => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Token budget exceeded".to_string(),
                error_kind: ErrorKind::ContextExhausted,
                attempt,
            }
        }
        LlmOutcome::AuthError { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::Auth,
                attempt,
            }
        }
        LlmOutcome::RequestRejected { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::InvalidRequest,
                attempt,
            }
        }
        LlmOutcome::Cancelled => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Request cancelled".to_string(),
                error_kind: ErrorKind::Cancelled,
                attempt,
            }
        }
    }
}

/// Convert `ToolOutcome` to the equivalent `Event` for delegation to `transition()`.
fn tool_outcome_to_event(outcome: ToolOutcome) -> Event {
    match outcome {
        ToolOutcome::Completed(result) => Event::ToolComplete {
            tool_use_id: result.tool_use_id.clone(),
            result,
        },
        ToolOutcome::Aborted {
            tool_use_id,
            reason: _,
        } => Event::ToolAborted { tool_use_id },
        ToolOutcome::Failed { tool_use_id, error } => Event::ToolComplete {
            tool_use_id: tool_use_id.clone(),
            result: ToolResult::error(tool_use_id, error),
        },
    }
}

/// Handle `PersistOutcome` directly — no Event equivalent exists.
/// Persistence failures are logged but don't change state.
fn handle_persist_outcome(
    state: &ConvState,
    outcome: PersistOutcome,
) -> Result<TransitionResult, InvalidOutcome> {
    match outcome {
        PersistOutcome::Ok => Ok(TransitionResult::new(state.clone())),
        PersistOutcome::Failed { error } => Err(InvalidOutcome {
            reason: format!("Persistence failed: {error}"),
        }),
    }
}

/// Extract the current attempt number from state (for LLM error conversion).
fn current_attempt(state: &ConvState) -> u32 {
    match state {
        ConvState::LlmRequesting { attempt } | ConvState::AwaitingContinuation { attempt, .. } => {
            *attempt
        }
        _ => 1,
    }
}

// Helper functions

impl ToolResult {
    #[allow(dead_code)] // Used in tests; normal tool rounds use PersistCheckpoint
    fn display_data(&self) -> Option<Value> {
        self.display_data.clone()
    }
}

/// Threshold as fraction of context window for triggering continuation (REQ-BED-019)
const CONTINUATION_THRESHOLD: f64 = 0.90;

/// Check if context usage has exceeded the continuation threshold (REQ-BED-019)
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn should_trigger_continuation(usage: &UsageData, context_window: usize) -> bool {
    let used = usage.context_window_used();
    let threshold = (context_window as f64 * CONTINUATION_THRESHOLD) as u64;
    used >= threshold
}

/// Handle context exhaustion based on conversation type (REQ-BED-019, REQ-BED-024)
fn handle_context_exhaustion(
    ctx: &ConvContext,
    blocks: Vec<crate::llm::ContentBlock>,
    tool_calls: Vec<ToolCall>,
    usage_data: UsageData,
) -> TransitionResult {
    use crate::state_machine::state::SubAgentOutcome;

    match ctx.context_exhaustion_behavior {
        ContextExhaustionBehavior::ThresholdBasedContinuation => {
            // Normal conversation: trigger continuation flow
            TransitionResult::new(ConvState::AwaitingContinuation {
                rejected_tool_calls: tool_calls.clone(),
                attempt: 1,
            })
            .with_effect(Effect::persist_agent_message(
                blocks,
                Some(usage_data),
                &ctx.working_dir,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change(
                "awaiting_continuation",
                json!({
                    "rejected_tools": tool_calls.iter().map(ToolCall::name).collect::<Vec<_>>()
                }),
            ))
            .with_effect(Effect::RequestContinuation {
                rejected_tool_calls: tool_calls,
            })
        }
        ContextExhaustionBehavior::IntentionallyUnhandled => {
            // REQ-BED-024: Sub-agent fails immediately
            TransitionResult::new(ConvState::Failed {
                error: "Context window exhausted before result submission".to_string(),
                error_kind: ErrorKind::ContextExhausted,
            })
            .with_effect(Effect::persist_agent_message(
                blocks,
                Some(usage_data),
                &ctx.working_dir,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error: "Context window exhausted before result submission".to_string(),
                    error_kind: ErrorKind::ContextExhausted,
                },
            })
        }
    }
}

/// Extract concatenated text from content blocks for implicit sub-agent completion.
fn extract_text_from_content(blocks: &[crate::llm::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            crate::llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn retry_delay(attempt: u32) -> Duration {
    // Exponential backoff: 1s, 2s, 4s
    Duration::from_secs(1 << (attempt - 1))
}

/// Helper to create `state_change` notification for `LlmRequesting`
fn notify_llm_requesting(attempt: u32) -> Effect {
    Effect::notify_state_change(
        "llm_requesting",
        json!({
            "attempt": attempt
        }),
    )
}

/// Helper to create `state_change` notification for `ToolExecuting`
fn notify_tool_executing(
    tool_name: &str,
    tool_id: &str,
    remaining_count: usize,
    completed_count: usize,
) -> Effect {
    Effect::notify_state_change(
        "tool_executing",
        json!({
            "current_tool": {
                "name": tool_name,
                "id": tool_id
            },
            "remaining_count": remaining_count,
            "completed_count": completed_count
        }),
    )
}

/// Helper to create `state_change` notification for `AwaitingSubAgents`
fn notify_awaiting_sub_agents(pending: &[PendingSubAgent], completed: &[SubAgentResult]) -> Effect {
    Effect::notify_state_change(
        "awaiting_sub_agents",
        json!({
            "pending": pending,
            "completed_results": completed
        }),
    )
}

#[allow(dead_code)] // Conversion utility
pub fn llm_error_to_db_error(kind: crate::llm::LlmErrorKind) -> ErrorKind {
    // Explicit match arms — no catch-all. The compiler enforces exhaustiveness.
    match kind {
        crate::llm::LlmErrorKind::Auth => ErrorKind::Auth,
        crate::llm::LlmErrorKind::RateLimit => ErrorKind::RateLimit,
        crate::llm::LlmErrorKind::Network => ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => ErrorKind::InvalidRequest,
        crate::llm::LlmErrorKind::ServerError => ErrorKind::ServerError,
        crate::llm::LlmErrorKind::ContentFilter => ErrorKind::ContentFilter,
        crate::llm::LlmErrorKind::ContextWindowExceeded => ErrorKind::ContextExhausted,
    }
}

// ErrorKind::is_retryable() is now defined in db/schema.rs

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_context() -> ConvContext {
        ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000)
    }

    #[test]
    fn test_idle_to_llm_requesting() {
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::UserMessage {
                text: "Hello".to_string(),
                llm_text: None,
                images: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::LlmRequesting { attempt: 1 }
        ));
        assert!(!result.effects.is_empty());
    }

    #[test]
    fn test_reject_message_while_busy() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::UserMessage {
                text: "Hello".to_string(),
                llm_text: None,
                images: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        );

        assert!(matches!(result, Err(TransitionError::AgentBusy)));
    }

    #[test]
    fn test_error_recovery() {
        let result = transition(
            &ConvState::Error {
                message: "Previous error".to_string(),
                error_kind: ErrorKind::Network,
            },
            &test_context(),
            Event::UserMessage {
                text: "Try again".to_string(),
                llm_text: None,
                images: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::LlmRequesting { attempt: 1 }
        ));
    }

    #[test]
    fn test_cancellation_produces_synthetic_results() {
        use crate::llm::ContentBlock;
        use crate::state_machine::state::{
            AssistantMessage, BashInput, BashMode, ToolCall, ToolInput,
        };

        // Build an AssistantMessage with 3 tool_use blocks matching the 3 tools
        let assistant_message = AssistantMessage::new(
            vec![
                ContentBlock::tool_use("tool-1", "bash", serde_json::json!({"command": "echo 1"})),
                ContentBlock::tool_use("tool-2", "bash", serde_json::json!({"command": "echo 2"})),
                ContentBlock::tool_use("tool-3", "bash", serde_json::json!({"command": "echo 3"})),
            ],
            None,
            None,
        );

        let result = transition(
            &ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    "tool-1",
                    ToolInput::Bash(BashInput {
                        command: "echo 1".to_string(),
                        mode: BashMode::Default,
                    }),
                ),
                remaining_tools: vec![
                    ToolCall::new(
                        "tool-2",
                        ToolInput::Bash(BashInput {
                            command: "echo 2".to_string(),
                            mode: BashMode::Default,
                        }),
                    ),
                    ToolCall::new(
                        "tool-3",
                        ToolInput::Bash(BashInput {
                            command: "echo 3".to_string(),
                            mode: BashMode::Default,
                        }),
                    ),
                ],
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message,
            },
            &test_context(),
            Event::UserCancel { reason: None },
        )
        .unwrap();

        // Phase 1: Should go to CancellingTool with AbortTool effect
        assert!(
            matches!(result.new_state, ConvState::CancellingTool { .. }),
            "Should transition to CancellingTool"
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
            &test_context(),
            Event::ToolAborted {
                tool_use_id: "tool-1".to_string(),
            },
        )
        .unwrap();

        assert!(matches!(result2.new_state, ConvState::Idle));
        assert!(
            result2
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
            "Should have PersistCheckpoint effect instead of PersistToolResults"
        );
    }

    // ========================================================================
    // Context Exhaustion Tests (REQ-BED-019 through REQ-BED-024)
    // ========================================================================

    #[test]
    fn test_threshold_boundary_below() {
        // 89.9% should NOT trigger continuation
        let usage = UsageData {
            input_tokens: 89_900,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            !should_trigger_continuation(&usage, 100_000),
            "89.9% should not trigger continuation"
        );
    }

    #[test]
    fn test_threshold_boundary_at() {
        // Exactly 90% SHOULD trigger continuation
        let usage = UsageData {
            input_tokens: 90_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            should_trigger_continuation(&usage, 100_000),
            "90% should trigger continuation"
        );
    }

    #[test]
    fn test_threshold_boundary_above() {
        // 90.1% should trigger continuation
        let usage = UsageData {
            input_tokens: 90_100,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            should_trigger_continuation(&usage, 100_000),
            "90.1% should trigger continuation"
        );
    }

    #[test]
    fn test_threshold_with_output_tokens() {
        // 45k input + 45k output = 90k total >= 90% of 100k
        let usage = UsageData {
            input_tokens: 45_000,
            output_tokens: 45_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            should_trigger_continuation(&usage, 100_000),
            "Combined tokens should count toward threshold"
        );
    }

    #[test]
    fn test_subagent_context_exhaustion_fails_immediately() {
        use crate::llm::ContentBlock;
        use crate::state_machine::state::ContextExhaustionBehavior;

        // Create a sub-agent context
        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 100_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
        };

        let result = handle_context_exhaustion(
            &subagent_ctx,
            vec![ContentBlock::text("response")],
            vec![], // no tools
            UsageData {
                input_tokens: 95_000,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );

        // Sub-agent should go to Failed, not AwaitingContinuation
        assert!(
            matches!(
                result.new_state,
                ConvState::Failed {
                    error_kind: ErrorKind::ContextExhausted,
                    ..
                }
            ),
            "Sub-agent should fail immediately, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent of failure"
        );

        // Should NOT request continuation
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestContinuation { .. })),
            "Sub-agent should NOT request continuation"
        );
    }

    #[test]
    fn test_parent_context_exhaustion_triggers_continuation() {
        use crate::llm::ContentBlock;
        use crate::state_machine::state::{BashInput, BashMode, ToolCall, ToolInput};

        let parent_ctx = test_context(); // Uses ThresholdBasedContinuation

        let tool_calls = vec![ToolCall::new(
            "tool-1",
            ToolInput::Bash(BashInput {
                command: "echo test".to_string(),
                mode: BashMode::Default,
            }),
        )];

        let result = handle_context_exhaustion(
            &parent_ctx,
            vec![ContentBlock::text("response")],
            tool_calls.clone(),
            UsageData {
                input_tokens: 95_000,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );

        // Parent should go to AwaitingContinuation
        assert!(
            matches!(result.new_state, ConvState::AwaitingContinuation { .. }),
            "Parent should enter AwaitingContinuation, got {:?}",
            result.new_state
        );

        // Should request continuation with rejected tools
        assert!(
            result.effects.iter().any(|e| matches!(
                e,
                Effect::RequestContinuation { rejected_tool_calls } if rejected_tool_calls.len() == 1
            )),
            "Parent should request continuation with rejected tools"
        );

        // Should NOT notify parent (it's not a sub-agent)
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Parent conversation should NOT notify parent"
        );
    }

    #[test]
    fn test_subagent_text_only_response_is_implicit_completion() {
        use crate::llm::{ContentBlock, Usage};
        use crate::state_machine::state::ContextExhaustionBehavior;

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
        };

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &subagent_ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::text("Here is my analysis of the codebase.")],
                tool_calls: vec![], // No tools — LLM didn't call submit_result
                end_turn: true,
                usage: Usage {
                    input_tokens: 5000,
                    output_tokens: 500,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        )
        .unwrap();

        // Should go to Completed, NOT Idle
        assert!(
            matches!(result.new_state, ConvState::Completed { .. }),
            "Sub-agent text-only response should go to Completed, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent on implicit completion"
        );

        // Should NOT emit notify_agent_done (that's for parent conversations)
        assert!(
            !result.effects.iter().any(|e| matches!(
                e,
                Effect::NotifyClient { event_type, .. } if event_type == "agent_done"
            )),
            "Sub-agent should NOT emit agent_done SSE event"
        );
    }

    #[test]
    fn test_parent_text_only_response_still_goes_idle() {
        use crate::llm::{ContentBlock, Usage};

        let parent_ctx = test_context();

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &parent_ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::text("Here is my response.")],
                tool_calls: vec![],
                end_turn: true,
                usage: Usage {
                    input_tokens: 5000,
                    output_tokens: 500,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        )
        .unwrap();

        // Parent should still go to Idle
        assert!(
            matches!(result.new_state, ConvState::Idle),
            "Parent text-only response should go to Idle, got {:?}",
            result.new_state
        );

        // Should NOT notify parent (it IS the parent)
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Parent should NOT have NotifyParent effect"
        );
    }

    #[test]
    fn test_subagent_llm_retries_exhausted_notifies_parent() {
        use crate::state_machine::state::ContextExhaustionBehavior;

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
        };

        // attempt == MAX_RETRY_ATTEMPTS (3), retryable error → retries exhausted
        let result = transition(
            &ConvState::LlmRequesting { attempt: 3 },
            &subagent_ctx,
            Event::LlmError {
                message: "Request timeout".to_string(),
                error_kind: ErrorKind::Network, // retryable
                attempt: 3,
            },
        )
        .unwrap();

        // Sub-agent should go to Failed, NOT Error
        assert!(
            matches!(result.new_state, ConvState::Failed { .. }),
            "Sub-agent with exhausted retries should go to Failed, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent when LLM retries are exhausted"
        );
    }

    #[test]
    fn test_subagent_llm_non_retryable_error_notifies_parent() {
        use crate::state_machine::state::ContextExhaustionBehavior;

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
        };

        // Non-retryable error at attempt 1 → immediate failure
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &subagent_ctx,
            Event::LlmError {
                message: "Invalid API key".to_string(),
                error_kind: ErrorKind::Auth, // non-retryable
                attempt: 1,
            },
        )
        .unwrap();

        // Sub-agent should go to Failed, NOT Error
        assert!(
            matches!(result.new_state, ConvState::Failed { .. }),
            "Sub-agent with non-retryable error should go to Failed, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent on non-retryable LLM error"
        );
    }

    #[test]
    fn test_parent_llm_retries_exhausted_still_goes_to_error() {
        let parent_ctx = test_context();

        let result = transition(
            &ConvState::LlmRequesting { attempt: 3 },
            &parent_ctx,
            Event::LlmError {
                message: "Request timeout".to_string(),
                error_kind: ErrorKind::Network,
                attempt: 3,
            },
        )
        .unwrap();

        // Parent should still go to Error (user can retry)
        assert!(
            matches!(result.new_state, ConvState::Error { .. }),
            "Parent with exhausted retries should go to Error, got {:?}",
            result.new_state
        );

        // Should NOT notify parent
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Parent should NOT have NotifyParent effect"
        );
    }

    // ========================================================================
    // Ask User Question Tests (REQ-AUQ-001)
    // ========================================================================

    fn make_ask_user_question_tool_call(tool_id: &str) -> ToolCall {
        use crate::state_machine::state::{
            AskUserQuestionInput, QuestionOption, ToolInput, UserQuestion,
        };
        ToolCall::new(
            tool_id,
            ToolInput::AskUserQuestion(AskUserQuestionInput {
                questions: vec![UserQuestion {
                    question: "Which library?".to_string(),
                    header: "Dependencies".to_string(),
                    options: vec![
                        QuestionOption {
                            label: "lodash".to_string(),
                            description: None,
                            preview: None,
                        },
                        QuestionOption {
                            label: "ramda".to_string(),
                            description: None,
                            preview: None,
                        },
                    ],
                    multi_select: false,
                }],
                metadata: None,
            }),
        )
    }

    #[test]
    fn test_llm_response_with_ask_user_question_goes_to_awaiting() {
        use crate::llm::{ContentBlock, Usage};

        let ctx = test_context();
        let tool = make_ask_user_question_tool_call("tool-auq-1");

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![
                    ContentBlock::text("Let me ask you something"),
                    ContentBlock::ToolUse {
                        id: "tool-auq-1".to_string(),
                        name: "ask_user_question".to_string(),
                        input: serde_json::json!({}),
                    },
                ],
                tool_calls: vec![tool],
                end_turn: false,
                usage: Usage::default(),
            },
        )
        .unwrap();

        assert!(
            matches!(result.new_state, ConvState::AwaitingUserResponse { .. }),
            "Should go to AwaitingUserResponse, got {:?}",
            result.new_state
        );

        // Should have PersistCheckpoint + PersistState
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
            "Should have PersistCheckpoint effect"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistState)),
            "Should have PersistState effect"
        );
    }

    #[test]
    fn test_ask_user_question_must_be_only_tool() {
        use crate::llm::{ContentBlock, Usage};
        use crate::state_machine::state::{BashInput, BashMode, ToolInput};

        let ctx = test_context();
        let auq_tool = make_ask_user_question_tool_call("tool-auq-1");
        let bash_tool = ToolCall::new(
            "tool-bash-1",
            ToolInput::Bash(BashInput {
                command: "echo test".to_string(),
                mode: BashMode::Default,
            }),
        );

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool-auq-1".to_string(),
                        name: "ask_user_question".to_string(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-bash-1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo test"}),
                    },
                ],
                tool_calls: vec![auq_tool, bash_tool],
                end_turn: false,
                usage: Usage::default(),
            },
        );

        let result = result.expect("Should produce Ok transition to Error state");
        assert!(
            matches!(result.new_state, ConvState::Error { .. }),
            "Should transition to Error when ask_user_question mixed with other tools, got {:?}",
            result.new_state
        );
    }

    #[test]
    fn test_awaiting_user_response_with_answer_goes_to_llm_requesting() {
        use crate::state_machine::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let mut answers = std::collections::HashMap::new();
        answers.insert("Which library?".to_string(), "lodash".to_string());

        let result = transition(
            &state,
            &test_context(),
            Event::UserQuestionResponse {
                answers,
                annotations: None,
            },
        )
        .unwrap();

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "Should go to LlmRequesting, got {:?}",
            result.new_state
        );

        // Should have PersistMessage (user answers) + PersistState + RequestLlm
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistMessage { .. })),
            "Should have PersistMessage effect for user answers"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Should have RequestLlm effect"
        );
    }

    #[test]
    fn test_awaiting_user_response_cancel_goes_to_idle() {
        use crate::state_machine::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let result =
            transition(&state, &test_context(), Event::UserCancel { reason: None }).unwrap();

        assert!(
            matches!(result.new_state, ConvState::Idle),
            "Should go to Idle, got {:?}",
            result.new_state
        );

        // Should have PersistMessage (system: declined)
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistMessage { .. })),
            "Should have PersistMessage effect for decline"
        );

        // Should have agent_done notification
        assert!(
            result.effects.iter().any(|e| matches!(
                e,
                Effect::NotifyClient { event_type, .. } if event_type == "agent_done"
            )),
            "Should have agent_done notification"
        );
    }

    #[test]
    fn test_awaiting_user_response_rejects_user_message() {
        use crate::state_machine::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let result = transition(
            &state,
            &test_context(),
            Event::UserMessage {
                text: "hello".to_string(),
                llm_text: None,
                images: vec![],
                message_id: "msg-1".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        );

        assert!(
            matches!(result, Err(TransitionError::AwaitingUserResponse)),
            "Should reject user messages with AwaitingUserResponse error, got {:?}",
            result
        );
    }
}
