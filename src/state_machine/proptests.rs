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
use std::path::PathBuf;

// ============================================================================
// Test Helpers
// ============================================================================

pub(crate) fn test_context() -> ConvContext {
    ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000)
}

/// Build an `AssistantMessage` with `ToolUse` content blocks for the given tool IDs.
/// Used in tests that construct `ToolExecuting`/`CancellingTool` states directly,
/// where `CheckpointData::tool_round()` enforces `tool_use` count == `tool_result` count.
fn assistant_message_for_tools(tool_ids: &[&str]) -> AssistantMessage {
    let content_blocks: Vec<ContentBlock> = tool_ids
        .iter()
        .map(|id| ContentBlock::ToolUse {
            id: (*id).to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({}),
        })
        .collect();
    AssistantMessage::new(content_blocks, None, None)
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
        display_data: None,
        images: vec![],
    })
}

pub(crate) fn arb_error_kind() -> impl Strategy<Value = ErrorKind> {
    prop_oneof![
        Just(ErrorKind::Network),
        Just(ErrorKind::RateLimit),
        Just(ErrorKind::ServerError),
        Just(ErrorKind::Auth),
        Just(ErrorKind::InvalidRequest),
        Just(ErrorKind::ContentFilter),
        Just(ErrorKind::ContextExhausted),
        Just(ErrorKind::TimedOut),
        Just(ErrorKind::Cancelled),
        Just(ErrorKind::SubAgentError),
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
    )
        .prop_map(|(current_tool, remaining_tools)| {
            // AssistantMessage must have ToolUse blocks for all tools
            // so CheckpointData::tool_round() count invariant holds
            let mut content_blocks: Vec<ContentBlock> = vec![ContentBlock::ToolUse {
                id: current_tool.id.clone(),
                name: current_tool.name().to_string(),
                input: serde_json::json!({}),
            }];
            for t in &remaining_tools {
                content_blocks.push(ContentBlock::ToolUse {
                    id: t.id.clone(),
                    name: t.name().to_string(),
                    input: serde_json::json!({}),
                });
            }
            ConvState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message: AssistantMessage::new(content_blocks, None, None),
            }
        })
}

fn arb_error_state() -> impl Strategy<Value = ConvState> {
    ("[a-zA-Z ]{1,30}", arb_error_kind()).prop_map(|(message, error_kind)| ConvState::Error {
        message,
        error_kind,
    })
}

fn arb_cancelling_tool_state() -> impl Strategy<Value = ConvState> {
    ("[a-z]{8}", proptest::collection::vec(arb_tool_call(), 0..3)).prop_map(
        |(tool_use_id, skipped_tools)| {
            // AssistantMessage must have ToolUse blocks for the aborted tool + skipped tools
            let mut content_blocks: Vec<ContentBlock> = vec![ContentBlock::ToolUse {
                id: tool_use_id.clone(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            }];
            for t in &skipped_tools {
                content_blocks.push(ContentBlock::ToolUse {
                    id: t.id.clone(),
                    name: t.name().to_string(),
                    input: serde_json::json!({}),
                });
            }
            ConvState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results: vec![],
                assistant_message: AssistantMessage::new(content_blocks, None, None),
                pending_sub_agents: vec![],
            }
        },
    )
}

fn arb_awaiting_continuation_state() -> impl Strategy<Value = ConvState> {
    (proptest::collection::vec(arb_tool_call(), 0..3), 1u32..5).prop_map(
        |(rejected_tool_calls, attempt)| ConvState::AwaitingContinuation {
            rejected_tool_calls,
            attempt,
        },
    )
}

fn arb_context_exhausted_state() -> impl Strategy<Value = ConvState> {
    // Include empty, normal, and long summaries with various characters
    prop_oneof![
        Just(String::new()),
        "[a-zA-Z0-9 .,!?\n]{1,100}",
        "[\x00-\x7F]{1,500}", // ASCII including control chars
    ]
    .prop_map(|summary| ConvState::ContextExhausted { summary })
}

fn arb_terminal_state() -> impl Strategy<Value = ConvState> {
    Just(ConvState::Terminal)
}

fn arb_awaiting_task_approval_state() -> impl Strategy<Value = ConvState> {
    (
        "[a-zA-Z ]{1,30}",
        prop_oneof![
            Just("p0".to_string()),
            Just("p1".to_string()),
            Just("p2".to_string())
        ],
        "[a-zA-Z0-9 .,\n]{1,100}",
    )
        .prop_map(|(title, priority, plan)| ConvState::AwaitingTaskApproval {
            title,
            priority,
            plan,
        })
}

fn arb_user_question() -> impl Strategy<Value = state::UserQuestion> {
    (
        "[a-zA-Z ]{1,30}",
        "[a-zA-Z ]{1,20}",
        proptest::collection::vec(
            "[a-zA-Z ]{1,20}".prop_map(|label| state::QuestionOption {
                label,
                description: None,
                preview: None,
            }),
            2..=4,
        ),
    )
        .prop_map(|(question, header, options)| state::UserQuestion {
            question,
            header,
            options,
            multi_select: false,
        })
}

