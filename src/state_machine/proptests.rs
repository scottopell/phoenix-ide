//! Property-based tests for the state machine
//!
//! These tests verify key invariants hold across all possible inputs.

#![allow(clippy::collapsible_if)]
#![allow(clippy::single_match_else)]

use super::state::*;
use super::transition::*;
use super::*;
use crate::db::{ErrorKind, ToolResult};
use crate::llm::{ContentBlock, Usage};
use proptest::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;

// ============================================================================
// Test Helpers
// ============================================================================

fn test_context() -> ConvContext {
    ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model")
}

// ============================================================================
// Arbitrary Generators
// ============================================================================

fn arb_bash_mode() -> impl Strategy<Value = BashMode> {
    prop_oneof![
        Just(BashMode::Default),
        Just(BashMode::Slow),
        Just(BashMode::Background),
    ]
}

fn arb_bash_input() -> impl Strategy<Value = BashInput> {
    ("[a-z ]{1,20}", arb_bash_mode()).prop_map(|(command, mode)| BashInput { command, mode })
}

fn arb_think_input() -> impl Strategy<Value = ThinkInput> {
    "[a-zA-Z ]{1,50}".prop_map(|thoughts| ThinkInput { thoughts })
}

fn arb_tool_input() -> impl Strategy<Value = ToolInput> {
    prop_oneof![
        arb_bash_input().prop_map(ToolInput::Bash),
        arb_think_input().prop_map(ToolInput::Think),
    ]
}

fn arb_tool_call() -> impl Strategy<Value = ToolCall> {
    ("[a-z]{8}", arb_tool_input()).prop_map(|(id, input)| ToolCall::new(id, input))
}

fn arb_tool_result() -> impl Strategy<Value = ToolResult> {
    ("[a-z]{8}", any::<bool>(), "[a-zA-Z0-9 ]{0,50}").prop_map(|(id, success, output)| ToolResult {
        tool_use_id: id,
        success,
        output,
        is_error: !success,
    })
}

fn arb_error_kind() -> impl Strategy<Value = ErrorKind> {
    prop_oneof![
        Just(ErrorKind::Network),
        Just(ErrorKind::RateLimit),
        Just(ErrorKind::Auth),
        Just(ErrorKind::InvalidRequest),
        Just(ErrorKind::Unknown),
    ]
}

fn arb_idle_state() -> impl Strategy<Value = ConvState> {
    Just(ConvState::Idle)
}

fn arb_llm_requesting_state() -> impl Strategy<Value = ConvState> {
    (1u32..5).prop_map(|attempt| ConvState::LlmRequesting { attempt })
}

fn arb_tool_executing_state() -> impl Strategy<Value = ConvState> {
    (
        arb_tool_call(),
        proptest::collection::vec(arb_tool_call(), 0..3),
        proptest::collection::vec("[a-z]{8}".prop_map(String::from), 0..3),
    )
        .prop_map(|(current_tool, remaining_tools, persisted_ids)| {
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                persisted_tool_ids: persisted_ids.into_iter().collect(),
                pending_sub_agents: vec![],
            }
        })
}

fn arb_error_state() -> impl Strategy<Value = ConvState> {
    ("[a-zA-Z ]{1,30}", arb_error_kind()).prop_map(|(message, error_kind)| ConvState::Error {
        message,
        error_kind,
    })
}

fn arb_cancelling_llm_state() -> impl Strategy<Value = ConvState> {
    Just(ConvState::CancellingLlm)
}

fn arb_cancelling_tool_state() -> impl Strategy<Value = ConvState> {
    (
        "[a-z]{8}",
        proptest::collection::vec(arb_tool_call(), 0..3),
        proptest::collection::vec("[a-z]{8}".prop_map(String::from), 0..3),
    )
        .prop_map(|(tool_use_id, skipped_tools, persisted_ids)| {
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                persisted_tool_ids: persisted_ids.into_iter().collect(),
            }
        })
}

fn arb_awaiting_llm_state() -> impl Strategy<Value = ConvState> {
    Just(ConvState::AwaitingLlm)
}

fn arb_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        arb_idle_state(),
        arb_llm_requesting_state(),
        arb_tool_executing_state(),
        arb_error_state(),
        arb_cancelling_llm_state(),
        arb_cancelling_tool_state(),
        arb_awaiting_llm_state(),
    ]
}

fn arb_working_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        arb_llm_requesting_state(),
        arb_tool_executing_state(),
        Just(ConvState::AwaitingLlm),
    ]
}

fn arb_busy_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        arb_working_state(),
        Just(ConvState::CancellingLlm),
        arb_cancelling_tool_state(),
    ]
}

fn arb_user_message_event() -> impl Strategy<Value = Event> {
    "[a-zA-Z ]{1,30}".prop_map(|text| Event::UserMessage {
        text,
        images: vec![],
        message_id: uuid::Uuid::new_v4().to_string(),
        user_agent: None,
    })
}

fn arb_llm_response_event() -> impl Strategy<Value = Event> {
    proptest::collection::vec(arb_tool_call(), 0..3).prop_map(|tool_calls| Event::LlmResponse {
        content: vec![ContentBlock::text("response")],
        tool_calls,
        end_turn: true,
        usage: Usage::default(),
    })
}

fn arb_tool_complete_event() -> impl Strategy<Value = Event> {
    arb_tool_result().prop_map(|result| Event::ToolComplete {
        tool_use_id: result.tool_use_id.clone(),
        result,
    })
}

