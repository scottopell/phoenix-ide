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
use super::event::{CoreEvent, ParentEvent, ParentOnlyEvent, SubAgentEvent, SubAgentOnlyEvent};
use super::outcome::{EffectOutcome, InvalidOutcome, LlmOutcome, PersistOutcome, ToolExecOutcome};
use super::state::{
    AssistantMessage, ContextExhaustionBehavior, CoreState, ModeKind, ParentState, PendingSubAgent,
    RecoveryKind, SubAgentResult, SubAgentState, TaskApprovalOutcome, ToolCall, ToolInput,
};
use super::{ConvContext, ConvState, Effect, Event};
use crate::db::{ErrorKind, ToolResult, UsageData};
use serde_json::json;
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

/// Errors that can occur during transition.
///
/// Every variant is either payload-free or carries structured data. In
/// particular `InvalidTransition` carries `&'static str` discriminators
/// sourced from [`ConvState::variant_name`] / [`Event::variant_name`]
/// instead of a `format!("{state:?}/{event:?}")` dump — task 24682
/// follow-up. This means `Display`-formatting a `TransitionError`
/// anywhere in the codebase produces output that is always safe to
/// show to humans, never leaks the internal `Debug` shape of
/// `ConvState` or `Event`, and never embeds payload data.
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
    #[error("Invalid transition: no arm for state={state} event={event}")]
    InvalidTransition {
        /// Variant name of the `ConvState` that didn't have a matching
        /// transition arm, e.g. `"Idle"`. Populated via
        /// [`ConvState::variant_name`]. Never contains payload data.
        state: &'static str,
        /// Variant name of the `Event` we were trying to apply, e.g.
        /// `"UserCancel"`. Populated via [`Event::variant_name`].
        /// Never contains payload data.
        event: &'static str,
    },
}

/// Pure transition function — compatibility wrapper.
///
/// Dispatches to `transition_parent` or `transition_sub_agent` based on
/// `context.is_sub_agent`. `ConvState`/`Event` are converted to the split types,
/// the result is converted back to `ConvState`. This preserves the existing API
/// while the split functions enforce structural type safety.
///
/// REQ-BED-001: This function is pure - given the same inputs, it always
/// produces the same outputs, with no I/O side effects.
pub fn transition(
    state: &ConvState,
    context: &ConvContext,
    event: Event,
) -> Result<TransitionResult, TransitionError> {
    if context.is_sub_agent {
        let sub_state = SubAgentState::try_from(state.clone()).map_err(|e| {
            TransitionError::InvalidTransition {
                state: e.from_variant,
                event: event.variant_name(),
            }
        })?;
        let sub_event = match SubAgentEvent::try_from(event) {
            Ok(e) => e,
            Err(e) => {
                // Parent-only events reaching a sub-agent context are invalid.
                // Terminal states absorb silently; non-terminal states reject.
                if sub_state.is_terminal() {
                    return Ok(TransitionResult::new(state.clone()));
                }
                return Err(TransitionError::InvalidTransition {
                    state: state.variant_name(),
                    event: e.event_variant,
                });
            }
        };
        let result = transition_sub_agent(&sub_state, context, sub_event)?;
        Ok(result.into_conv_result())
    } else {
        let parent_state = ParentState::try_from(state.clone()).map_err(|e| {
            TransitionError::InvalidTransition {
                state: e.from_variant,
                event: event.variant_name(),
            }
        })?;
        let parent_event = match ParentEvent::try_from(event) {
            Ok(e) => e,
            Err(e) => {
                // Sub-agent-only events reaching a parent context are stale/invalid.
                // Terminal states absorb silently; non-terminal states reject.
                if parent_state.is_terminal() {
                    return Ok(TransitionResult::new(state.clone()));
                }
                return Err(TransitionError::InvalidTransition {
                    state: state.variant_name(),
                    event: e.event_variant,
                });
            }
        };
        let result = transition_parent(&parent_state, context, parent_event)?;
        Ok(result.into_conv_result())
    }
}

// ============================================================================
// Split transition functions — CoreState, ParentState, SubAgentState
// ============================================================================

/// Result of a parent state transition
#[derive(Debug)]
pub struct ParentTransitionResult {
    pub new_state: ParentState,
    pub effects: Vec<Effect>,
}

impl ParentTransitionResult {
    fn new(state: ParentState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    fn into_conv_result(self) -> TransitionResult {
        TransitionResult {
            new_state: self.new_state.into(),
            effects: self.effects,
        }
    }
}

/// Result of a sub-agent state transition
#[derive(Debug)]
pub struct SubAgentTransitionResult {
    pub new_state: SubAgentState,
    pub effects: Vec<Effect>,
}

impl SubAgentTransitionResult {
    fn new(state: SubAgentState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    fn into_conv_result(self) -> TransitionResult {
        TransitionResult {
            new_state: self.new_state.into(),
            effects: self.effects,
        }
    }
}

/// Result of a core state transition
#[derive(Debug)]
pub struct CoreTransitionResult {
    pub new_state: CoreState,
    pub effects: Vec<Effect>,
}

impl CoreTransitionResult {
    fn new(state: CoreState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    fn into_parent_result(self) -> ParentTransitionResult {
        ParentTransitionResult {
            new_state: ParentState::Core(self.new_state),
            effects: self.effects,
        }
    }

    fn into_sub_agent_result(self) -> SubAgentTransitionResult {
        SubAgentTransitionResult {
            new_state: SubAgentState::Core(self.new_state),
            effects: self.effects,
        }
    }
}

// ============================================================================
// transition_core — shared behavior for both parent and sub-agent
// ============================================================================

/// Core transition function handling behavior shared by both conversation types.
///
/// Routes (state, event) pairs to domain-specific handlers. Each handler is
/// independently testable with explicit inputs and outputs.
///
/// Does NOT handle: `propose_task` interception (parent-only), terminal tools
/// (sub-agent-only), `LlmError` -> `Error` vs `Failed` (diverges by type),
/// `UserCancel` from `LlmRequesting` (parent -> `Idle`, sub-agent -> `Failed`).
pub fn transition_core(
    state: &CoreState,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, &event) {
        // User Message Handling (REQ-BED-002)
        (
            CoreState::Idle | CoreState::Error { .. },
            CoreEvent::UserMessage {
                text,
                llm_text,
                images,
                message_id,
                user_agent,
                skill_invocation,
            },
        ) => Ok(
            CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_user_message(
                    text.clone(),
                    llm_text.clone(),
                    images.clone(),
                    message_id.clone(),
                    user_agent.clone(),
                    skill_invocation.clone(),
                ))
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        ),