fn arb_awaiting_user_response_state() -> impl Strategy<Value = ConvState> {
    (
        proptest::collection::vec(arb_user_question(), 1..=4),
        "[a-z]{8}",
    )
        .prop_map(|(questions, tool_use_id)| ConvState::AwaitingUserResponse {
            questions,
            tool_use_id,
        })
}

fn arb_awaiting_recovery_state() -> impl Strategy<Value = ConvState> {
    ("[a-zA-Z ]{1,30}", arb_error_kind()).prop_map(|(message, error_kind)| {
        ConvState::AwaitingRecovery {
            message,
            error_kind,
            recovery_kind: super::state::RecoveryKind::Credential,
        }
    })
}

pub(crate) fn arb_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![
        arb_idle_state(),
        arb_llm_requesting_state(),
        arb_tool_executing_state(),
        arb_error_state(),
        arb_cancelling_tool_state(),
        arb_awaiting_continuation_state(),
        arb_context_exhausted_state(),
        arb_awaiting_task_approval_state(),
        arb_awaiting_user_response_state(),
        arb_terminal_state(),
        arb_awaiting_recovery_state(),
    ]
}

fn arb_working_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![arb_llm_requesting_state(), arb_tool_executing_state(),]
}

fn arb_busy_state() -> impl Strategy<Value = ConvState> {
    prop_oneof![arb_working_state(), arb_cancelling_tool_state(),]
}

fn arb_user_message_event() -> impl Strategy<Value = Event> {
    "[a-zA-Z ]{1,30}".prop_map(|text| Event::UserMessage {
        text,
        llm_text: None,
        images: vec![],
        message_id: uuid::Uuid::new_v4().to_string(),
        user_agent: None,
        skill_invocation: None,
    })
}

fn arb_llm_response_event() -> impl Strategy<Value = Event> {
    proptest::collection::vec(arb_tool_call(), 0..3).prop_map(|tool_calls| {
        // Content must include ToolUse blocks matching tool_calls,
        // since CheckpointData::tool_round() enforces matching counts.
        let mut content = vec![ContentBlock::text("response")];
        for tc in &tool_calls {
            content.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name().to_string(),
                input: serde_json::json!({}),
            });
        }
        Event::LlmResponse {
            content,
            tool_calls,
            end_turn: true,
            usage: Usage::default(),
        }
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
            recovery_in_progress: false,
        }
    })
}

fn arb_retry_timeout_event() -> impl Strategy<Value = Event> {
    (1u32..5).prop_map(|attempt| Event::RetryTimeout { attempt })
}

fn arb_task_approval_outcome() -> impl Strategy<Value = TaskApprovalOutcome> {
    prop_oneof![
        Just(TaskApprovalOutcome::Approved),
        Just(TaskApprovalOutcome::Rejected),
        "[a-zA-Z ]{1,30}"
            .prop_map(|annotations| TaskApprovalOutcome::FeedbackProvided { annotations }),
    ]
}

fn arb_user_question_response_event() -> impl Strategy<Value = Event> {
    proptest::collection::vec(("[a-zA-Z ]{1,20}", "[a-zA-Z ]{1,20}"), 1..=4).prop_map(|pairs| {
        let answers = pairs
            .into_iter()
            .collect::<std::collections::HashMap<String, String>>();
        Event::UserQuestionResponse {
            answers,
            annotations: None,
        }
    })
}

fn arb_task_approval_event() -> impl Strategy<Value = Event> {
    arb_task_approval_outcome().prop_map(|outcome| Event::TaskApprovalResponse { outcome })
}

fn arb_grace_turn_exhausted_event() -> impl Strategy<Value = Event> {
    proptest::option::of("[a-zA-Z0-9 ]{0,100}")
        .prop_map(|result| Event::GraceTurnExhausted { result })
}