fn arb_llm_error_event() -> impl Strategy<Value = Event> {
    ("[a-zA-Z ]{1,30}", arb_error_kind(), 1u32..5).prop_map(|(message, error_kind, attempt)| {
        Event::LlmError {
            message,
            error_kind,
            attempt,
        }
    })
}

fn arb_retry_timeout_event() -> impl Strategy<Value = Event> {
    (1u32..5).prop_map(|attempt| Event::RetryTimeout { attempt })
}

fn arb_event() -> impl Strategy<Value = Event> {
    prop_oneof![
        arb_user_message_event(),
        arb_llm_response_event(),
        arb_tool_complete_event(),
        arb_llm_error_event(),
        arb_retry_timeout_event(),
        Just(Event::UserCancel),
    ]
}

// ============================================================================
// State Validity Checkers
// ============================================================================

fn is_valid_state(state: &ConvState) -> bool {
    match state {
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            ..
        } => {
            // No duplicate tool IDs
            let mut ids: Vec<_> = std::iter::once(&current_tool.id)
                .chain(remaining_tools.iter().map(|t| &t.id))
                .collect();
            let len = ids.len();
            ids.sort();
            ids.dedup();
            ids.len() == len
        }
        ConvState::LlmRequesting { attempt } => *attempt >= 1 && *attempt <= 10,
        _ => true,
    }
}

