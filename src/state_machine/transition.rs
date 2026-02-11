//! Pure state transition function
//!
//! REQ-BED-001: Pure State Transitions
//! REQ-BED-002: User Message Handling
//! REQ-BED-003: LLM Response Processing
//! REQ-BED-004: Tool Execution Coordination
//! REQ-BED-005: Cancellation Handling
//! REQ-BED-006: Error Recovery

use super::state::{PendingSubAgent, SubAgentResult};
use super::{ConvContext, ConvState, Effect, Event};
use crate::db::{ErrorKind, ToolResult, UsageData};
use serde_json::{json, Value};
use std::collections::HashSet;
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
                images,
                message_id,
                user_agent,
            },
        ) => Ok(
            TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_user_message(
                    text, images, message_id, user_agent,
                ))
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        ),

        // Busy states + UserMessage -> Reject (REQ-BED-002)
        (ConvState::AwaitingLlm, Event::UserMessage { .. })
        | (ConvState::LlmRequesting { .. }, Event::UserMessage { .. })
        | (ConvState::ToolExecuting { .. }, Event::UserMessage { .. })
        | (ConvState::AwaitingSubAgents { .. }, Event::UserMessage { .. }) => {
            Err(TransitionError::AgentBusy)
        }

        (ConvState::CancellingLlm, Event::UserMessage { .. })
        | (ConvState::CancellingTool { .. }, Event::UserMessage { .. })
        | (ConvState::CancellingSubAgents { .. }, Event::UserMessage { .. }) => {
            Err(TransitionError::CancellationInProgress)
        }

        // ============================================================
        // LLM Response Processing (REQ-BED-003)
        // ============================================================

        // AwaitingLlm is an intermediate state - immediately transition to LlmRequesting
        // This is handled in the runtime, not here

        // LlmRequesting + LlmResponse with tools -> ToolExecuting (or terminal for sub-agents)
        (
            ConvState::LlmRequesting { .. },
            Event::LlmResponse {
                content,
                tool_calls,
                end_turn: _,
                usage,
            },
        ) => {
            let usage_data = usage_to_data(&usage);

            if tool_calls.is_empty() {
                // No tools, just text response -> Idle
                Ok(TransitionResult::new(ConvState::Idle)
                    .with_effect(Effect::persist_agent_message(content, Some(usage_data)))
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else if context.is_sub_agent {
                // Check for terminal tools (submit_result/submit_error)
                let terminal_tool = tool_calls.iter().find(|t| t.input.is_terminal_tool());

                if let Some(tool) = terminal_tool {
                    // Terminal tool must be the only tool
                    if tool_calls.len() > 1 {
                        return Err(TransitionError::InvalidTransition(
                            "submit_result/submit_error must be the only tool in response"
                                .to_string(),
                        ));
                    }

                    // Transition directly to terminal state
                    match &tool.input {
                        crate::state_machine::state::ToolInput::SubmitResult(input) => {
                            use crate::state_machine::state::SubAgentOutcome;
                            Ok(TransitionResult::new(ConvState::Completed {
                                result: input.result.clone(),
                            })
                            .with_effect(Effect::persist_agent_message(content, Some(usage_data)))
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
                            .with_effect(Effect::persist_agent_message(content, Some(usage_data)))
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

                    Ok(TransitionResult::new(ConvState::ToolExecuting {
                        current_tool: first.clone(),
                        remaining_tools: rest,
                        persisted_tool_ids: HashSet::new(),
                        pending_sub_agents: vec![],
                    })
                    .with_effect(Effect::persist_agent_message(content, Some(usage_data)))
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

                Ok(TransitionResult::new(ConvState::ToolExecuting {
                    current_tool: first.clone(),
                    remaining_tools: rest,
                    persisted_tool_ids: HashSet::new(),
                    pending_sub_agents: vec![],
                })
                .with_effect(Effect::persist_agent_message(content, Some(usage_data)))
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
                persisted_tool_ids,
                pending_sub_agents,
            },
            Event::ToolComplete {
                tool_use_id,
                result,
            },
        ) if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_persisted = persisted_tool_ids.clone();
            new_persisted.insert(result.tool_use_id.clone());
            let completed_count = new_persisted.len();

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();
            let remaining_count = new_remaining.len();

            Ok(TransitionResult::new(ConvState::ToolExecuting {
                current_tool: next_tool.clone(),
                remaining_tools: new_remaining,
                persisted_tool_ids: new_persisted,
                pending_sub_agents: pending_sub_agents.clone(),
            })
            .with_effect(Effect::persist_tool_message(
                &result.tool_use_id,
                &result.output,
                result.is_error,
                result.display_data(),
            ))
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
                pending_sub_agents,
                ..
            },
            Event::ToolComplete {
                tool_use_id,
                result,
            },
        ) if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && pending_sub_agents.is_empty() =>
        {
            Ok(
                TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::persist_tool_message(
                        &result.tool_use_id,
                        &result.output,
                        result.is_error,
                        result.display_data(),
                    ))
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
                pending_sub_agents,
                ..
            },
            Event::ToolComplete {
                tool_use_id,
                result,
            },
        ) if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && !pending_sub_agents.is_empty() =>
        {
            Ok(TransitionResult::new(ConvState::AwaitingSubAgents {
                pending: pending_sub_agents.clone(),
                completed_results: vec![],
                spawn_tool_id: None, // spawn_agents was earlier in the batch, tool_use_id lost
            })
            .with_effect(Effect::persist_tool_message(
                &result.tool_use_id,
                &result.output,
                result.is_error,
                result.display_data(),
            ))
            .with_effect(Effect::PersistState)
            .with_effect(notify_awaiting_sub_agents(pending_sub_agents, &[])))
        }

        // ToolExecuting + SpawnAgentsComplete (more tools) -> ToolExecuting (accumulate agents)
        (
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                persisted_tool_ids,
                pending_sub_agents,
            },
            Event::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            },
        ) if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_persisted = persisted_tool_ids.clone();
            new_persisted.insert(result.tool_use_id.clone());
            let completed_count = new_persisted.len();

            let mut new_pending = pending_sub_agents.clone();
            new_pending.extend(spawned);

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();
            let remaining_count = new_remaining.len();

            Ok(TransitionResult::new(ConvState::ToolExecuting {
                current_tool: next_tool.clone(),
                remaining_tools: new_remaining,
                persisted_tool_ids: new_persisted,
                pending_sub_agents: new_pending,
            })
            .with_effect(Effect::persist_tool_message(
                &result.tool_use_id,
                &result.output,
                result.is_error,
                result.display_data(),
            ))
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
                pending_sub_agents,
                ..
            },
            Event::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            },
        ) if tool_use_id == current_tool.id && remaining_tools.is_empty() => {
            let mut all_pending = pending_sub_agents.clone();
            all_pending.extend(spawned);

            // spawn_agents is the last tool, so we have its tool_use_id
            let spawn_id = result.tool_use_id.clone();
            Ok(TransitionResult::new(ConvState::AwaitingSubAgents {
                pending: all_pending.clone(),
                completed_results: vec![],
                spawn_tool_id: Some(spawn_id),
            })
            .with_effect(Effect::persist_tool_message(
                &result.tool_use_id,
                &result.output,
                result.is_error,
                result.display_data(),
            ))
            .with_effect(Effect::PersistState)
            .with_effect(notify_awaiting_sub_agents(&all_pending, &[])))
        }

        // ============================================================
        // Cancellation (REQ-BED-005)
        // ============================================================

        // LlmRequesting + UserCancel -> CancellingLlm (parent) or Failed (sub-agent)
        (ConvState::LlmRequesting { .. }, Event::UserCancel) if !context.is_sub_agent => {
            Ok(TransitionResult::new(ConvState::CancellingLlm)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::AbortLlm))
        }

        // CancellingLlm + LlmResponse/LlmAborted -> Idle (discard response)
        (ConvState::CancellingLlm, Event::LlmResponse { .. } | Event::LlmAborted) => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // AwaitingLlm + UserCancel -> Idle (parent) or Failed (sub-agent)
        (ConvState::AwaitingLlm, Event::UserCancel) if !context.is_sub_agent => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // AwaitingSubAgents + UserCancel -> CancellingSubAgents
        (
            ConvState::AwaitingSubAgents {
                pending,
                completed_results,
                ..
            },
            Event::UserCancel,
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
                persisted_tool_ids,
                pending_sub_agents,
            },
            Event::UserCancel,
        ) if !context.is_sub_agent => {
            let mut result = TransitionResult::new(ConvState::CancellingTool {
                tool_use_id: current_tool.id.clone(),
                skipped_tools: remaining_tools.clone(),
                persisted_tool_ids: persisted_tool_ids.clone(),
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

        // CancellingTool + ToolAborted -> Idle with synthetic results
        (
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                persisted_tool_ids,
            },
            Event::ToolAborted {
                tool_use_id: aborted_id,
            },
        ) if *tool_use_id == aborted_id => {
            // Generate synthetic results for aborted and skipped tools only.
            // Tools in persisted_tool_ids were already persisted via PersistMessage
            // when each tool completed, so we don't include them here.
            let aborted_result = ToolResult::cancelled(tool_use_id.clone(), "Cancelled by user");
            let skipped_results: Vec<ToolResult> = skipped_tools
                .iter()
                .map(|tool| ToolResult::cancelled(tool.id.clone(), "Skipped due to cancellation"))
                .collect();

            let mut new_results = vec![aborted_result];
            new_results.extend(skipped_results);

            // Validate: none of the new results should be for already-persisted tools
            validate_no_duplicate_persists(&new_results, persisted_tool_ids)?;

            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistToolResults {
                    results: new_results,
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // CancellingTool + ToolComplete -> Idle (tool finished before abort, use synthetic anyway)
        (
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                persisted_tool_ids,
            },
            Event::ToolComplete {
                tool_use_id: completed_id,
                result: _, // Discard actual result, use synthetic
            },
        ) if *tool_use_id == completed_id => {
            // Tool finished before we could abort it - still use synthetic result.
            // Tools in persisted_tool_ids were already persisted via PersistMessage
            // when each tool completed, so we don't include them here.
            let cancelled_result = ToolResult::cancelled(tool_use_id.clone(), "Cancelled by user");
            let skipped_results: Vec<ToolResult> = skipped_tools
                .iter()
                .map(|tool| ToolResult::cancelled(tool.id.clone(), "Skipped due to cancellation"))
                .collect();

            let mut new_results = vec![cancelled_result];
            new_results.extend(skipped_results);

            // Validate: none of the new results should be for already-persisted tools
            validate_no_duplicate_persists(&new_results, persisted_tool_ids)?;

            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistToolResults {
                    results: new_results,
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
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
        (state, Event::UserCancel) if context.is_sub_agent && !state.is_terminal() => {
            use crate::state_machine::state::SubAgentOutcome;
            Ok(TransitionResult::new(ConvState::Failed {
                error: "Cancelled by parent".to_string(),
                error_kind: ErrorKind::Cancelled,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error: "Cancelled by parent".to_string(),
                    error_kind: ErrorKind::Cancelled,
                },
            }))
        }

        // ============================================================
        // Invalid Transitions
        // ============================================================
        (state, event) => Err(TransitionError::InvalidTransition(format!(
            "No transition from {state:?} with event {event:?}"
        ))),
    }
}

// Helper functions

/// Validates that none of the tool results to be persisted have IDs that are already persisted.
/// This is a critical invariant: each `tool_use_id` must be persisted exactly once.
fn validate_no_duplicate_persists(
    results: &[ToolResult],
    already_persisted: &HashSet<String>,
) -> Result<(), TransitionError> {
    for result in results {
        if already_persisted.contains(&result.tool_use_id) {
            return Err(TransitionError::InvalidTransition(format!(
                "Attempted to persist duplicate tool result for tool_use_id: {}",
                result.tool_use_id
            )));
        }
    }
    Ok(())
}

impl ToolResult {
    fn display_data(&self) -> Option<Value> {
        self.display_data.clone()
    }
}

fn usage_to_data(usage: &crate::llm::Usage) -> UsageData {
    UsageData {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
    }
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
    match kind {
        crate::llm::LlmErrorKind::Auth => ErrorKind::Auth,
        crate::llm::LlmErrorKind::RateLimit => ErrorKind::RateLimit,
        crate::llm::LlmErrorKind::Network => ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => ErrorKind::InvalidRequest,
        _ => ErrorKind::Unknown,
    }
}

impl ErrorKind {
    fn is_retryable(&self) -> bool {
        matches!(self, ErrorKind::Network | ErrorKind::RateLimit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_context() -> ConvContext {
        ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model")
    }

    #[test]
    fn test_idle_to_llm_requesting() {
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::UserMessage {
                text: "Hello".to_string(),
                images: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
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
                images: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
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
                images: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
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
        use crate::state_machine::state::{BashInput, BashMode, ToolCall, ToolInput};

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
                persisted_tool_ids: HashSet::new(),
                pending_sub_agents: vec![],
            },
            &test_context(),
            Event::UserCancel,
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

        // Phase 2: ToolAborted -> Idle with synthetic results
        let result2 = transition(
            &result.new_state,
            &test_context(),
            Event::ToolAborted {
                tool_use_id: "tool-1".to_string(),
            },
        )
        .unwrap();

        assert!(matches!(result2.new_state, ConvState::Idle));
        assert!(result2
            .effects
            .iter()
            .any(|e| matches!(e, Effect::PersistToolResults { .. })));
    }

    #[test]
    fn test_duplicate_persist_validation_fails() {
        #[allow(unused_imports)]
        use crate::state_machine::state::{BashInput, BashMode, ToolCall, ToolInput};

        // Create a CancellingTool state where tool-1 was already persisted
        let mut already_persisted = HashSet::new();
        already_persisted.insert("tool-1".to_string());

        let state = ConvState::CancellingTool {
            tool_use_id: "tool-1".to_string(), // This tool would create duplicate
            skipped_tools: vec![],
            persisted_tool_ids: already_persisted,
        };

        // This should fail because tool-1 is in persisted_tool_ids
        let result = transition(
            &state,
            &test_context(),
            Event::ToolAborted {
                tool_use_id: "tool-1".to_string(),
            },
        );

        assert!(
            matches!(result, Err(TransitionError::InvalidTransition(_))),
            "Should fail with InvalidTransition due to duplicate persist"
        );
    }
}