pub(crate) fn arb_event() -> impl Strategy<Value = Event> {
    prop_oneof![
        arb_user_message_event(),
        arb_llm_response_event(),
        arb_tool_complete_event(),
        arb_llm_error_event(),
        arb_retry_timeout_event(),
        Just(Event::UserCancel { reason: None }),
        arb_task_approval_event(),
        arb_user_question_response_event(),
        arb_grace_turn_exhausted_event(),
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
            llm_text: None,
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
            skill_invocation: None,
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
        let result = transition(&state, &test_context(), Event::UserCancel { reason: None });
        prop_assert!(result.is_ok(), "Cancel failed: {:?}", result);
        let new_state = result.unwrap().new_state;
        prop_assert!(
            matches!(
                new_state,
                ConvState::Idle | ConvState::CancellingTool { .. }
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
        result_output in "[a-zA-Z0-9 ]{0,50}",
        result_success in any::<bool>()
    ) {
        // Build AssistantMessage with ToolUse blocks for all tools
        let all_ids: Vec<String> = std::iter::once(current.id.clone())
            .chain(remaining.iter().map(|t| t.id.clone()))
            .collect();
        let all_id_refs: Vec<&str> = all_ids.iter().map(String::as_str).collect();
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: assistant_message_for_tools(&all_id_refs),
        };
        let event = Event::ToolComplete {
            tool_use_id: current.id.clone(),
            result: ToolResult {
                tool_use_id: current.id,
                success: result_success,
                output: result_output,
                is_error: !result_success,
                display_data: None,
                images: vec![],
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
            llm_text: None,
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
            skill_invocation: None,
        };
        let result = transition(&state, &test_context(), event);
        // Busy states either return AgentBusy, CancellationInProgress, or InvalidTransition
        prop_assert!(
            result.is_err(),
            "Busy state should reject messages, got {:?}",
            result
        );
    }

    // Invariant 5b: ContextExhausted rejects user messages (REQ-BED-021)
    #[test]
    fn prop_context_exhausted_rejects_messages(
        state in arb_context_exhausted_state(),
        text in "[a-zA-Z ]{1,30}"
    ) {
        let event = Event::UserMessage {
            text,
            llm_text: None,
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
            skill_invocation: None,
        };
        let result = transition(&state, &test_context(), event);
        prop_assert!(
            matches!(result, Err(TransitionError::ContextExhausted)),
            "ContextExhausted should reject messages with ContextExhausted error, got {:?}",
            result
        );
    }

    // Invariant 5c: ContextExhausted is stable (ignores non-message events)
    #[test]
    fn prop_context_exhausted_stable(
        summary in "[a-zA-Z0-9 ]{0,50}",
        event in arb_event().prop_filter("not UserMessage", |e| !matches!(e, Event::UserMessage { .. }))
    ) {
        let state = ConvState::ContextExhausted { summary: summary.clone() };
        let result = transition(&state, &test_context(), event);
        prop_assert!(
            result.is_ok(),
            "ContextExhausted should accept non-message events, got {:?}",
            result
        );
        let new_state = result.unwrap().new_state;
        prop_assert!(
            matches!(&new_state, ConvState::ContextExhausted { summary: s } if *s == summary),
            "ContextExhausted should remain unchanged, got {:?}",
            new_state
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
            llm_text: None,
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
            skill_invocation: None,
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
        // Content must include ToolUse blocks matching tool_calls
        let content: Vec<ContentBlock> = tool_calls.iter().map(|tc| ContentBlock::ToolUse {
            id: tc.id.clone(),
            name: tc.name().to_string(),
            input: serde_json::json!({}),
        }).collect();
        let event = Event::LlmResponse {
            content,
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
            recovery_in_progress: false,
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
            recovery_in_progress: false,
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
            recovery_in_progress: false,
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

    // Invariant 15: LlmRequesting + UserCancel -> Idle with AbortLlm effect
    #[test]
    fn prop_llm_cancel_goes_to_idle(_dummy in Just(())) {
        let state = ConvState::LlmRequesting { attempt: 1 };
        let result = transition(&state, &test_context(), Event::UserCancel { reason: None });
        prop_assert!(result.is_ok());

        let tr = result.unwrap();
        prop_assert!(
            matches!(tr.new_state, ConvState::Idle),
            "Should go to Idle"
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
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining.clone(),
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        };

        let result = transition(&state, &test_context(), Event::UserCancel { reason: None });
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

    // Invariant 16: CancellingTool + ToolAborted -> Idle with atomic checkpoint
    #[test]
    fn prop_cancelling_tool_aborted_goes_idle(
        tool_use_id in "[a-z]{8}",
        skipped in proptest::collection::vec(arb_tool_call(), 0..3),
    ) {
        // Build an AssistantMessage with tool_use blocks for all tools:
        // the aborted tool + all skipped tools (+ no completed_results in this test)
        let mut content_blocks: Vec<ContentBlock> = vec![
            ContentBlock::ToolUse {
                id: tool_use_id.clone(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
        ];
        for tool in &skipped {
            content_blocks.push(ContentBlock::ToolUse {
                id: tool.id.clone(),
                name: tool.name().to_string(),
                input: serde_json::json!({}),
            });
        }
        let assistant_message = AssistantMessage::new(content_blocks, None, None);

        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped.clone(),
            completed_results: vec![],
            assistant_message,
            pending_sub_agents: vec![],
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

        // Should have PersistCheckpoint with correct count
        let persist = tr.effects.iter().find(|e| matches!(e, Effect::PersistCheckpoint { .. }));
        prop_assert!(persist.is_some());

        if let Some(Effect::PersistCheckpoint { data }) = persist {
            let CheckpointData::ToolRound { tool_results, .. } = data;
            // aborted(1) + skipped (completed_results is empty in this test)
            let expected_len = 1 + skipped.len();
            prop_assert_eq!(tool_results.len(), expected_len);
        }
    }

    // Invariant 17: CancellingTool + ToolComplete (racing) -> Idle with synthetic (not actual) results
    #[test]
    fn prop_cancelling_tool_complete_uses_synthetic(
        tool_use_id in "[a-z]{8}",
        skipped in proptest::collection::vec(arb_tool_call(), 0..3),
    ) {
        // Build an AssistantMessage with tool_use blocks for all tools
        let mut content_blocks: Vec<ContentBlock> = vec![
            ContentBlock::ToolUse {
                id: tool_use_id.clone(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
        ];
        for tool in &skipped {
            content_blocks.push(ContentBlock::ToolUse {
                id: tool.id.clone(),
                name: tool.name().to_string(),
                input: serde_json::json!({}),
            });
        }
        let assistant_message = AssistantMessage::new(content_blocks, None, None);

        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: skipped.clone(),
            completed_results: vec![],
            assistant_message,
            pending_sub_agents: vec![],
        };

        // Tool completes naturally before abort takes effect
        let actual_result = ToolResult {
            tool_use_id: tool_use_id.clone(),
            success: true,
            output: "actual output that should be discarded".to_string(),
            is_error: false,
            display_data: None,
            images: vec![],
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
        if let Some(Effect::PersistCheckpoint { data }) = tr.effects.iter().find(|e| matches!(e, Effect::PersistCheckpoint { .. })) {
            let CheckpointData::ToolRound { tool_results, .. } = data;
            // Find the result for our tool - it should be marked as cancelled, not successful
            let our_result = tool_results.iter().find(|r| r.tool_use_id == tool_use_id);
            prop_assert!(our_result.is_some());
            prop_assert!(!our_result.unwrap().success, "Cancelled tool should not show as successful");
        }
    }

    // Invariant 18: Tool completion with wrong ID is invalid
    #[test]
    fn prop_tool_complete_wrong_id_fails(
        current in arb_tool_call(),
        remaining in proptest::collection::vec(arb_tool_call(), 0..3),
    ) {
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        };
        let event = Event::ToolComplete {
            tool_use_id: "wrong-id".to_string(),
            result: ToolResult {
                tool_use_id: "wrong-id".to_string(),
                success: true,
                output: "output".to_string(),
                is_error: false,
                display_data: None,
                images: vec![],
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
            llm_text: None,
            images: vec![],
            message_id: uuid::Uuid::new_v4().to_string(),
            user_agent: None,
            skill_invocation: None,
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::LlmRequesting { attempt: 1 }));
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::RequestLlm)));

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
            content: vec![
                ContentBlock::text("I'll run ls"),
                ContentBlock::ToolUse {
                    id: "tool-123".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ],
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
                display_data: None,
                images: vec![],
            },
        },
    )
    .unwrap();
    state = result.new_state;
    assert!(matches!(state, ConvState::LlmRequesting { attempt: 1 }));
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::RequestLlm)));

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
            recovery_in_progress: false,
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
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, Effect::RequestLlm)));

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
#[allow(clippy::too_many_lines)]
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
            content: vec![
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "t2".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "t3".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                },
            ],
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
                display_data: None,
                images: vec![],
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
            completed_results,
            ..
        } => {
            assert_eq!(current_tool.id, "t2");
            assert_eq!(remaining_tools.len(), 1);
            assert_eq!(completed_results.len(), 1);
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
                display_data: None,
                images: vec![],
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
            completed_results,
            ..
        } => {
            assert_eq!(current_tool.id, "t3");
            assert!(remaining_tools.is_empty());
            assert_eq!(completed_results.len(), 2);
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
                display_data: None,
                images: vec![],
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
#[allow(clippy::too_many_lines)]
fn test_cancel_mid_tool_chain() {
    let ctx = test_context();

    // t1 already completed (stored in completed_results)
    let t1_result = ToolResult {
        tool_use_id: "t1".to_string(),
        success: true,
        output: "1".to_string(),
        is_error: false,
        display_data: None,
        images: vec![],
    };

    // Build AssistantMessage with tool_use blocks for all 4 tools
    let assistant_message = AssistantMessage::new(
        vec![
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolUse {
                id: "t2".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolUse {
                id: "t3".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolUse {
                id: "t4".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
        ],
        None,
        None,
    );

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
        completed_results: vec![t1_result],
        pending_sub_agents: vec![],
        assistant_message,
    };

    // Phase 1: UserCancel -> CancellingTool + AbortTool effect
    let result = transition(&state, &ctx, Event::UserCancel { reason: None }).unwrap();

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

    // Phase 2: ToolAborted -> Idle with atomic checkpoint
    let result2 = transition(
        &result.new_state,
        &ctx,
        Event::ToolAborted {
            tool_use_id: "t2".to_string(),
        },
    )
    .unwrap();

    assert!(matches!(result2.new_state, ConvState::Idle));

    // Should have PersistCheckpoint with all results
    let persist_effect = result2
        .effects
        .iter()
        .find(|e| matches!(e, Effect::PersistCheckpoint { .. }));
    assert!(persist_effect.is_some(), "Should have PersistCheckpoint");

    if let Some(Effect::PersistCheckpoint { data }) = persist_effect {
        let CheckpointData::ToolRound { tool_results, .. } = data;
        // Should have results for completed (t1) + aborted (t2) + skipped (t3, t4) = 4 total
        assert_eq!(
            tool_results.len(),
            4,
            "Should have 4 results (completed + aborted + skipped)"
        );
        // t1 should be successful, t2/t3/t4 should be cancelled/skipped
        let t1 = tool_results.iter().find(|r| r.tool_use_id == "t1").unwrap();
        assert!(t1.success, "t1 should be successful (was completed)");
        assert!(
            tool_results
                .iter()
                .filter(|r| r.tool_use_id != "t1")
                .all(|r| !r.success),
            "t2/t3/t4 should be cancelled/skipped"
        );
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
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: assistant_message_for_tools(&["t1", "t2"]),
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
                display_data: None,
                images: vec![],
            },
        },
    )
    .unwrap();

    match result.new_state {
        ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            completed_results,
            ..
        } => {
            assert_eq!(current_tool.id, "t2");
            assert!(remaining_tools.is_empty());
            assert_eq!(completed_results.len(), 1);
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
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: assistant_message_for_tools(&["t1"]),
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
                display_data: None,
                images: vec![],
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
        ("[a-zA-Z ]{1,30}", arb_error_kind())
            .prop_map(|(error, error_kind)| { SubAgentOutcome::Failure { error, error_kind } }),
        Just(SubAgentOutcome::TimedOut),
    ]
}

/// Helper to create `PendingSubAgent` from id string
fn pending_agent(id: &str) -> PendingSubAgent {
    PendingSubAgent {
        agent_id: id.to_string(),
        task: format!("Task for {id}"),
        mode: SubAgentMode::Explore,
    }
}

// Fan-in conservation: pending + completed = constant
proptest! {
    #[test]
    fn prop_subagent_count_conserved(
        initial_ids in proptest::collection::vec("[a-z]{8}", 1..5),
    ) {
    let n = initial_ids.len();
    let initial_pending: Vec<PendingSubAgent> = initial_ids.iter().map(|id| pending_agent(id)).collect();
    let mut state = ConvState::AwaitingSubAgents {
        pending: initial_pending,
        completed_results: vec![],
        spawn_tool_id: None,
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
                pending,
                completed_results,
                ..
            } => {
                prop_assert_eq!(pending.len() + completed_results.len(), n);
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

// Pending IDs decrease monotonically
proptest! {
    #[test]
    fn prop_pending_decreases_monotonically(
        initial_ids in proptest::collection::vec("[a-z]{8}", 2..5),
    ) {
    let initial_pending: Vec<PendingSubAgent> = initial_ids.iter().map(|id| pending_agent(id)).collect();
    let mut state = ConvState::AwaitingSubAgents {
        pending: initial_pending,
        completed_results: vec![],
        spawn_tool_id: None,
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

        if let ConvState::AwaitingSubAgents { pending, .. } = &state {
            prop_assert!(pending.len() < prev_pending);
            prev_pending = pending.len();
        }
    }
    }
}

// Unknown agent_id is rejected
proptest! {
    #[test]
    fn prop_unknown_agent_rejected(
        pending_ids in proptest::collection::vec("[a-z]{8}", 1..3),
        unknown_id in "[A-Z]{8}", // Different pattern to ensure no overlap
    ) {
    let pending: Vec<PendingSubAgent> = pending_ids.iter().map(|id| pending_agent(id)).collect();
    let state = ConvState::AwaitingSubAgents {
        pending,
        completed_results: vec![],
        spawn_tool_id: None,
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

// Last completion exits AwaitingSubAgents
proptest! {
    #[test]
    fn prop_last_completion_exits_awaiting(
        agent_ids in proptest::collection::vec("[a-z]{8}", 1..4),
        outcome in arb_sub_agent_outcome(),
    ) {
    let pending: Vec<PendingSubAgent> = agent_ids.iter().map(|id| pending_agent(id)).collect();
    let mut state = ConvState::AwaitingSubAgents {
        pending,
        completed_results: vec![],
        spawn_tool_id: None,
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

// AwaitingSubAgents + UserCancel -> CancellingSubAgents
proptest! {
    #[test]
    fn prop_awaiting_cancel_goes_to_cancelling(
        pending_ids in proptest::collection::vec("[a-z]{8}", 1..4),
    ) {
    let pending: Vec<PendingSubAgent> = pending_ids.iter().map(|id| pending_agent(id)).collect();
    let state = ConvState::AwaitingSubAgents {
        pending: pending.clone(),
        completed_results: vec![],
        spawn_tool_id: None,
    };

    let result = transition(&state, &test_context(), Event::UserCancel { reason: None }).unwrap();

    match result.new_state {
        ConvState::CancellingSubAgents {
            pending: new_pending,
            ..
        } => {
            prop_assert_eq!(new_pending, pending);
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

// CancellingSubAgents collects results until all done
proptest! {
    #[test]
    fn prop_cancelling_collects_until_done(
        initial_ids in proptest::collection::vec("[a-z]{8}", 2..4),
    ) {
    let pending: Vec<PendingSubAgent> = initial_ids.iter().map(|id| pending_agent(id)).collect();
    let mut state = ConvState::CancellingSubAgents {
        pending,
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

/// `ToolExecuting` with `pending_sub_agents` goes to `AwaitingSubAgents` on last tool
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
        completed_results: vec![],
        pending_sub_agents: vec![
            PendingSubAgent {
                agent_id: "agent-1".to_string(),
                task: "Task 1".to_string(),
                mode: SubAgentMode::Explore,
            },
            PendingSubAgent {
                agent_id: "agent-2".to_string(),
                task: "Task 2".to_string(),
                mode: SubAgentMode::Explore,
            },
        ],
        assistant_message: assistant_message_for_tools(&["t1"]),
    };

    let event = Event::ToolComplete {
        tool_use_id: "t1".to_string(),
        result: ToolResult {
            tool_use_id: "t1".to_string(),
            success: true,
            output: "done".to_string(),
            is_error: false,
            display_data: None,
            images: vec![],
        },
    };

    let result = transition(&state, &test_context(), event).unwrap();

    match result.new_state {
        ConvState::AwaitingSubAgents { pending, .. } => {
            assert_eq!(pending.len(), 2);
            assert_eq!(pending[0].agent_id, "agent-1");
            assert_eq!(pending[1].agent_id, "agent-2");
        }
        _ => panic!("Expected AwaitingSubAgents, got {:?}", result.new_state),
    }
}

/// `SpawnAgentsComplete` accumulates agent IDs
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
        completed_results: vec![],
        pending_sub_agents: vec![PendingSubAgent {
            agent_id: "existing-agent".to_string(),
            task: "Existing task".to_string(),
            mode: SubAgentMode::Explore,
        }],
        assistant_message: AssistantMessage::default(),
    };

    let event = Event::SpawnAgentsComplete {
        tool_use_id: "spawn-1".to_string(),
        result: ToolResult {
            tool_use_id: "spawn-1".to_string(),
            success: true,
            output: "Spawned 2 agents".to_string(),
            is_error: false,
            display_data: None,
            images: vec![],
        },
        spawned: vec![
            PendingSubAgent {
                agent_id: "new-agent-1".to_string(),
                task: "New task 1".to_string(),
                mode: SubAgentMode::Explore,
            },
            PendingSubAgent {
                agent_id: "new-agent-2".to_string(),
                task: "New task 2".to_string(),
                mode: SubAgentMode::Explore,
            },
        ],
    };

    let result = transition(&state, &test_context(), event).unwrap();

    match result.new_state {
        ConvState::ToolExecuting {
            pending_sub_agents,
            current_tool,
            ..
        } => {
            assert_eq!(current_tool.id, "t2");
            assert_eq!(pending_sub_agents.len(), 3);
            assert_eq!(pending_sub_agents[0].agent_id, "existing-agent");
            assert_eq!(pending_sub_agents[1].agent_id, "new-agent-1");
            assert_eq!(pending_sub_agents[2].agent_id, "new-agent-2");
        }
        _ => panic!("Expected ToolExecuting, got {:?}", result.new_state),
    }
}

// Note: The old "No Duplicate Tool Persists" tests were removed because
// the structural invariant is now enforced by CheckpointData::tool_round()
// which requires tool_use count == tool_result count at construction time.

// ============================================================================
// Outcome Generators (for handle_outcome tests)
// ============================================================================

use super::outcome::{AbortReason, EffectOutcome, LlmOutcome, PersistOutcome, ToolOutcome};

fn arb_abort_reason() -> impl Strategy<Value = AbortReason> {
    prop_oneof![
        Just(AbortReason::CancellationRequested),
        Just(AbortReason::Timeout),
        Just(AbortReason::ParentCancelled),
    ]
}

fn arb_llm_outcome() -> impl Strategy<Value = LlmOutcome> {
    // Use (0..8u8) selector + string to avoid Clone requirement on LlmOutcome
    (
        0..8u8,
        proptest::collection::vec(arb_tool_call(), 0..3),
        "[a-zA-Z ]{1,20}",
    )
        .prop_map(|(variant, tool_calls, msg)| match variant {
            0 => {
                let mut content = vec![ContentBlock::text("response")];
                for tc in &tool_calls {
                    content.push(ContentBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name().to_string(),
                        input: serde_json::json!({}),
                    });
                }
                LlmOutcome::Response {
                    content,
                    tool_calls,
                    end_turn: true,
                    usage: Usage::default(),
                }
            }
            1 => LlmOutcome::RateLimited { retry_after: None },
            2 => LlmOutcome::ServerError {
                status: 500,
                body: msg,
            },
            3 => LlmOutcome::NetworkError { message: msg },
            4 => LlmOutcome::TokenBudgetExceeded,
            5 => LlmOutcome::AuthError {
                message: msg,
                recovery_in_progress: false,
            },
            6 => LlmOutcome::RequestRejected { message: msg },
            _ => LlmOutcome::Cancelled,
        })
}

fn arb_tool_outcome() -> impl Strategy<Value = ToolOutcome> {
    prop_oneof![
        arb_tool_result().prop_map(ToolOutcome::Completed),
        ("[a-z]{8}", arb_abort_reason()).prop_map(|(tool_use_id, reason)| ToolOutcome::Aborted {
            tool_use_id,
            reason,
        }),
        ("[a-z]{8}", "[a-zA-Z ]{1,20}")
            .prop_map(|(tool_use_id, error)| ToolOutcome::Failed { tool_use_id, error }),
    ]
}

fn arb_persist_outcome() -> impl Strategy<Value = PersistOutcome> {
    // Use bool selector to avoid Clone requirement on PersistOutcome
    (any::<bool>(), "[a-zA-Z ]{1,20}").prop_map(|(ok, error)| {
        if ok {
            PersistOutcome::Ok
        } else {
            PersistOutcome::Failed { error }
        }
    })
}

fn arb_effect_outcome() -> impl Strategy<Value = EffectOutcome> {
    // Use selector to avoid Clone requirement on EffectOutcome/LlmOutcome/PersistOutcome
    (
        0..5u8,
        arb_llm_outcome(),
        arb_tool_outcome(),
        "[a-z]{8}",
        arb_sub_agent_outcome(),
        arb_persist_outcome(),
        1u32..5,
    )
        .prop_map(
            |(variant, llm, tool, agent_id, sub_outcome, persist, attempt)| match variant {
                0 => EffectOutcome::Llm(llm),
                1 => EffectOutcome::Tool(tool),
                2 => EffectOutcome::SubAgent {
                    agent_id,
                    outcome: sub_outcome,
                },
                3 => EffectOutcome::Persist(persist),
                _ => EffectOutcome::RetryTimeout { attempt },
            },
        )
}

// ============================================================================
// handle_outcome Property Tests
// ============================================================================

use super::transition::handle_outcome;

proptest! {

    // handle_outcome never panics for any (state, outcome) pair
    #[test]
    fn prop_handle_outcome_never_panics(
        state in arb_state(),
        outcome in arb_effect_outcome()
    ) {
        let ctx = test_context();
        // Should return Ok or Err(InvalidOutcome), never panic
        let _ = handle_outcome(&state, &ctx, outcome);
    }


    // PersistOutcome::Ok returns the same state unchanged
    #[test]
    fn prop_persist_ok_returns_same_state(state in arb_state()) {
        let ctx = test_context();
        let outcome = EffectOutcome::Persist(PersistOutcome::Ok);
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        prop_assert_eq!(
            format!("{:?}", result.unwrap().new_state),
            format!("{:?}", state),
            "PersistOutcome::Ok should return unchanged state"
        );
    }

    // PersistOutcome::Failed always returns InvalidOutcome
    #[test]
    fn prop_persist_failed_returns_invalid(
        state in arb_state(),
        error in "[a-zA-Z ]{1,20}"
    ) {
        let ctx = test_context();
        let outcome = EffectOutcome::Persist(PersistOutcome::Failed { error });
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_err());
    }

    // LlmOutcome::AuthError always produces non-retryable error (goes to Error state)
    #[test]
    fn prop_auth_error_is_non_retryable(
        attempt in 1u32..5,
        message in "[a-zA-Z ]{1,20}"
    ) {
        let ctx = test_context();
        let state = ConvState::LlmRequesting { attempt };
        let outcome = EffectOutcome::Llm(LlmOutcome::AuthError { message, recovery_in_progress: false });
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        match result.unwrap().new_state {
            ConvState::Error { error_kind, .. } => {
                prop_assert_eq!(error_kind, ErrorKind::Auth);
            }
            s => prop_assert!(false, "AuthError should go to Error state, got {:?}", s),
        }
    }

    // LlmOutcome::RequestRejected always produces non-retryable error
    #[test]
    fn prop_request_rejected_is_non_retryable(
        attempt in 1u32..5,
        message in "[a-zA-Z ]{1,20}"
    ) {
        let ctx = test_context();
        let state = ConvState::LlmRequesting { attempt };
        let outcome = EffectOutcome::Llm(LlmOutcome::RequestRejected { message });
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        match result.unwrap().new_state {
            ConvState::Error { error_kind, .. } => {
                prop_assert_eq!(error_kind, ErrorKind::InvalidRequest);
            }
            s => prop_assert!(false, "RequestRejected should go to Error state, got {:?}", s),
        }
    }

    // LlmOutcome::NetworkError is retryable (increments attempt, not Error)
    #[test]
    fn prop_network_error_is_retryable(
        attempt in 1u32..3, // < MAX_RETRY_ATTEMPTS
        message in "[a-zA-Z ]{1,20}"
    ) {
        let ctx = test_context();
        let state = ConvState::LlmRequesting { attempt };
        let outcome = EffectOutcome::Llm(LlmOutcome::NetworkError { message });
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        match result.unwrap().new_state {
            ConvState::LlmRequesting { attempt: new_attempt } => {
                prop_assert_eq!(new_attempt, attempt + 1);
            }
            s => prop_assert!(false, "NetworkError should retry, got {:?}", s),
        }
    }

    // LlmOutcome::RateLimited is retryable
    #[test]
    fn prop_rate_limited_is_retryable(attempt in 1u32..3) {
        let ctx = test_context();
        let state = ConvState::LlmRequesting { attempt };
        let outcome = EffectOutcome::Llm(LlmOutcome::RateLimited { retry_after: None });
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        match result.unwrap().new_state {
            ConvState::LlmRequesting { attempt: new_attempt } => {
                prop_assert_eq!(new_attempt, attempt + 1);
            }
            s => prop_assert!(false, "RateLimited should retry, got {:?}", s),
        }
    }

    // ToolOutcome::Completed with matching ID succeeds in ToolExecuting
    #[test]
    fn prop_tool_outcome_completed_succeeds(
        current in arb_tool_call(),
        remaining in proptest::collection::vec(arb_tool_call(), 0..3),
    ) {
        let ctx = test_context();
        let all_ids: Vec<String> = std::iter::once(current.id.clone())
            .chain(remaining.iter().map(|t| t.id.clone()))
            .collect();
        let all_id_refs: Vec<&str> = all_ids.iter().map(String::as_str).collect();
        let state = ConvState::ToolExecuting {
            current_tool: current.clone(),
            remaining_tools: remaining,
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: assistant_message_for_tools(&all_id_refs),
        };
        let tool_result = ToolResult {
            tool_use_id: current.id.clone(),
            success: true,
            output: "done".to_string(),
            is_error: false,
            display_data: None,
            images: vec![],
        };
        let outcome = EffectOutcome::Tool(ToolOutcome::Completed(tool_result));
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok(), "ToolOutcome::Completed should succeed: {:?}", result);
    }

    // ToolOutcome::Aborted produces ToolAborted event
    #[test]
    fn prop_tool_outcome_aborted_in_cancelling(
        tool_use_id in "[a-z]{8}",
        reason in arb_abort_reason(),
    ) {
        let ctx = test_context();
        let content_blocks = vec![ContentBlock::ToolUse {
            id: tool_use_id.clone(),
            name: "bash".to_string(),
            input: serde_json::json!({}),
        }];
        let state = ConvState::CancellingTool {
            tool_use_id: tool_use_id.clone(),
            skipped_tools: vec![],
            completed_results: vec![],
            assistant_message: AssistantMessage::new(content_blocks, None, None),
            pending_sub_agents: vec![],
        };
        let outcome = EffectOutcome::Tool(ToolOutcome::Aborted {
            tool_use_id,
            reason,
        });
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        prop_assert!(
            matches!(result.unwrap().new_state, ConvState::Idle),
            "Aborted tool in CancellingTool should go to Idle"
        );
    }

    // RetryTimeout outcome in LlmRequesting produces RequestLlm effect
    #[test]
    fn prop_retry_timeout_outcome_triggers_llm(attempt in 1u32..5) {
        let ctx = test_context();
        let state = ConvState::LlmRequesting { attempt };
        let outcome = EffectOutcome::RetryTimeout { attempt };
        let result = handle_outcome(&state, &ctx, outcome);
        prop_assert!(result.is_ok());
        let tr = result.unwrap();
        prop_assert!(
            tr.effects.iter().any(|e| matches!(e, Effect::RequestLlm)),
            "RetryTimeout should produce RequestLlm effect"
        );
    }

    // ToolOutcome in Idle is invalid (no active tool to complete)
    #[test]
    fn prop_tool_outcome_in_idle_is_invalid(outcome in arb_tool_outcome()) {
        let ctx = test_context();
        let state = ConvState::Idle;
        let result = handle_outcome(&state, &ctx, EffectOutcome::Tool(outcome));
        prop_assert!(result.is_err(), "Tool outcome in Idle should be invalid");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    // handle_outcome never panics with large random outcome sequences.
    // 32 cases suffice — the sequence structure is what matters, not volume.
    #[test]
    fn prop_handle_outcome_sequence_never_panics(
        outcomes in proptest::collection::vec(arb_effect_outcome(), 0..10)
    ) {
        let ctx = test_context();
        let mut state = ConvState::Idle;

        if let Ok(result) = transition(
            &state,
            &ctx,
            Event::UserMessage {
                text: "test".to_string(),
                llm_text: None,
                images: vec![],
                message_id: "test-msg".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        ) {
            state = result.new_state;
        }

        for outcome in outcomes {
            match handle_outcome(&state, &ctx, outcome) {
                Ok(result) => {
                    state = result.new_state;
                    prop_assert!(is_valid_state(&state));
                }
                Err(_) => { /* InvalidOutcome is fine — state unchanged */ }
            }
        }
    }
}