fn effects_are_valid(effects: &[Effect], new_state: &ConvState) -> bool {
    // Check that ExecuteTool effects only appear in appropriate states
    let has_execute = effects
        .iter()
        .any(|e| matches!(e, Effect::ExecuteTool { .. }));
    let has_request_llm = effects.iter().any(|e| matches!(e, Effect::RequestLlm));

    // ExecuteTool should appear when transitioning to ToolExecuting
    // OR when moving to next tool in ToolExecuting
    if has_execute {
        if !matches!(new_state, ConvState::ToolExecuting { .. }) {
            return false;
        }
    }

    // RequestLlm should only appear when transitioning to LlmRequesting
    if has_request_llm {
        if !matches!(new_state, ConvState::LlmRequesting { .. }) {
            return false;
        }
    }

    true
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    // Invariant 1: Valid state after any transition
    #[test]
    fn prop_transitions_preserve_validity(events in proptest::collection::vec(arb_event(), 0..20)) {
        let mut state = ConvState::Idle;
        let ctx = test_context();

        for event in events {
            match transition(&state, &ctx, event) {
                Ok(result) => {
                    state = result.new_state;
                    prop_assert!(is_valid_state(&state), "Invalid state: {:?}", state);
                    prop_assert!(
                        effects_are_valid(&result.effects, &state),
                        "Invalid effects for state {:?}: {:?}",
                        state,
                        result.effects
                    );
                }
                Err(_) => { /* Invalid transition is OK */ }
            }
        }
    }

    // Invariant 2: Error state is always recoverable
    #[test]
    fn prop_error_always_recoverable(
        message in "[a-zA-Z ]{1,30}",
        kind in arb_error_kind()
    ) {
        let state = ConvState::Error {
            message,
            error_kind: kind,
        };
        let event = Event::UserMessage {
            text: "retry".to_string(),
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok(), "Error recovery failed: {:?}", result);
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::LlmRequesting { .. }),
            "Should transition to LlmRequesting"
        );
    }

    // Invariant 3: Cancel from any working state reaches a cancelling state
    #[test]
    fn prop_cancel_stops_work(state in arb_working_state()) {
        let result = transition(&state, &test_context(), Event::UserCancel);
        prop_assert!(result.is_ok(), "Cancel failed: {:?}", result);
        let new_state = result.unwrap().new_state;
        prop_assert!(
            matches!(
                new_state,
                ConvState::Idle | ConvState::CancellingLlm | ConvState::CancellingTool { .. }
            ),
            "Should reach Idle or a cancelling state, got {:?}",
            new_state
        );
    }

    // Invariant 4: Tool completion with matching ID always succeeds
    #[test]
    fn prop_tool_complete_with_matching_id_succeeds(
        current in arb_tool_call(),
        remaining in proptest::collection::vec(arb_tool_call(), 0..3),
        persisted_ids in proptest::collection::vec("[a-z]{8}".prop_map(String::from), 0..3),
        result_output in "[a-zA-Z0-9 ]{0,50}",
        result_success in any::<bool>()
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            persisted_tool_ids: persisted_ids.into_iter().collect(),
            pending_sub_agents: vec![],
        };
        let event = Event::ToolComplete {
            tool_use_id: current.id.clone(),
            result: ToolResult {
                tool_use_id: current.id,
                success: result_success,
                output: result_output,
                is_error: !result_success,
            },
        };

        let trans_result = transition(&state, &test_context(), event);
        prop_assert!(trans_result.is_ok(), "Tool completion failed: {:?}", trans_result);
    }

    // Invariant 5: Busy states reject user messages
    #[test]
    fn prop_busy_rejects_messages(state in arb_busy_state()) {
        let event = Event::UserMessage {
            text: "hi".to_string(),
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
        };
        let result = transition(&state, &test_context(), event);
        // Busy states either return AgentBusy, CancellationInProgress, or InvalidTransition
        prop_assert!(
            result.is_err(),
            "Busy state should reject messages, got {:?}",
            result
        );
    }

    // Invariant 6: PersistState effect always emitted on state change
    #[test]
    fn prop_state_changes_persist(state in arb_state(), event in arb_event()) {
        if let Ok(result) = transition(&state, &test_context(), event) {
            if result.new_state != state {
                prop_assert!(
                    result.effects.iter().any(|e| matches!(e, Effect::PersistState)),
                    "State changed but no PersistState effect: {:?} -> {:?}",
                    state,
                    result.new_state
                );
            }
        }
    }

    // Invariant 7: Idle state accepts user messages
    #[test]
    fn prop_idle_accepts_messages(text in "[a-zA-Z ]{1,30}") {
        let state = ConvState::Idle;
        let event = Event::UserMessage {
            text,
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok(), "Idle should accept messages: {:?}", result);
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::LlmRequesting { attempt: 1 }),
            "Should transition to LlmRequesting"
        );
    }

    // Invariant 8: LLM response without tools goes to Idle
    #[test]
    fn prop_llm_response_without_tools_goes_idle(attempt in 1u32..5) {
        let state = ConvState::LlmRequesting { attempt };
        let event = Event::LlmResponse {
            content: vec![ContentBlock::text("Hello")],
            tool_calls: vec![],
            end_turn: true,
            usage: Usage::default(),
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::Idle),
            "Should go to Idle when no tools"
        );
    }

    // Invariant 9: LLM response with tools goes to ToolExecuting
    #[test]
    fn prop_llm_response_with_tools_executes(
        attempt in 1u32..5,
        tool_calls in proptest::collection::vec(arb_tool_call(), 1..4)
    ) {
        let state = ConvState::LlmRequesting { attempt };
        let event = Event::LlmResponse {
            content: vec![],
            tool_calls: tool_calls.clone(),
            end_turn: false,
            usage: Usage::default(),
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());

        let new_state = result.unwrap().new_state;
        match new_state {
            ConvState::ToolExecuting { current_tool, remaining_tools, .. } => {
                prop_assert_eq!(&current_tool.id, &tool_calls[0].id);
                prop_assert_eq!(remaining_tools.len(), tool_calls.len() - 1);
            }
            _ => prop_assert!(false, "Should be ToolExecuting, got {:?}", new_state),
        }
    }

    // Invariant 10: Retryable LLM errors increment attempt counter
    #[test]
    fn prop_retryable_error_increments_attempt(
        attempt in 1u32..3,  // Must be < MAX_RETRY_ATTEMPTS (3)
        message in "[a-zA-Z ]{1,30}"
    ) {
        let state = ConvState::LlmRequesting { attempt };
        // Network and RateLimit are retryable
        let error_kind = ErrorKind::Network;
        let event = Event::LlmError {
            message,
            error_kind,
            attempt,
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());

        match result.unwrap().new_state {
            ConvState::LlmRequesting { attempt: new_attempt } => {
                prop_assert_eq!(new_attempt, attempt + 1);
            }
            _ => prop_assert!(false, "Should stay in LlmRequesting with incremented attempt"),
        }
    }

    // Invariant 11: Non-retryable LLM errors go to Error state
    #[test]
    fn prop_non_retryable_error_goes_to_error(
        attempt in 1u32..5,
        message in "[a-zA-Z ]{1,30}"
    ) {
        let state = ConvState::LlmRequesting { attempt };
        // Auth and InvalidRequest are non-retryable
        let error_kind = ErrorKind::Auth;
        let event = Event::LlmError {
            message: message.clone(),
            error_kind: error_kind.clone(),
            attempt,
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());

        match result.unwrap().new_state {
            ConvState::Error { error_kind: ek, .. } => {
                prop_assert_eq!(ek, error_kind);
            }
            s => prop_assert!(false, "Should be Error state, got {:?}", s),
        }
    }

    // Invariant 12: Exhausted retries go to Error state
    #[test]
    fn prop_exhausted_retries_go_to_error(message in "[a-zA-Z ]{1,30}") {
        let state = ConvState::LlmRequesting { attempt: 3 }; // MAX_RETRY_ATTEMPTS
        let event = Event::LlmError {
            message,
            error_kind: ErrorKind::Network, // Retryable but exhausted
            attempt: 3,
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::Error { .. }),
            "Should go to Error after exhausting retries"
        );
    }

    // Invariant 13: RetryTimeout triggers LLM request
    #[test]
    fn prop_retry_timeout_triggers_llm_request(attempt in 1u32..5) {
        let state = ConvState::LlmRequesting { attempt };
        let event = Event::RetryTimeout { attempt };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());

        let tr = result.unwrap();
        prop_assert!(
            matches!(tr.new_state, ConvState::LlmRequesting { .. }),
            "Should stay in LlmRequesting"
        );
        prop_assert!(
            tr.effects.iter().any(|e| matches!(e, Effect::RequestLlm)),
            "Should have RequestLlm effect"
        );
    }

    // Invariant 14: CancellingLlm + LlmResponse/LlmAborted goes to Idle
    #[test]
    fn prop_cancelling_llm_plus_response_goes_idle(_dummy in Just(())) {
        let state = ConvState::CancellingLlm;
        let event = Event::LlmResponse {
            content: vec![ContentBlock::text("response")],
            tool_calls: vec![],
            end_turn: true,
            usage: Usage::default(),
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::Idle),
            "Should go to Idle when response arrives after cancel"
        );
    }

    // Invariant 14b: CancellingLlm + LlmAborted goes to Idle
    #[test]
    fn prop_cancelling_llm_plus_aborted_goes_idle(_dummy in Just(())) {
        let state = ConvState::CancellingLlm;
        let result = transition(&state, &test_context(), Event::LlmAborted);
        prop_assert!(result.is_ok());
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::Idle),
            "Should go to Idle when LLM request is aborted"
        );
    }

    // Invariant 15: LlmRequesting + UserCancel -> CancellingLlm with AbortLlm effect
    #[test]
    fn prop_llm_cancel_goes_to_cancelling(_dummy in Just(())) {
        let state = ConvState::LlmRequesting { attempt: 1 };
        let result = transition(&state, &test_context(), Event::UserCancel);
        prop_assert!(result.is_ok());

        let tr = result.unwrap();
        prop_assert!(
            matches!(tr.new_state, ConvState::CancellingLlm),
            "Should go to CancellingLlm"
        );
        prop_assert!(
            tr.effects.iter().any(|e| matches!(e, Effect::AbortLlm)),
            "Should have AbortLlm effect"
        );
    }

    // Invariant 16: ToolExecuting + UserCancel -> CancellingTool with AbortTool effect
    #[test]
    fn prop_tool_cancel_goes_to_cancelling(
        current in arb_tool_call(),
        remaining in proptest::collection::vec(arb_tool_call(), 0..3),
        persisted_ids in proptest::collection::vec("[a-z]{8}".prop_map(String::from), 0..3)
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining.clone(),
            persisted_tool_ids: persisted_ids.into_iter().collect(),
            pending_sub_agents: vec![],
        };

        let result = transition(&state, &test_context(), Event::UserCancel);
        prop_assert!(result.is_ok());

        let tr = result.unwrap();

        // Should go to CancellingTool
        match &tr.new_state {
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                ..
            } => {
                prop_assert_eq!(tool_use_id, &current.id);
                prop_assert_eq!(skipped_tools.len(), remaining.len());
            }
            s => prop_assert!(false, "Expected CancellingTool, got {:?}", s),
        }

        // Should have AbortTool effect
        prop_assert!(
            tr.effects.iter().any(|e| matches!(e, Effect::AbortTool { tool_use_id } if tool_use_id == &current.id)),
            "Should have AbortTool effect for current tool"
        );
    }

    // Invariant 16: CancellingTool + ToolAborted -> Idle with synthetic results
    // Note: persisted_tool_ids must NOT contain the tool_use_id or any skipped tool IDs
    #[test]
    fn prop_cancelling_tool_aborted_goes_idle(
        tool_use_id in "[a-z]{8}",
        skipped in proptest::collection::vec(arb_tool_call(), 0..3),
        other_persisted in proptest::collection::vec("[A-Z]{8}".prop_map(String::from), 0..3) // Use uppercase to avoid collisions
    ) {
        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped.clone(),
            persisted_tool_ids: other_persisted.into_iter().collect(),
        };

        let result = transition(
            &state,
            &test_context(),
            Event::ToolAborted {
                tool_use_id: tool_use_id.clone(),
            },
        );
        prop_assert!(result.is_ok());

        let tr = result.unwrap();
        prop_assert!(matches!(tr.new_state, ConvState::Idle));

        // Should have PersistToolResults with correct count
        let persist = tr.effects.iter().find(|e| matches!(e, Effect::PersistToolResults { .. }));
        prop_assert!(persist.is_some());

        if let Some(Effect::PersistToolResults { results }) = persist {
            // aborted(1) + skipped (persisted_tool_ids were already persisted separately)
            let expected_len = 1 + skipped.len();
            prop_assert_eq!(results.len(), expected_len);
        }
    }

    // Invariant 17: CancellingTool + ToolComplete (racing) -> Idle with synthetic (not actual) results
    // Note: persisted_tool_ids must NOT contain the tool_use_id or any skipped tool IDs
    #[test]
    fn prop_cancelling_tool_complete_uses_synthetic(
        tool_use_id in "[a-z]{8}",
        skipped in proptest::collection::vec(arb_tool_call(), 0..3),
        other_persisted in proptest::collection::vec("[A-Z]{8}".prop_map(String::from), 0..3) // Use uppercase to avoid collisions
    ) {
        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped.clone(),
            persisted_tool_ids: other_persisted.into_iter().collect(),
        };

        // Tool completes naturally before abort takes effect
        let actual_result = ToolResult {
            tool_use_id: tool_use_id.clone(),
            success: true,
            output: "actual output that should be discarded".to_string(),
            is_error: false,
        };

        let result = transition(
            &state,
            &test_context(),
            Event::ToolComplete {
                tool_use_id: tool_use_id.clone(),
                result: actual_result,
            },
        );
        prop_assert!(result.is_ok());

        let tr = result.unwrap();
        prop_assert!(matches!(tr.new_state, ConvState::Idle));

        // Should still use synthetic results (all failed), not the actual success
        if let Some(Effect::PersistToolResults { results }) = tr.effects.iter().find(|e| matches!(e, Effect::PersistToolResults { .. })) {
            // Find the result for our tool - it should be marked as cancelled, not successful
            let our_result = results.iter().find(|r| r.tool_use_id == tool_use_id);
            prop_assert!(our_result.is_some());
            prop_assert!(!our_result.unwrap().success, "Cancelled tool should not show as successful");
        }
    }

    // Invariant 18: Tool completion with wrong ID is invalid
    #[test]
    fn prop_tool_complete_wrong_id_fails(
        current in arb_tool_call(),
        remaining in proptest::collection::vec(arb_tool_call(), 0..3),
        persisted_ids in proptest::collection::vec("[a-z]{8}".prop_map(String::from), 0..3)
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            persisted_tool_ids: persisted_ids.into_iter().collect(),
            pending_sub_agents: vec![],
        };
        let event = Event::ToolComplete {
            tool_use_id: "wrong-id".to_string(),
            result: ToolResult {
                tool_use_id: "wrong-id".to_string(),
                success: true,
                output: "output".to_string(),
                is_error: false,
            },
        };

        let result = transition(&state, &test_context(), event);
        // Should fail because tool_use_id doesn't match current_tool.id
        prop_assert!(
            result.is_err(),
            "Should reject tool completion with wrong ID"
        );
    }
}