        (
            CoreState::LlmRequesting { .. }
            | CoreState::ToolExecuting { .. }
            | CoreState::AwaitingSubAgents { .. },
            CoreEvent::UserMessage { .. },
        ) => Err(TransitionError::AgentBusy),

        (
            CoreState::CancellingTool { .. } | CoreState::CancellingSubAgents { .. },
            CoreEvent::UserMessage { .. },
        ) => Err(TransitionError::CancellationInProgress),

        // LLM Response Processing (REQ-BED-003)
        (CoreState::LlmRequesting { .. }, CoreEvent::LlmResponse { .. }) => {
            handle_core_llm_response(state, context, event)
        }

        // Error Handling and Retry (REQ-BED-006)
        (CoreState::LlmRequesting { .. }, CoreEvent::LlmError { .. })
        | (CoreState::LlmRequesting { .. }, CoreEvent::RetryTimeout { .. }) => {
            handle_core_error_retry(state, event)
        }

        // Tool Execution (REQ-BED-004)
        (CoreState::ToolExecuting { .. }, CoreEvent::ToolComplete { .. })
        | (CoreState::ToolExecuting { .. }, CoreEvent::SpawnAgentsComplete { .. }) => {
            handle_core_tool_complete(state, event)
        }

        // Cancellation (REQ-BED-005)
        (CoreState::AwaitingSubAgents { .. }, CoreEvent::UserCancel { .. })
        | (CoreState::ToolExecuting { .. }, CoreEvent::UserCancel { .. })
        | (CoreState::LlmRequesting { .. }, CoreEvent::UserCancel { .. })
        | (CoreState::CancellingTool { .. }, CoreEvent::ToolAborted { .. })
        | (CoreState::CancellingTool { .. }, CoreEvent::ToolComplete { .. })
        | (CoreState::CancellingTool { .. }, CoreEvent::SubAgentResult { .. }) => {
            handle_core_cancellation(state, event)
        }

        // Sub-Agent Results (REQ-BED-008)
        (CoreState::AwaitingSubAgents { .. }, CoreEvent::SubAgentResult { .. })
        | (CoreState::CancellingSubAgents { .. }, CoreEvent::SubAgentResult { .. }) => {
            handle_core_sub_agents(state, event)
        }

        // Context Continuation (REQ-BED-019 through REQ-BED-024)
        (CoreState::AwaitingContinuation { .. }, CoreEvent::LlmError { .. })
        | (CoreState::AwaitingContinuation { .. }, CoreEvent::RetryTimeout { .. })
        | (CoreState::Idle, CoreEvent::UserTriggerContinuation) => {
            handle_core_continuation(state, event)
        }

        // Stale LlmResponse after cancel
        (CoreState::Idle, CoreEvent::LlmResponse { .. }) => {
            Ok(CoreTransitionResult::new(CoreState::Idle))
        }

        // Stale UserTriggerContinuation: any non-Idle Core state means the
        // conversation is already in flight (LLM round, tools, sub-agents,
        // continuation summary) or in a sub-agent terminal state. The user's
        // intent ("summarize now") is either being served by the in-flight
        // path or no longer meaningful. Absorbing avoids the SSE-vs-click
        // race surfacing as an error to the user.
        (state, CoreEvent::UserTriggerContinuation) => {
            tracing::debug!(
                state = state.variant_name(),
                "Absorbing stale UserTriggerContinuation"
            );
            Ok(CoreTransitionResult::new(state.clone()))
        }

        // Invalid Transitions
        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

// ============================================================================
// Domain-specific handlers for transition_core
// ============================================================================

/// Handles `LlmResponse` events when in `LlmRequesting` state.
///
/// By the time we get here, `propose_task` interception, `ask_user_question`
/// interception, context exhaustion check, and sub-agent terminal tool handling
/// have already been done by the parent/sub-agent wrappers. `LlmResponse` here
/// means "normal tool execution or text-only response."
#[allow(clippy::unnecessary_wraps)]
fn handle_core_llm_response(
    state: &CoreState,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    let CoreEvent::LlmResponse {
        content,
        tool_calls,
        end_turn: _,
        usage: usage_data,
    } = event
    else {
        unreachable!("handle_core_llm_response called with non-LlmResponse event");
    };
    let CoreState::LlmRequesting { .. } = state else {
        unreachable!("handle_core_llm_response called in non-LlmRequesting state");
    };

    if tool_calls.is_empty() && content.is_empty() {
        tracing::debug!("LLM returned end_turn with empty content — no message to persist");
        return Ok(CoreTransitionResult::new(CoreState::Idle)
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_agent_done()));
    }

