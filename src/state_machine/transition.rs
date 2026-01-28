//! Pure state transition function
//!
//! REQ-BED-001: Pure State Transitions
//! REQ-BED-002: User Message Handling
//! REQ-BED-003: LLM Response Processing
//! REQ-BED-004: Tool Execution Coordination
//! REQ-BED-005: Cancellation Handling
//! REQ-BED-006: Error Recovery

use super::{ConvContext, ConvState, Effect, Event};
use crate::db::{ErrorKind, ToolResult, UsageData};
use crate::llm::ContentBlock;
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
pub fn transition(
    state: &ConvState,
    _context: &ConvContext,
    event: Event,
) -> Result<TransitionResult, TransitionError> {
    match (state, event) {
        // ============================================================
        // User Message Handling (REQ-BED-002)
        // ============================================================
        
        // Idle + UserMessage -> LlmRequesting
        (ConvState::Idle, Event::UserMessage { text, images }) => {
            let content = build_user_message_content(&text, &images);
            Ok(TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_user_message(content))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestLlm))
        }

        // Error + UserMessage -> LlmRequesting (recovery, REQ-BED-006)
        (ConvState::Error { .. }, Event::UserMessage { text, images }) => {
            let content = build_user_message_content(&text, &images);
            Ok(TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_user_message(content))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestLlm))
        }

        // Busy states + UserMessage -> Reject (REQ-BED-002)
        (ConvState::LlmRequesting { .. }, Event::UserMessage { .. })
        | (ConvState::ToolExecuting { .. }, Event::UserMessage { .. })
        | (ConvState::AwaitingSubAgents { .. }, Event::UserMessage { .. }) => {
            Err(TransitionError::AgentBusy)
        }

        (ConvState::Cancelling { .. }, Event::UserMessage { .. }) => {
            Err(TransitionError::CancellationInProgress)
        }

        // ============================================================
        // LLM Response Processing (REQ-BED-003)
        // ============================================================

        // AwaitingLlm is an intermediate state - immediately transition to LlmRequesting
        // This is handled in the runtime, not here

        // LlmRequesting + LlmResponse with tools -> ToolExecuting
        (ConvState::LlmRequesting { .. }, Event::LlmResponse { content, end_turn: _, usage }) => {
            let tool_uses = extract_tool_uses(&content);
            
            if tool_uses.is_empty() {
                // No tools, just text response -> Idle
                let usage_data = usage_to_data(&usage);
                Ok(TransitionResult::new(ConvState::Idle)
                    .with_effect(Effect::persist_agent_message(
                        content_to_json(&content),
                        Some(usage_data),
                    ))
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                // Has tools -> ToolExecuting
                let (first, rest) = (tool_uses[0].clone(), tool_uses[1..].to_vec());
                let usage_data = usage_to_data(&usage);
                
                Ok(TransitionResult::new(ConvState::ToolExecuting {
                    current_tool_id: first.0.clone(),
                    remaining_tool_ids: rest.iter().map(|(id, _, _)| id.clone()).collect(),
                    completed_results: vec![],
                })
                .with_effect(Effect::persist_agent_message(
                    content_to_json(&content),
                    Some(usage_data),
                ))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::ExecuteTool {
                    tool_use_id: first.0,
                    name: first.1,
                    input: first.2,
                }))
            }
        }

        // ============================================================
        // Error Handling and Retry (REQ-BED-006)
        // ============================================================

        // LlmRequesting + LlmError (retryable) -> LlmRequesting with incremented attempt
        (ConvState::LlmRequesting { attempt }, Event::LlmError { message, error_kind, .. })
            if error_kind.is_retryable() && *attempt < MAX_RETRY_ATTEMPTS =>
        {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);
            
            Ok(TransitionResult::new(ConvState::LlmRequesting { attempt: new_attempt })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::ScheduleRetry { delay, attempt: new_attempt })
                .with_effect(Effect::notify_state_change("llm_requesting", json!({
                    "attempt": new_attempt,
                    "max_attempts": MAX_RETRY_ATTEMPTS,
                    "message": format!("Retrying... (attempt {})", new_attempt)
                }))))
        }

        // LlmRequesting + LlmError (non-retryable or exhausted) -> Error
        (ConvState::LlmRequesting { attempt }, Event::LlmError { message, error_kind, .. }) => {
            let error_message = if error_kind.is_retryable() {
                format!("Failed after {} attempts: {}", attempt, message)
            } else {
                message
            };
            
            Ok(TransitionResult::new(ConvState::Error {
                message: error_message.clone(),
                error_kind: error_kind.into(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change("error", json!({
                "message": error_message
            }))))
        }

        // RetryTimeout -> Make LLM request
        (ConvState::LlmRequesting { attempt }, Event::RetryTimeout { attempt: retry_attempt })
            if *attempt == retry_attempt =>
        {
            Ok(TransitionResult::new(ConvState::LlmRequesting { attempt: *attempt })
                .with_effect(Effect::RequestLlm))
        }

        // ============================================================
        // Tool Execution (REQ-BED-004)
        // ============================================================

        // ToolExecuting + ToolComplete (more tools remaining) -> ToolExecuting (next tool)
        (ConvState::ToolExecuting { current_tool_id, remaining_tool_ids, completed_results }, 
         Event::ToolComplete { tool_use_id, result })
            if &tool_use_id == current_tool_id && !remaining_tool_ids.is_empty() =>
        {
            let mut new_results = completed_results.clone();
            new_results.push(result.clone());
            
            let next_tool_id = remaining_tool_ids[0].clone();
            let new_remaining = remaining_tool_ids[1..].to_vec();
            
            // We need tool info from the pending tools - this is stored when we receive the LLM response
            // For now, we just track IDs; the executor will look up the actual tool info
            Ok(TransitionResult::new(ConvState::ToolExecuting {
                current_tool_id: next_tool_id.clone(),
                remaining_tool_ids: new_remaining,
                completed_results: new_results,
            })
            .with_effect(Effect::persist_tool_message(
                tool_result_to_json(&result),
                result.display_data(),
            ))
            .with_effect(Effect::PersistState))
        }

        // ToolExecuting + ToolComplete (last tool) -> AwaitingLlm
        (ConvState::ToolExecuting { current_tool_id, remaining_tool_ids, completed_results },
         Event::ToolComplete { tool_use_id, result })
            if &tool_use_id == current_tool_id && remaining_tool_ids.is_empty() =>
        {
            let mut new_results = completed_results.clone();
            new_results.push(result.clone());
            
            Ok(TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_tool_message(
                    tool_result_to_json(&result),
                    result.display_data(),
                ))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestLlm))
        }

        // ============================================================
        // Cancellation (REQ-BED-005)
        // ============================================================

        // LlmRequesting + UserCancel -> Cancelling
        (ConvState::LlmRequesting { .. }, Event::UserCancel) => {
            Ok(TransitionResult::new(ConvState::Cancelling { pending_tool_id: None })
                .with_effect(Effect::PersistState))
        }

        // Cancelling + LlmResponse -> Idle (ignore response)
        (ConvState::Cancelling { .. }, Event::LlmResponse { .. }) => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // ToolExecuting + UserCancel -> Idle with synthetic results
        (ConvState::ToolExecuting { current_tool_id, remaining_tool_ids, completed_results },
         Event::UserCancel) => {
            // Generate synthetic cancelled result for current tool
            let current_result = ToolResult::cancelled(
                current_tool_id.clone(),
                "Cancelled by user",
            );
            
            // Generate synthetic skipped results for remaining tools
            let skipped_results: Vec<ToolResult> = remaining_tool_ids
                .iter()
                .map(|id| ToolResult::cancelled(id.clone(), "Skipped due to cancellation"))
                .collect();
            
            let mut all_results = completed_results.clone();
            all_results.push(current_result);
            all_results.extend(skipped_results);
            
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistToolResults { results: all_results })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // AwaitingLlm + UserCancel -> Idle
        (ConvState::AwaitingLlm, Event::UserCancel) => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // AwaitingSubAgents + UserCancel -> Idle (sub-agents will be terminated)
        (ConvState::AwaitingSubAgents { .. }, Event::UserCancel) => {
            Ok(TransitionResult::new(ConvState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        // ============================================================
        // Sub-Agent Results (REQ-BED-008)
        // ============================================================

        // AwaitingSubAgents + SubAgentResult (more pending) -> AwaitingSubAgents
        (ConvState::AwaitingSubAgents { pending_ids, completed_results },
         Event::SubAgentResult { agent_id, result })
            if pending_ids.contains(&agent_id) && pending_ids.len() > 1 =>
        {
            let new_pending: Vec<_> = pending_ids.iter()
                .filter(|id| *id != &agent_id)
                .cloned()
                .collect();
            let mut new_results = completed_results.clone();
            new_results.push(result);
            
            Ok(TransitionResult::new(ConvState::AwaitingSubAgents {
                pending_ids: new_pending,
                completed_results: new_results,
            })
            .with_effect(Effect::PersistState))
        }

        // AwaitingSubAgents + SubAgentResult (last one) -> AwaitingLlm
        (ConvState::AwaitingSubAgents { pending_ids, completed_results },
         Event::SubAgentResult { agent_id, result })
            if pending_ids.contains(&agent_id) && pending_ids.len() == 1 =>
        {
            let mut new_results = completed_results.clone();
            new_results.push(result);
            
            // Aggregate results into a message for the LLM
            let aggregated = aggregate_sub_agent_results(&new_results);
            
            Ok(TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_tool_message(aggregated, None))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestLlm))
        }

        // ============================================================
        // Invalid Transitions
        // ============================================================
        
        (state, event) => {
            Err(TransitionError::InvalidTransition(format!(
                "No transition from {:?} with event {:?}",
                state, event
            )))
        }
    }
}