// ============================================================================
// Sequence Tests - Multi-Step Scenarios
// ============================================================================

/// Test a complete user message -> LLM response -> tool execution -> completion cycle
#[test]
fn test_complete_tool_cycle() {
    let ctx = test_context();
    let mut state = ConvState::Idle;

    // Step 1: User sends message
    let result = transition(
        &state,
        &ctx,
        Event::UserMessage {
            text: "run ls".to_string(),
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::LlmRequesting { attempt: 1 }));
    assert!(result.effects.iter().any(|e| matches!(e, Effect::RequestLlm)));

    // Step 2: LLM responds with tool call
    let tool = ToolCall::new(
        "tool-123",
        ToolInput::Bash(BashInput {
            command: "ls".to_string(),
            mode: BashMode::Default,
        }),
    );
    let result = transition(
        &state,
        &ctx,
        Event::LlmResponse {
            content: vec![ContentBlock::text("I'll run ls")],
            tool_calls: vec![tool.clone()],
            end_turn: false,
            usage: Usage::default(),
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::ToolExecuting { .. }));
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::ExecuteTool { .. })));

    // Step 3: Tool completes
    let result = transition(
        &state,
        &ctx,
        Event::ToolComplete {
            tool_use_id: "tool-123".to_string(),
            result: ToolResult {
                tool_use_id: "tool-123".to_string(),
                success: true,
                output: "file1 file2".to_string(),
                is_error: false,
            },
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::LlmRequesting { attempt: 1 }));
    assert!(result.effects.iter().any(|e| matches!(e, Effect::RequestLlm)));

    // Step 4: LLM responds with text only
    let result = transition(
        &state,
        &ctx,
        Event::LlmResponse {
            content: vec![ContentBlock::text("Found file1 and file2")],
            tool_calls: vec![],
            end_turn: true,
            usage: Usage::default(),
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::Idle));
}