    if tool_calls.is_empty() {
        return Ok(CoreTransitionResult::new(CoreState::Idle)
            .with_effect(Effect::persist_agent_message(
                content,
                Some(usage_data),
                &context.working_dir,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_agent_done()));
    }

    // Has tools -> ToolExecuting
    let first = tool_calls[0].clone();
    let rest = tool_calls[1..].to_vec();
    let remaining_count = rest.len();
    let display_data = compute_bash_display_data(&content, &context.working_dir);
    let assistant_message = AssistantMessage::new(content, Some(usage_data), display_data);

    Ok(CoreTransitionResult::new(CoreState::ToolExecuting {
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

/// Handles `ToolComplete` and `SpawnAgentsComplete` events during `ToolExecuting` state.
#[allow(clippy::too_many_lines)]
fn handle_core_tool_complete(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    let CoreState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        pending_sub_agents,
        assistant_message,
    } = state
    else {
        unreachable!("handle_core_tool_complete called in non-ToolExecuting state");
    };

    match event {
        // ToolComplete (more tools remaining) -> next tool
        CoreEvent::ToolComplete {
            tool_use_id,
            result,
        } if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_results = completed_results.clone();
            new_results.push(result);
            let completed_count = new_results.len();

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();
            let remaining_count = new_remaining.len();

            Ok(CoreTransitionResult::new(CoreState::ToolExecuting {
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

        // ToolComplete (last tool, no sub-agents) -> LlmRequesting
        CoreEvent::ToolComplete {
            tool_use_id,
            result,
        } if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && pending_sub_agents.is_empty() =>
        {
            let mut all_results = completed_results.clone();
            all_results.push(result);

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in last-tool transition");

            Ok(
                CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(notify_llm_requesting(1))
                    .with_effect(Effect::RequestLlm),
            )
        }

        // ToolComplete (last tool, has sub-agents) -> AwaitingSubAgents
        CoreEvent::ToolComplete {
            tool_use_id,
            result,
        } if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && !pending_sub_agents.is_empty() =>
        {
            let mut all_results = completed_results.clone();
            all_results.push(result);

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect(
                    "tool_use/tool_result count mismatch in last-tool-with-subagents transition",
                );

            Ok(CoreTransitionResult::new(CoreState::AwaitingSubAgents {
                pending: pending_sub_agents.clone(),
                completed_results: vec![],
                spawn_tool_id: None,
            })
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(notify_awaiting_sub_agents(pending_sub_agents, &[])))
        }

        // SpawnAgentsComplete (more tools) -> accumulate
        CoreEvent::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        } if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_results = completed_results.clone();
            new_results.push(result);
            let completed_count = new_results.len();

            let mut new_pending = pending_sub_agents.clone();
            new_pending.extend(spawned);

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();
            let remaining_count = new_remaining.len();

            Ok(CoreTransitionResult::new(CoreState::ToolExecuting {
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

        // SpawnAgentsComplete (last tool) -> AwaitingSubAgents
        CoreEvent::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        } if tool_use_id == current_tool.id && remaining_tools.is_empty() => {
            let mut all_pending = pending_sub_agents.clone();
            all_pending.extend(spawned);

            let mut all_results = completed_results.clone();
            let spawn_id = result.tool_use_id.clone();
            all_results.push(result);

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in spawn-agents-last transition");

            Ok(CoreTransitionResult::new(CoreState::AwaitingSubAgents {
                pending: all_pending.clone(),
                completed_results: vec![],
                spawn_tool_id: Some(spawn_id),
            })
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(notify_awaiting_sub_agents(&all_pending, &[])))
        }

        // tool_use_id mismatch or unexpected event variant
        _ => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Handles cancellation-related events: `UserCancel` from active states,
/// `ToolAborted`/`ToolComplete` during `CancellingTool`, `SubAgentResult` during `CancellingTool`.
#[allow(clippy::too_many_lines)]
fn handle_core_cancellation(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // AwaitingSubAgents + UserCancel -> CancellingSubAgents
        (
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                ..
            },
            CoreEvent::UserCancel { .. },
        ) => {
            let ids: Vec<String> = pending.iter().map(|p| p.agent_id.clone()).collect();
            Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                pending: pending.clone(),
                completed_results: completed_results.clone(),
            })
            .with_effect(Effect::CancelSubAgents { ids })
            .with_effect(Effect::PersistState))
        }

        // ToolExecuting + UserCancel -> CancellingTool
        (
            CoreState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            CoreEvent::UserCancel { .. },
        ) => {
            let mut result = CoreTransitionResult::new(CoreState::CancellingTool {
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

            if !pending_sub_agents.is_empty() {
                let ids: Vec<String> = pending_sub_agents
                    .iter()
                    .map(|p| p.agent_id.clone())
                    .collect();
                result = result.with_effect(Effect::CancelSubAgents { ids });
            }

            Ok(result)
        }

        // LlmRequesting + UserCancel -> Idle
        (CoreState::LlmRequesting { .. }, CoreEvent::UserCancel { .. }) => {
            Ok(CoreTransitionResult::new(CoreState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::AbortLlm)
                .with_effect(Effect::notify_agent_done()))
        }

        // CancellingTool + ToolAborted -> Idle or CancellingSubAgents
        (
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreEvent::ToolAborted {
                tool_use_id: aborted_id,
            },
        ) if *tool_use_id == aborted_id => {
            let all_results = build_cancellation_results(
                completed_results,
                tool_use_id,
                "Cancelled by user",
                skipped_tools,
            );

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in cancellation transition");

            if pending_sub_agents.is_empty() {
                Ok(CoreTransitionResult::new(CoreState::Idle)
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                    pending: pending_sub_agents.clone(),
                    completed_results: vec![],
                })
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState))
            }
        }

        // CancellingTool + ToolComplete -> Idle or CancellingSubAgents
        (
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreEvent::ToolComplete {
                tool_use_id: completed_id,
                result: _,
            },
        ) if *tool_use_id == completed_id => {
            let all_results = build_cancellation_results(
                completed_results,
                tool_use_id,
                "Cancelled by user",
                skipped_tools,
            );

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in cancellation-complete transition");

            if pending_sub_agents.is_empty() {
                Ok(CoreTransitionResult::new(CoreState::Idle)
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                    pending: pending_sub_agents.clone(),
                    completed_results: vec![],
                })
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState))
            }
        }

        // CancellingTool + SubAgentResult -> absorb
        (
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreEvent::SubAgentResult { agent_id, .. },
        ) if pending_sub_agents.iter().any(|p| p.agent_id == agent_id) => {
            let new_pending: Vec<_> = pending_sub_agents
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            Ok(CoreTransitionResult::new(CoreState::CancellingTool {
                tool_use_id: tool_use_id.clone(),
                skipped_tools: skipped_tools.clone(),
                completed_results: completed_results.clone(),
                assistant_message: assistant_message.clone(),
                pending_sub_agents: new_pending,
            })
            .with_effect(Effect::PersistState))
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Builds the tool results list for cancellation transitions, including the
/// cancelled current tool and skipped remaining tools.
fn build_cancellation_results(
    completed_results: &[ToolResult],
    cancelled_tool_id: &str,
    cancel_reason: &str,
    skipped_tools: &[ToolCall],
) -> Vec<ToolResult> {
    let mut all_results = completed_results.to_vec();
    all_results.push(ToolResult::cancelled(
        cancelled_tool_id.to_string(),
        cancel_reason,
    ));
    for tool in skipped_tools {
        all_results.push(ToolResult::cancelled(
            tool.id.clone(),
            "Skipped due to cancellation",
        ));
    }
    all_results
}

/// Handles `SubAgentResult` events in `AwaitingSubAgents` and `CancellingSubAgents` states.
#[allow(clippy::too_many_lines)]
fn handle_core_sub_agents(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // AwaitingSubAgents + SubAgentResult (more pending)
        (
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
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

            let notify = notify_awaiting_sub_agents(&new_pending, &new_results);

            Ok(CoreTransitionResult::new(CoreState::AwaitingSubAgents {
                pending: new_pending,
                completed_results: new_results,
                spawn_tool_id: spawn_tool_id.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(notify))
        }

        // AwaitingSubAgents + SubAgentResult (last one) -> LlmRequesting
        (
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
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
                CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistSubAgentResults {
                        results: new_results,
                        spawn_tool_id: spawn_tool_id.clone(),
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(notify_llm_requesting(1))
                    .with_effect(Effect::RequestLlm),
            )
        }

        // CancellingSubAgents + SubAgentResult (more pending)
        (
            CoreState::CancellingSubAgents {
                pending,
                completed_results,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
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

            Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                pending: new_pending,
                completed_results: new_results,
            })
            .with_effect(Effect::PersistState))
        }

        // CancellingSubAgents + SubAgentResult (last one) -> Idle
        (
            CoreState::CancellingSubAgents { pending, .. },
            CoreEvent::SubAgentResult { agent_id, .. },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() == 1 => {
            Ok(CoreTransitionResult::new(CoreState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Handles `LlmError` and `RetryTimeout` events during `LlmRequesting` state.
fn handle_core_error_retry(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // Retryable LlmError below max -> retry (shared)
        (CoreState::LlmRequesting { attempt }, CoreEvent::LlmError { error_kind, .. })
            if error_kind.is_retryable() && *attempt < MAX_RETRY_ATTEMPTS =>
        {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);

            Ok(CoreTransitionResult::new(CoreState::LlmRequesting {
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

        // Non-retryable or exhausted LlmError -> Error (core default)
        (
            CoreState::LlmRequesting { attempt },
            CoreEvent::LlmError {
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

            Ok(CoreTransitionResult::new(CoreState::Error {
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
            CoreState::LlmRequesting { attempt },
            CoreEvent::RetryTimeout {
                attempt: retry_attempt,
            },
        ) if *attempt == retry_attempt => {
            Ok(
                CoreTransitionResult::new(CoreState::LlmRequesting { attempt: *attempt })
                    .with_effect(Effect::RequestLlm),
            )
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Handles continuation-related events: `LlmError`/`RetryTimeout` during
/// `AwaitingContinuation`, and `UserTriggerContinuation` from Idle.
fn handle_core_continuation(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // LlmError during continuation - retry
        (
            CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
            CoreEvent::LlmError { error_kind, .. },
        ) if error_kind.is_retryable() && *attempt < MAX_RETRY_ATTEMPTS => {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);

            Ok(CoreTransitionResult::new(CoreState::AwaitingContinuation {
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

        // RetryTimeout during continuation
        (
            CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
            CoreEvent::RetryTimeout {
                attempt: timeout_attempt,
            },
        ) if *attempt == timeout_attempt => {
            Ok(CoreTransitionResult::new(CoreState::AwaitingContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
                attempt: *attempt,
            })
            .with_effect(Effect::RequestContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
            }))
        }

        // UserTriggerContinuation from Idle (REQ-BED-023)
        (CoreState::Idle, CoreEvent::UserTriggerContinuation) => {
            Ok(CoreTransitionResult::new(CoreState::AwaitingContinuation {
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

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

// ============================================================================
// transition_parent — parent-specific transitions, delegates core
// ============================================================================

/// Parent transition function. Handles parent-only states and events, delegates
/// core state + core event combinations to `transition_core`.
#[allow(clippy::too_many_lines)]
pub fn transition_parent(
    state: &ParentState,
    context: &ConvContext,
    event: ParentEvent,
) -> Result<ParentTransitionResult, TransitionError> {
    match (state, event) {
        // ============================================================
        // Parent-only state: AwaitingTaskApproval
        // ============================================================
        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Core(CoreEvent::UserMessage { .. } | CoreEvent::UserTriggerContinuation),
        ) => Err(TransitionError::AwaitingTaskApproval),

        (
            ParentState::AwaitingTaskApproval {
                title,
                priority,
                plan,
            },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Approved,
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting { attempt: 1 }))
                .with_effect(Effect::ApproveTask {
                    title: title.clone(),
                    priority: priority.clone(),
                    plan: plan.clone(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        ),

        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::FeedbackProvided { annotations },
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting { attempt: 1 }))
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

        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Rejected,
            })
            | ParentEvent::Core(CoreEvent::UserCancel { .. }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::Idle))
                .with_effect(Effect::PersistMessage {
                    content: crate::db::MessageContent::system("Task rejected."),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()),
        ),

        // ============================================================
        // Parent-only state: AwaitingUserResponse
        // ============================================================
        (
            ParentState::AwaitingUserResponse { .. },
            ParentEvent::Core(CoreEvent::UserMessage { .. } | CoreEvent::UserTriggerContinuation),
        ) => Err(TransitionError::AwaitingUserResponse),

        (
            ParentState::AwaitingUserResponse { questions, .. },
            ParentEvent::Parent(ParentOnlyEvent::UserQuestionResponse {
                answers,
                annotations,
            }),
        ) => {
            let answers_text = questions
                .iter()
                .filter_map(|q| {
                    let a = answers.get(&q.question)?;
                    let q_text = &q.question;
                    let mut parts = vec![format!("\"{}\" = \"{}\"", q_text, a)];
                    let question_data = questions.iter().find(|qq| qq.question == *q_text);
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
                    if let Some(ref anns) = annotations {
                        if let Some(ann) = anns.get(q_text.as_str()) {
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
                ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting {
                    attempt: 1,
                }))
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

        (
            ParentState::AwaitingUserResponse { .. },
            ParentEvent::Core(CoreEvent::UserCancel { .. }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting {
                attempt: 1,
            }))
            .with_effect(Effect::PersistMessage {
                content: crate::db::MessageContent::user(
                    "I declined to answer those questions. Please proceed using your own judgment."
                        .to_string(),
                ),
                display_data: None,
                usage_data: None,
                message_id: uuid::Uuid::new_v4().to_string(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(notify_llm_requesting(1))
            .with_effect(Effect::RequestLlm),
        ),

        // ============================================================
        // Parent-only state: AwaitingRecovery (REQ-BED-030)
        // ============================================================
        (
            ParentState::AwaitingRecovery { .. },
            ParentEvent::Parent(ParentOnlyEvent::CredentialBecameAvailable),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting { attempt: 1 }))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestLlm),
        ),

        (
            ParentState::AwaitingRecovery { error_kind, .. },
            ParentEvent::Parent(ParentOnlyEvent::CredentialHelperFailed { message }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::Error {
                message: message.clone(),
                error_kind: error_kind.clone(),
            }))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change(
                "error",
                json!({ "message": message }),
            )),
        ),

        (ParentState::AwaitingRecovery { .. }, ParentEvent::Core(CoreEvent::UserCancel { .. })) => {
            Ok(
                ParentTransitionResult::new(ParentState::Core(CoreState::Idle))
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change("idle", json!({}))),
            )
        }

        // ============================================================
        // Parent-only state: ContextExhausted
        // ============================================================
        (
            ParentState::ContextExhausted { .. },
            ParentEvent::Core(CoreEvent::UserMessage { .. }),
        ) => Err(TransitionError::ContextExhausted),

        (state @ ParentState::ContextExhausted { .. }, _event) => {
            Ok(ParentTransitionResult::new(state.clone()))
        }

        // ============================================================
        // Parent-only state: Terminal
        // ============================================================
        (ParentState::Terminal, ParentEvent::Core(CoreEvent::UserMessage { .. })) => {
            Err(TransitionError::ConversationTerminal)
        }

        (ParentState::Terminal, _event) => Ok(ParentTransitionResult::new(ParentState::Terminal)),

        // ============================================================
        // Task resolution: Idle + TaskResolved -> Terminal (REQ-BED-029)
        // ============================================================
        (
            ParentState::Core(CoreState::Idle),
            ParentEvent::Parent(ParentOnlyEvent::TaskResolved {
                system_message,
                repo_root,
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Terminal).with_effect(Effect::ResolveTask {
                system_message,
                repo_root,
            }),
        ),

        // ============================================================
        // Parent-specific LLM response interceptions (before core)
        //
        // Combined into a single match arm to avoid borrow-after-move
        // issues with guards on the same event payload.
        // ============================================================
        (
            ParentState::Core(CoreState::LlmRequesting { .. }),
            ParentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                usage: usage_data,
                ..
            }),
        ) => {
            // REQ-BED-028: propose_task interception (checked first)
            if let Some(tool) = tool_calls
                .iter()
                .find(|t| matches!(t.input, ToolInput::ProposeTask(_)))
            {
                // propose_task is only valid in Managed mode (Explore/Work lifecycle).
                // Direct and Branch mode should never produce this tool call.
                if context.mode == ModeKind::Direct || context.mode == ModeKind::Branch {
                    return Ok(
                        ParentTransitionResult::new(ParentState::Core(CoreState::Error {
                            message: "propose_task is not available in Direct or Branch mode"
                                .to_string(),
                            error_kind: ErrorKind::InvalidRequest,
                        }))
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change(
                            "error",
                            json!({"message": "propose_task not available in this mode"}),
                        )),
                    );
                }

                if tool_calls.len() > 1 {
                    let msg = "propose_task must be the only tool in response".to_string();
                    return Ok(
                        ParentTransitionResult::new(ParentState::Core(CoreState::Error {
                            message: msg.clone(),
                            error_kind: ErrorKind::InvalidRequest,
                        }))
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change(
                            "error",
                            json!({ "message": msg }),
                        )),
                    );
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

                    return Ok(
                        ParentTransitionResult::new(ParentState::AwaitingTaskApproval {
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
                        )),
                    );
                }
                unreachable!("propose_task_tool matched but input was not ProposeTask");
            }

            // REQ-AUQ-001: ask_user_question interception
            if let Some(tool) = tool_calls
                .iter()
                .find(|t| matches!(t.input, ToolInput::AskUserQuestion(_)))
            {
                if tool_calls.len() > 1 {
                    let msg = "ask_user_question must be the only tool in response".to_string();
                    return Ok(
                        ParentTransitionResult::new(ParentState::Core(CoreState::Error {
                            message: msg.clone(),
                            error_kind: ErrorKind::InvalidRequest,
                        }))
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change(
                            "error",
                            json!({ "message": msg }),
                        )),
                    );
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

                    return Ok(
                        ParentTransitionResult::new(ParentState::AwaitingUserResponse {
                            questions: input.questions.clone(),
                            tool_use_id: tool.id.clone(),
                        })
                        .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change(
                            "awaiting_user_response",
                            json!({ "questions": input.questions }),
                        )),
                    );
                }
                unreachable!("ask_question_tool matched but input was not AskUserQuestion");
            }

            // REQ-BED-019: Context exhaustion check (after propose_task/ask_user_question)
            if should_trigger_continuation(&usage_data, context.context_window) {
                let tr = handle_context_exhaustion(context, content, tool_calls, usage_data);
                return Ok(ParentTransitionResult {
                    new_state: ParentState::try_from(tr.new_state)
                        .expect("handle_context_exhaustion returns parent-valid state"),
                    effects: tr.effects,
                });
            }

            // No interception needed — delegate to core
            let core_event = CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn: false,
                usage: usage_data,
            };
            let ParentState::Core(core_state) = state else {
                unreachable!()
            };
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_parent_result())
        }

        // AwaitingRecovery interception for auth errors
        (
            ParentState::Core(CoreState::LlmRequesting { .. }),
            ParentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                recovery_in_progress: true,
                ..
            }),
        ) if matches!(error_kind, ErrorKind::Auth) => {
            Ok(ParentTransitionResult::new(ParentState::AwaitingRecovery {
                message: message.clone(),
                error_kind: error_kind.clone(),
                recovery_kind: RecoveryKind::Credential,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change(
                "awaiting_recovery",
                json!({
                    "message": message,
                    "recovery_kind": "credential"
                }),
            )))
        }

        // ============================================================
        // Parent-specific continuation transitions
        // ============================================================
        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::ContinuationResponse { summary }),
        ) => Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
            summary: summary.clone(),
        })
        .with_effect(Effect::persist_continuation_message(&summary))
        .with_effect(Effect::PersistState)
        .with_effect(Effect::NotifyContextExhausted { summary })),

        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::ContinuationFailed { error }),
        ) => {
            let fallback = format!(
                "Context limit reached. The continuation summary could not be generated: {error}. \
                Please start a new conversation."
            );
            Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
                summary: fallback.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&fallback))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
        }

        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::UserCancel { .. }),
        ) => {
            let cancelled =
                "Continuation cancelled by user. Please start a new conversation.".to_string();
            Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
                summary: cancelled.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&cancelled))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::AbortLlm)
            .with_effect(Effect::NotifyContextExhausted { summary: cancelled }))
        }

        // LlmError during continuation - retries exhausted -> ContextExhausted
        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::LlmError {
                ref message,
                ref error_kind,
                ..
            }),
        ) if !error_kind.is_retryable() || {
            // Check if we're at/past max retries
            match state {
                ParentState::Core(CoreState::AwaitingContinuation { attempt, .. }) => {
                    *attempt >= MAX_RETRY_ATTEMPTS
                }
                _ => false,
            }
        } =>
        {
            let message = message.clone();
            let fallback = format!(
                "Context limit reached. The continuation summary could not be generated: {message}. \
                Please start a new conversation."
            );
            Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
                summary: fallback.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&fallback))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
        }

        // Stale TaskApprovalResponse
        (state, ParentEvent::Parent(ParentOnlyEvent::TaskApprovalResponse { .. })) => {
            tracing::debug!("Absorbing stale TaskApprovalResponse");
            Ok(ParentTransitionResult::new(state.clone()))
        }

        // ============================================================
        // Delegate to core
        // ============================================================
        (ParentState::Core(core_state), ParentEvent::Core(core_event)) => {
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_parent_result())
        }

        // Invalid: parent-only events in non-matching states
        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