// Helper functions

fn build_user_message_content(text: &str, images: &[crate::state_machine::event::ImageData]) -> Value {
    if images.is_empty() {
        json!({ "text": text })
    } else {
        json!({
            "text": text,
            "images": images.iter().map(|img| json!({
                "data": img.data,
                "media_type": img.media_type
            })).collect::<Vec<_>>()
        })
    }
}

fn extract_tool_uses(content: &[ContentBlock]) -> Vec<(String, String, Value)> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect()
}

fn content_to_json(content: &[ContentBlock]) -> Value {
    serde_json::to_value(content).unwrap_or(Value::Null)
}

fn tool_result_to_json(result: &ToolResult) -> Value {
    json!({
        "tool_use_id": result.tool_use_id,
        "content": result.output,
        "is_error": result.is_error
    })
}

impl ToolResult {
    fn display_data(&self) -> Option<Value> {
        None // Tool results don't have special display data by default
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

fn aggregate_sub_agent_results(results: &[crate::db::SubAgentResult]) -> Value {
    let summaries: Vec<Value> = results
        .iter()
        .map(|r| json!({
            "agent_id": r.agent_id,
            "success": r.success,
            "result": r.result
        }))
        .collect();
    
    json!({
        "sub_agent_results": summaries
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_context() -> ConvContext {
        ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model")
    }

    #[test]
    fn test_idle_to_awaiting_llm() {
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::UserMessage {
                text: "Hello".to_string(),
                images: vec![],
            },
        ).unwrap();

        assert!(matches!(result.new_state, ConvState::AwaitingLlm));
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
            },
        ).unwrap();

        assert!(matches!(result.new_state, ConvState::AwaitingLlm));
    }

    #[test]
    fn test_cancellation_produces_synthetic_results() {
        let result = transition(
            &ConvState::ToolExecuting {
                current_tool_id: "tool-1".to_string(),
                remaining_tool_ids: vec!["tool-2".to_string(), "tool-3".to_string()],
                completed_results: vec![],
            },
            &test_context(),
            Event::UserCancel,
        ).unwrap();

        assert!(matches!(result.new_state, ConvState::Idle));
        // Should have effect to persist synthetic results
        assert!(result.effects.iter().any(|e| matches!(e, Effect::PersistToolResults { .. })));
    }
}