/// Test retry cycle: error -> retry -> success
#[test]
fn test_retry_cycle() {
    let ctx = test_context();
    let mut state = ConvState::LlmRequesting { attempt: 1 };

    // First attempt fails with network error (retryable)
    let result = transition(
        &state,
        &ctx,
        Event::LlmError {
            message: "connection reset".to_string(),
            error_kind: ErrorKind::Network,
            attempt: 1,
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::LlmRequesting { attempt: 2 }));
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::ScheduleRetry { .. })));

    // Retry timeout fires
    let result = transition(&state, &ctx, Event::RetryTimeout { attempt: 2 }).unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::LlmRequesting { attempt: 2 }));
    assert!(result.effects.iter().any(|e| matches!(e, Effect::RequestLlm)));

    // Second attempt succeeds
    let result = transition(
        &state,
        &ctx,
        Event::LlmResponse {
            content: vec![ContentBlock::text("Success!")],
            tool_calls: vec![],
            end_turn: true,
            usage: Usage::default(),
        },
    )
    .unwrap();
    assert!(matches!(result.new_state, ConvState::Idle));
}

/// Test multiple tool execution chain
#[test]
fn test_multi_tool_chain() {
    let ctx = test_context();

    let tool1 = ToolCall::new(
        "t1",
        ToolInput::Bash(BashInput {
            command: "echo 1".to_string(),
            mode: BashMode::Default,
        }),
    );
    let tool2 = ToolCall::new(
        "t2",
        ToolInput::Bash(BashInput {
            command: "echo 2".to_string(),
            mode: BashMode::Default,
        }),
    );
    let tool3 = ToolCall::new(
        "t3",
        ToolInput::Bash(BashInput {
            command: "echo 3".to_string(),
            mode: BashMode::Default,
        }),
    );

    // LLM responds with 3 tools
    let mut state = ConvState::LlmRequesting { attempt: 1 };
    let result = transition(
        &state,
        &ctx,
        Event::LlmResponse {
            content: vec![],
            tool_calls: vec![tool1.clone(), tool2.clone(), tool3.clone()],
            end_turn: false,
            usage: Usage::default(),
        },
    )
    .unwrap();
    state = result.new_state;

    // Should be executing tool1 with tool2, tool3 remaining
    match &state {
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            ..
        } => {
            assert_eq!(current_tool.id, "t1");
            assert_eq!(remaining_tools.len(), 2);
        }
        _ => panic!("Expected ToolExecuting"),
    }

    // Complete tool1
    let result = transition(
        &state,
        &ctx,
        Event::ToolComplete {
            tool_use_id: "t1".to_string(),
            result: ToolResult {
                tool_use_id: "t1".to_string(),
                success: true,
                output: "1".to_string(),
                is_error: false,
            },
        },
    )
    .unwrap();
    state = result.new_state;

    // Should now be executing tool2
    match &state {
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            persisted_tool_ids,
            ..
        } => {
            assert_eq!(current_tool.id, "t2");
            assert_eq!(remaining_tools.len(), 1);
            assert_eq!(persisted_tool_ids.len(), 1);
        }
        _ => panic!("Expected ToolExecuting"),
    }

    // Complete tool2
    let result = transition(
        &state,
        &ctx,
        Event::ToolComplete {
            tool_use_id: "t2".to_string(),
            result: ToolResult {
                tool_use_id: "t2".to_string(),
                success: true,
                output: "2".to_string(),
                is_error: false,
            },
        },
    )
    .unwrap();
    state = result.new_state;

    // Should now be executing tool3 (last one)
    match &state {
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            persisted_tool_ids,
            ..
        } => {
            assert_eq!(current_tool.id, "t3");
            assert!(remaining_tools.is_empty());
            assert_eq!(persisted_tool_ids.len(), 2);
        }
        _ => panic!("Expected ToolExecuting"),
    }

    // Complete tool3
    let result = transition(
        &state,
        &ctx,
        Event::ToolComplete {
            tool_use_id: "t3".to_string(),
            result: ToolResult {
                tool_use_id: "t3".to_string(),
                success: true,
                output: "3".to_string(),
                is_error: false,
            },
        },
    )
    .unwrap();

    // Should go back to LlmRequesting
    assert!(matches!(
        result.new_state,
        ConvState::LlmRequesting { attempt: 1 }
    ));
}