// ============================================================================
// transition_sub_agent — sub-agent-specific transitions, delegates core
// ============================================================================

/// Sub-agent transition function. Handles sub-agent-only states and events,
/// intercepts core events with sub-agent-specific behavior, delegates the
/// rest to `transition_core`.
#[allow(clippy::too_many_lines)]
pub fn transition_sub_agent(
    state: &SubAgentState,
    context: &ConvContext,
    event: SubAgentEvent,
) -> Result<SubAgentTransitionResult, TransitionError> {
    use crate::state_machine::state::SubAgentOutcome;

    match (state, event) {
        // ============================================================
        // Terminal state absorption (Completed / Failed)
        // ============================================================
        (SubAgentState::Completed { .. } | SubAgentState::Failed { .. }, _event) => {
            Ok(SubAgentTransitionResult::new(state.clone()))
        }

        // ============================================================
        // Grace Turn Exhausted (REQ-BED-026)
        // ============================================================
        (
            _state,
            SubAgentEvent::SubAgent(SubAgentOnlyEvent::GraceTurnExhausted { result: Some(text) }),
        ) => Ok(SubAgentTransitionResult::new(SubAgentState::Completed {
            result: text.clone(),
        })
        .with_effect(Effect::PersistState)
        .with_effect(Effect::NotifyParent {
            outcome: SubAgentOutcome::Success { result: text },
        })),

        (
            _state,
            SubAgentEvent::SubAgent(SubAgentOnlyEvent::GraceTurnExhausted { result: None }),
        ) => {
            let error = "Sub-agent exceeded turn limit with no output".to_string();
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
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
        // Sub-agent UserCancel -> Failed (from any non-terminal core state)
        // ============================================================
        (SubAgentState::Core(_), SubAgentEvent::Core(CoreEvent::UserCancel { reason })) => {
            let error = reason
                .clone()
                .unwrap_or_else(|| "Cancelled by parent".to_string());
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
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
        // Sub-agent LLM error handling (non-retryable or exhausted -> Failed)
        // ============================================================
        (
            SubAgentState::Core(CoreState::LlmRequesting { attempt }),
            SubAgentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                ..
            }),
        ) if !error_kind.is_retryable() || *attempt >= MAX_RETRY_ATTEMPTS => {
            let error_message = if error_kind.is_retryable() {
                format!("Failed after {attempt} attempts: {message}")
            } else {
                message
            };
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
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

        // ============================================================
        // Sub-agent LLM response handling (combined to avoid
        // borrow-after-move issues with guards)
        // ============================================================
        (
            SubAgentState::Core(CoreState::LlmRequesting { .. }),
            SubAgentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                usage: usage_data,
                ..
            }),
        ) => {
            // Context exhaustion check first (sub-agent fails immediately)
            if should_trigger_continuation(&usage_data, context.context_window) {
                let tr = handle_context_exhaustion(context, content, tool_calls, usage_data);
                return Ok(SubAgentTransitionResult {
                    new_state: SubAgentState::try_from(tr.new_state)
                        .expect("sub-agent context exhaustion returns Failed"),
                    effects: tr.effects,
                });
            }

            // Text-only response -> implicit Completed
            if tool_calls.is_empty() {
                let result_text = extract_text_from_content(&content);
                let mut tr = SubAgentTransitionResult::new(SubAgentState::Completed {
                    result: result_text.clone(),
                });
                if !content.is_empty() {
                    tr = tr.with_effect(Effect::persist_agent_message(
                        content,
                        Some(usage_data),
                        &context.working_dir,
                    ));
                }
                return Ok(tr.with_effect(Effect::PersistState).with_effect(
                    Effect::NotifyParent {
                        outcome: SubAgentOutcome::Success {
                            result: result_text,
                        },
                    },
                ));
            }

            // Terminal tools (submit_result/submit_error)
            if let Some(terminal_tool) = tool_calls.iter().find(|t| t.input.is_terminal_tool()) {
                if tool_calls.len() > 1 {
                    let msg =
                        "submit_result/submit_error must be the only tool in response".to_string();
                    return Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
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

                return match &terminal_tool.input {
                    ToolInput::SubmitResult(input) => {
                        Ok(SubAgentTransitionResult::new(SubAgentState::Completed {
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
                    ToolInput::SubmitError(input) => {
                        Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
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
                };
            }

            // Normal tool execution -> delegate to core
            let core_event = CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn: false,
                usage: usage_data,
            };
            let SubAgentState::Core(core_state) = state else {
                unreachable!()
            };
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_sub_agent_result())
        }

        // ============================================================
        // Delegate to core for everything else
        // ============================================================
        (SubAgentState::Core(core_state), SubAgentEvent::Core(core_event)) => {
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_sub_agent_result())
        }
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
                recovery_in_progress: false,
            }
        }
        LlmOutcome::ServerError { status, body } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: format!("Server error {status}: {body}"),
                error_kind: ErrorKind::ServerError,
                attempt,
                recovery_in_progress: false,
            }
        }
        LlmOutcome::NetworkError { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::Network,
                attempt,
                recovery_in_progress: false,
            }
        }
        LlmOutcome::TokenBudgetExceeded => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Token budget exceeded".to_string(),
                error_kind: ErrorKind::ContextExhausted,
                attempt,
                recovery_in_progress: false,
            }
        }
        LlmOutcome::AuthError {
            message,
            recovery_in_progress,
        } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::Auth,
                attempt,
                recovery_in_progress,
            }
        }
        LlmOutcome::RequestRejected { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::InvalidRequest,
                attempt,
                recovery_in_progress: false,
            }
        }
        LlmOutcome::Cancelled => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Request cancelled".to_string(),
                error_kind: ErrorKind::Cancelled,
                attempt,
                recovery_in_progress: false,
            }
        }
    }
}

