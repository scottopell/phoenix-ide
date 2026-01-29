//! Property-based tests for the state machine
//!
//! These tests verify key invariants hold across all possible inputs.

use super::state::*;
use super::transition::*;
use super::*;
use crate::db::{ErrorKind, ToolResult};
use crate::llm::{ContentBlock, Usage};
use proptest::prelude::*;
use serde_json::json;
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
        proptest::collection::vec(arb_tool_result(), 0..3),
    )
        .prop_map(|(current_tool, remaining_tools, completed_results)| {
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
            }
        })
}

fn arb_error_state() -> impl Strategy<Value = ConvState> {
    ("[a-zA-Z ]{1,30}", arb_error_kind()).prop_map(|(message, error_kind)| ConvState::Error {
        message,
        error_kind,
    })
}

fn arb_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        arb_idle_state(),
        arb_llm_requesting_state(),
        arb_tool_executing_state(),
        arb_error_state(),
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
        Just(ConvState::Cancelling { pending_tool_id: None }),
    ]
}

fn arb_user_message_event() -> impl Strategy<Value = Event> {
    "[a-zA-Z ]{1,30}".prop_map(|text| Event::UserMessage {
        text,
        images: vec![],
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

fn arb_event() -> impl Strategy<Value = Event> {
    prop_oneof![
        arb_user_message_event(),
        arb_llm_response_event(),
        arb_tool_complete_event(),
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
        };

        let result = transition(&state, &test_context(), event);
        prop_assert!(result.is_ok(), "Error recovery failed: {:?}", result);
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::LlmRequesting { .. }),
            "Should transition to LlmRequesting"
        );
    }

    // Invariant 3: Cancel from any working state reaches Idle or Cancelling
    #[test]
    fn prop_cancel_stops_work(state in arb_working_state()) {
        let result = transition(&state, &test_context(), Event::UserCancel);
        prop_assert!(result.is_ok(), "Cancel failed: {:?}", result);
        let new_state = result.unwrap().new_state;
        prop_assert!(
            matches!(new_state, ConvState::Idle | ConvState::Cancelling { .. }),
            "Should reach Idle or Cancelling, got {:?}",
            new_state
        );
    }

    // Invariant 4: Tool completion with matching ID always succeeds
    #[test]
    fn prop_tool_complete_with_matching_id_succeeds(
        current in arb_tool_call(),
        remaining in proptest::collection::vec(arb_tool_call(), 0..3),
        completed in proptest::collection::vec(arb_tool_result(), 0..3),
        result_output in "[a-zA-Z0-9 ]{0,50}",
        result_success in any::<bool>()
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            completed_results: completed,
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
}

// ============================================================================
// Unit Tests for Edge Cases
// ============================================================================

#[test]
fn test_tool_completion_advances_to_next_tool() {
    let tool1 = ToolCall::new("t1", ToolInput::Bash(BashInput {
        command: "echo 1".to_string(),
        mode: BashMode::Default,
    }));
    let tool2 = ToolCall::new("t2", ToolInput::Bash(BashInput {
        command: "echo 2".to_string(),
        mode: BashMode::Default,
    }));
    
    let state = ConvState::ToolExecuting {
        current_tool: tool1.clone(),
        remaining_tools: vec![tool2.clone()],
        completed_results: vec![],
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
    ).unwrap();
    
    match result.new_state {
        ConvState::ToolExecuting { current_tool, remaining_tools, completed_results } => {
            assert_eq!(current_tool.id, "t2");
            assert!(remaining_tools.is_empty());
            assert_eq!(completed_results.len(), 1);
        }
        _ => panic!("Expected ToolExecuting"),
    }
    
    // Should have ExecuteTool effect for next tool
    assert!(result.effects.iter().any(|e| matches!(e, Effect::ExecuteTool { tool } if tool.id == "t2")));
}

#[test]
fn test_last_tool_completion_goes_to_llm_requesting() {
    let tool1 = ToolCall::new("t1", ToolInput::Bash(BashInput {
        command: "echo 1".to_string(),
        mode: BashMode::Default,
    }));
    
    let state = ConvState::ToolExecuting {
        current_tool: tool1,
        remaining_tools: vec![],
        completed_results: vec![],
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
    ).unwrap();
    
    assert!(matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }));
    assert!(result.effects.iter().any(|e| matches!(e, Effect::RequestLlm)));
}