/// Test cancellation mid-tool-chain generates synthetic results for all remaining
#[test]
fn test_cancel_mid_tool_chain() {
    let ctx = test_context();

    // t1 already completed and was persisted
    let mut persisted = HashSet::new();
    persisted.insert("t1".to_string());

    let state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            "t2",
            ToolInput::Bash(BashInput {
                command: "sleep 10".to_string(),
                mode: BashMode::Default,
            }),
        ),
        remaining_tools: vec![
            ToolCall::new(
                "t3",
                ToolInput::Bash(BashInput {
                    command: "echo 3".to_string(),
                    mode: BashMode::Default,
                }),
            ),
            ToolCall::new(
                "t4",
                ToolInput::Bash(BashInput {
                    command: "echo 4".to_string(),
                    mode: BashMode::Default,
                }),
            ),
        ],
        persisted_tool_ids: persisted,
        pending_sub_agents: vec![],
    };

    // Phase 1: UserCancel -> CancellingTool + AbortTool effect
    let result = transition(&state, &ctx, Event::UserCancel).unwrap();

    assert!(
        matches!(result.new_state, ConvState::CancellingTool { .. }),
        "Should transition to CancellingTool, got {:?}",
        result.new_state
    );

    // Should have AbortTool effect
    let abort_effect = result
        .effects
        .iter()
        .find(|e| matches!(e, Effect::AbortTool { .. }));
    assert!(abort_effect.is_some(), "Should have AbortTool effect");

    // Phase 2: ToolAborted -> Idle with synthetic results
    let result2 = transition(
        &result.new_state,
        &ctx,
        Event::ToolAborted {
            tool_use_id: "t2".to_string(),
        },
    )
    .unwrap();

    assert!(matches!(result2.new_state, ConvState::Idle));

    // Should have PersistToolResults with synthetic results
    let persist_effect = result2
        .effects
        .iter()
        .find(|e| matches!(e, Effect::PersistToolResults { .. }));
    assert!(persist_effect.is_some(), "Should have PersistToolResults");

    if let Some(Effect::PersistToolResults { results }) = persist_effect {
        // Should have results for aborted (t2) + skipped (t3, t4) = 3 total
        // Note: completed (t1) was already persisted via PersistMessage
        assert_eq!(results.len(), 3, "Should have 3 results (aborted + skipped)");
        // All should be cancelled/skipped (no success)
        assert!(results.iter().all(|r| !r.success));
    }
}

// ============================================================================
// Unit Tests for Edge Cases
// ============================================================================

#[test]
fn test_tool_completion_advances_to_next_tool() {
    let tool1 = ToolCall::new(
        "t1",
        ToolInput::Bash(BashInput {
            command: "echo 1".to_string(),
            mode: BashMode::Default,
        }),
    );
    let tool2 = ToolCall::new(
        "t2",
        ToolInput::Bash(BashInput {
            command: "echo 2".to_string(),
            mode: BashMode::Default,
        }),
    );

    let state = ConvState::ToolExecuting {
        current_tool: tool1.clone(),
        remaining_tools: vec![tool2.clone()],
        persisted_tool_ids: HashSet::new(),
        pending_sub_agents: vec![],
    };

    let result = transition(
        &state,
        &test_context(),
        Event::ToolComplete {
            tool_use_id: "t1".to_string(),
            result: ToolResult {
                tool_use_id: "t1".to_string(),
                success: true,
                output: "1".to_string(),
                is_error: false,
            },
        },
    )
    .unwrap();

    match result.new_state {
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            persisted_tool_ids,
            ..
        } => {
            assert_eq!(current_tool.id, "t2");
            assert!(remaining_tools.is_empty());
            assert_eq!(persisted_tool_ids.len(), 1);
        }
        _ => panic!("Expected ToolExecuting"),
    }

    // Should have ExecuteTool effect for next tool
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::ExecuteTool { tool } if tool.id == "t2")));
}

#[test]
fn test_last_tool_completion_goes_to_llm_requesting() {
    let tool1 = ToolCall::new(
        "t1",
        ToolInput::Bash(BashInput {
            command: "echo 1".to_string(),
            mode: BashMode::Default,
        }),
    );

    let state = ConvState::ToolExecuting {
        current_tool: tool1,
        remaining_tools: vec![],
        persisted_tool_ids: HashSet::new(),
        pending_sub_agents: vec![],
    };

    let result = transition(
        &state,
        &test_context(),
        Event::ToolComplete {
            tool_use_id: "t1".to_string(),
            result: ToolResult {
                tool_use_id: "t1".to_string(),
                success: true,
                output: "done".to_string(),
                is_error: false,
            },
        },
    )
    .unwrap();

    assert!(matches!(
        result.new_state,
        ConvState::LlmRequesting { attempt: 1 }
    ));
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::RequestLlm)));
}