/// Convert `ToolExecOutcome` to the equivalent `Event` for delegation to `transition()`.
fn tool_outcome_to_event(outcome: ToolExecOutcome) -> Event {
    match outcome {
        ToolExecOutcome::Completed(result) => Event::ToolComplete {
            tool_use_id: result.tool_use_id.clone(),
            result,
        },
        ToolExecOutcome::Aborted {
            tool_use_id,
            reason: _,
        } => Event::ToolAborted { tool_use_id },
        ToolExecOutcome::Failed { tool_use_id, error } => Event::ToolComplete {
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
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 100_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
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
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
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
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
        };

        // attempt == MAX_RETRY_ATTEMPTS (3), retryable error → retries exhausted
        let result = transition(
            &ConvState::LlmRequesting { attempt: 3 },
            &subagent_ctx,
            Event::LlmError {
                message: "Request timeout".to_string(),
                error_kind: ErrorKind::Network, // retryable
                attempt: 3,
                recovery_in_progress: false,
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
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
        };

        // Non-retryable error at attempt 1 → immediate failure
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &subagent_ctx,
            Event::LlmError {
                message: "Invalid API key".to_string(),
                error_kind: ErrorKind::Auth, // non-retryable
                attempt: 1,
                recovery_in_progress: false,
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
                recovery_in_progress: false,
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
    fn test_awaiting_user_response_cancel_resumes_llm() {
        use crate::db::MessageContent;
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
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "Decline should re-enter LlmRequesting so the agent proceeds (REQ-AUQ-004), got {:?}",
            result.new_state
        );

        let decline_message = result.effects.iter().find_map(|e| {
            if let Effect::PersistMessage {
                content: MessageContent::User(user),
                ..
            } = e
            {
                Some(user.text.clone())
            } else {
                None
            }
        });
        assert!(
            decline_message
                .as_deref()
                .is_some_and(|t| t.contains("declined")),
            "Decline must persist a user-role message telling the agent to proceed, got {decline_message:?}"
        );

        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Should dispatch a new LLM request so the agent resumes"
        );

        assert!(
            !result.effects.iter().any(|e| matches!(
                e,
                Effect::NotifyClient { event_type, .. } if event_type == "agent_done"
            )),
            "Decline must not notify agent_done — the agent is resuming, not stopping"
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
            "Should reject user messages with AwaitingUserResponse error, got {result:?}"
        );
    }

    /// Race scenario: SSE-stream connect triggers `should_auto_continue`,
    /// state moves Idle -> `LlmRequesting` before the client receives the
    /// state change. User clicks "trigger continuation" against the stale
    /// Idle UI. The state machine must absorb the event, not surface it as
    /// an `InvalidTransition` error to the user.
    #[test]
    fn user_trigger_continuation_in_llm_requesting_is_absorbed() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::UserTriggerContinuation,
        )
        .expect("absorb, not error");

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "state must not change when absorbing, got {:?}",
            result.new_state
        );
        assert!(
            result.effects.is_empty(),
            "absorb arm must produce no effects, got {} effects",
            result.effects.len()
        );
    }

    #[test]
    fn user_trigger_continuation_in_tool_executing_is_absorbed() {
        use crate::state_machine::state::{
            AssistantMessage, BashInput, BashMode, ToolCall, ToolInput,
        };

        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "tool-1",
                ToolInput::Bash(BashInput {
                    command: "echo".to_string(),
                    mode: BashMode::Default,
                }),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        };

        let result = transition(&state, &test_context(), Event::UserTriggerContinuation)
            .expect("absorb, not error");

        assert!(matches!(result.new_state, ConvState::ToolExecuting { .. }));
        assert!(result.effects.is_empty());
    }

    #[test]
    fn user_trigger_continuation_in_awaiting_continuation_is_absorbed() {
        // Already summarizing — clicking again is a redundant intent, not
        // an invalid one.
        let state = ConvState::AwaitingContinuation {
            rejected_tool_calls: vec![],
            attempt: 1,
        };

        let result = transition(&state, &test_context(), Event::UserTriggerContinuation)
            .expect("absorb, not error");

        assert!(matches!(
            result.new_state,
            ConvState::AwaitingContinuation { attempt: 1, .. }
        ));
        assert!(result.effects.is_empty());
    }

    #[test]
    fn user_trigger_continuation_from_idle_still_starts_continuation() {
        // Regression guard: the absorb arm must not steal the Idle path,
        // which is the actual user-initiated continuation flow.
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::UserTriggerContinuation,
        )
        .expect("Idle path should succeed");

        assert!(
            matches!(
                result.new_state,
                ConvState::AwaitingContinuation { attempt: 1, .. }
            ),
            "Idle + UserTriggerContinuation must enter AwaitingContinuation, got {:?}",
            result.new_state
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestContinuation { .. })),
            "Idle path must fire RequestContinuation effect"
        );
    }
}