// ============================================================================
// Sub-Agent Property Tests
// ============================================================================

use super::state::SubAgentOutcome;

/// Generator for sub-agent outcome
fn arb_sub_agent_outcome() -> impl Strategy<Value = SubAgentOutcome> {
    prop_oneof![
        "[a-zA-Z ]{1,50}".prop_map(|result| SubAgentOutcome::Success { result }),
        ("[a-zA-Z ]{1,30}", arb_error_kind()).prop_map(|(error, error_kind)| {
            SubAgentOutcome::Failure { error, error_kind }
        }),
    ]
}

/// Fan-in conservation: pending + completed = constant
proptest! {
    #[test]
    fn prop_subagent_count_conserved(
        initial_ids in proptest::collection::vec("[a-z]{8}", 1..5),
    ) {
    let n = initial_ids.len();
    let mut state = ConvState::AwaitingSubAgents {
        pending_ids: initial_ids.clone(),
        completed_results: vec![],
    };

    for agent_id in initial_ids {
        let event = Event::SubAgentResult {
            agent_id: agent_id.clone(),
            outcome: SubAgentOutcome::Success {
                result: "done".to_string(),
            },
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok());
        state = result.unwrap().new_state;

        // Check conservation at each step
        match &state {
            ConvState::AwaitingSubAgents {
                pending_ids,
                completed_results,
            } => {
                prop_assert_eq!(pending_ids.len() + completed_results.len(), n);
            }
            ConvState::LlmRequesting { .. } => {
                // Terminal - all collected
            }
            other => {
                let msg = format!("Unexpected state: {other:?}");
                prop_assert!(false, "{}", msg);
            }
        }
    }
    }
}

/// Pending IDs decrease monotonically
proptest! {
    #[test]
    fn prop_pending_decreases_monotonically(
        initial_ids in proptest::collection::vec("[a-z]{8}", 2..5),
    ) {
    let mut state = ConvState::AwaitingSubAgents {
        pending_ids: initial_ids.clone(),
        completed_results: vec![],
    };
    let mut prev_pending = initial_ids.len();

    for agent_id in initial_ids {
        let event = Event::SubAgentResult {
            agent_id,
            outcome: SubAgentOutcome::Success {
                result: "done".to_string(),
            },
        };

        state = transition(&state, &test_context(), event).unwrap().new_state;

        if let ConvState::AwaitingSubAgents { pending_ids, .. } = &state {
            prop_assert!(pending_ids.len() < prev_pending);
            prev_pending = pending_ids.len();
        }
    }
    }
}

/// Unknown agent_id is rejected
proptest! {
    #[test]
    fn prop_unknown_agent_rejected(
        pending_ids in proptest::collection::vec("[a-z]{8}", 1..3),
        unknown_id in "[A-Z]{8}", // Different pattern to ensure no overlap
    ) {
    let state = ConvState::AwaitingSubAgents {
        pending_ids,
        completed_results: vec![],
    };

    let event = Event::SubAgentResult {
        agent_id: unknown_id,
        outcome: SubAgentOutcome::Success {
            result: "done".to_string(),
        },
    };

    let result = transition(&state, &test_context(), event);
    prop_assert!(result.is_err());
    }
}

/// Last completion exits AwaitingSubAgents
proptest! {
    #[test]
    fn prop_last_completion_exits_awaiting(
        agent_ids in proptest::collection::vec("[a-z]{8}", 1..4),
        outcome in arb_sub_agent_outcome(),
    ) {
    let mut state = ConvState::AwaitingSubAgents {
        pending_ids: agent_ids.clone(),
        completed_results: vec![],
    };

    for (i, agent_id) in agent_ids.iter().enumerate() {
        let event = Event::SubAgentResult {
            agent_id: agent_id.clone(),
            outcome: outcome.clone(),
        };

        let result = transition(&state, &test_context(), event).unwrap();

        if i < agent_ids.len() - 1 {
            let is_awaiting = matches!(result.new_state, ConvState::AwaitingSubAgents { .. });
            prop_assert!(is_awaiting);
        } else {
            // Last one should exit to LlmRequesting
            let is_llm_requesting = matches!(result.new_state, ConvState::LlmRequesting { .. });
            prop_assert!(is_llm_requesting);
        }
        state = result.new_state;
    }
    }
}

/// AwaitingSubAgents + UserCancel -> CancellingSubAgents
proptest! {
    #[test]
    fn prop_awaiting_cancel_goes_to_cancelling(
        pending_ids in proptest::collection::vec("[a-z]{8}", 1..4),
    ) {
    let state = ConvState::AwaitingSubAgents {
        pending_ids: pending_ids.clone(),
        completed_results: vec![],
    };

    let result = transition(&state, &test_context(), Event::UserCancel).unwrap();

    match result.new_state {
        ConvState::CancellingSubAgents {
            pending_ids: new_pending,
            ..
        } => {
            prop_assert_eq!(new_pending, pending_ids);
        }
        _ => prop_assert!(false, "Expected CancellingSubAgents"),
    }

    // Should have CancelSubAgents effect
    let has_cancel_effect = result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::CancelSubAgents { .. }));
    prop_assert!(has_cancel_effect);
    }
}

/// CancellingSubAgents collects results until all done
proptest! {
    #[test]
    fn prop_cancelling_collects_until_done(
        initial_ids in proptest::collection::vec("[a-z]{8}", 2..4),
    ) {
    let mut state = ConvState::CancellingSubAgents {
        pending_ids: initial_ids.clone(),
        completed_results: vec![],
    };

    for (i, agent_id) in initial_ids.iter().enumerate() {
        let event = Event::SubAgentResult {
            agent_id: agent_id.clone(),
            outcome: SubAgentOutcome::Failure {
                error: "Cancelled".to_string(),
                error_kind: ErrorKind::Cancelled,
            },
        };

        let result = transition(&state, &test_context(), event).unwrap();

        if i < initial_ids.len() - 1 {
            let is_cancelling = matches!(result.new_state, ConvState::CancellingSubAgents { .. });
            prop_assert!(is_cancelling);
        } else {
            // Last one goes to Idle
            let is_idle = matches!(result.new_state, ConvState::Idle);
            prop_assert!(is_idle);
        }
        state = result.new_state;
    }
    }
}

/// ToolExecuting with pending_sub_agents goes to AwaitingSubAgents on last tool
#[test]
fn test_tool_complete_with_pending_agents_goes_to_awaiting() {
    let state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            "t1",
            ToolInput::Bash(BashInput {
                command: "echo".to_string(),
                mode: BashMode::Default,
            }),
        ),
        remaining_tools: vec![],
        persisted_tool_ids: HashSet::new(),
        pending_sub_agents: vec!["agent-1".to_string(), "agent-2".to_string()],
    };

    let event = Event::ToolComplete {
        tool_use_id: "t1".to_string(),
        result: ToolResult {
            tool_use_id: "t1".to_string(),
            success: true,
            output: "done".to_string(),
            is_error: false,
        },
    };

    let result = transition(&state, &test_context(), event).unwrap();

    match result.new_state {
        ConvState::AwaitingSubAgents { pending_ids, .. } => {
            assert_eq!(pending_ids, vec!["agent-1", "agent-2"]);
        }
        _ => panic!("Expected AwaitingSubAgents, got {:?}", result.new_state),
    }
}

/// SpawnAgentsComplete accumulates agent IDs
#[test]
fn test_spawn_agents_complete_accumulates_ids() {
    let state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            "spawn-1",
            ToolInput::Unknown {
                name: "spawn_agents".to_string(),
                input: serde_json::json!({}),
            },
        ),
        remaining_tools: vec![ToolCall::new(
            "t2",
            ToolInput::Bash(BashInput {
                command: "echo".to_string(),
                mode: BashMode::Default,
            }),
        )],
        persisted_tool_ids: HashSet::new(),
        pending_sub_agents: vec!["existing-agent".to_string()],
    };

    let event = Event::SpawnAgentsComplete {
        tool_use_id: "spawn-1".to_string(),
        result: ToolResult {
            tool_use_id: "spawn-1".to_string(),
            success: true,
            output: "Spawned 2 agents".to_string(),
            is_error: false,
        },
        agent_ids: vec!["new-agent-1".to_string(), "new-agent-2".to_string()],
    };

    let result = transition(&state, &test_context(), event).unwrap();

    match result.new_state {
        ConvState::ToolExecuting {
            pending_sub_agents,
            current_tool,
            ..
        } => {
            assert_eq!(current_tool.id, "t2");
            assert_eq!(
                pending_sub_agents,
                vec!["existing-agent", "new-agent-1", "new-agent-2"]
            );
        }
        _ => panic!("Expected ToolExecuting, got {:?}", result.new_state),
    }
}

// ============================================================================
// Invariant: No Duplicate Tool Persists
// ============================================================================

/// Property: Cancellation should fail if it would produce duplicate persists
/// This tests that our validation logic correctly catches the bug scenario.
proptest! {
    #[test]
    fn prop_duplicate_persist_detected(
        tool_use_id in "[a-z]{8}",
        skipped in proptest::collection::vec(arb_tool_call(), 0..3)
    ) {
        // Create a state where tool_use_id is ALREADY in persisted_tool_ids
        // This simulates the bug scenario where we would try to persist it again
        let mut persisted = HashSet::new();
        persisted.insert(tool_use_id.clone());
        
        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped,
            persisted_tool_ids: persisted,
        };

        // This should fail because tool_use_id is already persisted
        let result = transition(
            &state,
            &test_context(),
            Event::ToolAborted {
                tool_use_id: tool_use_id.clone(),
            },
        );

        prop_assert!(
            result.is_err(),
            "Should fail when tool_use_id is already in persisted_tool_ids"
        );
    }

    /// Property: Cancellation should succeed when no duplicates would occur
    #[test]
    fn prop_no_duplicate_persist_succeeds(
        tool_use_id in "[a-z]{8}",
        skipped in proptest::collection::vec(arb_tool_call(), 0..3),
        other_persisted in proptest::collection::vec("[A-Z]{8}".prop_map(String::from), 0..3)
    ) {
        // Ensure tool_use_id is NOT in persisted_tool_ids (use uppercase for others)
        let persisted: HashSet<String> = other_persisted.into_iter().collect();
        
        // Also ensure skipped tool IDs don't collide with persisted
        let skipped_filtered: Vec<_> = skipped.into_iter()
            .filter(|t| !persisted.contains(&t.id))
            .collect();
        
        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped_filtered,
            persisted_tool_ids: persisted,
        };

        let result = transition(
            &state,
            &test_context(),
            Event::ToolAborted {
                tool_use_id: tool_use_id.clone(),
            },
        );

        prop_assert!(
            result.is_ok(),
            "Should succeed when no duplicate persists would occur: {:?}",
            result
        );
    }
}
