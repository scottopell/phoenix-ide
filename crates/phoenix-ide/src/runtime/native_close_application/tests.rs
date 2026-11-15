use super::*;
use crate::state_machine::state::{AssistantMessage, ToolCall, ToolInput};
use serde_json::Value;

fn fixture(input: Value) -> (ConvState, SourceToolCall) {
    let source = SourceToolCall {
        message_id: "assistant".into(),
        tool_use_id: "close-call".into(),
    };
    let state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &source.tool_use_id,
            ToolInput::Unknown {
                name: "close_conversation".into(),
                input: input.clone(),
            },
        ),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: AssistantMessage::new(
            source.message_id.clone(),
            vec![ContentBlock::ToolUse {
                id: source.tool_use_id.clone(),
                name: "close_conversation".into(),
                input,
            }],
            None,
            None,
        ),
    };
    (state, source)
}

fn initial_input() -> Value {
    json!({"action":"initial", "product_conversation_id":"target"})
}

fn derive(
    state: &ConvState,
    source: &SourceToolCall,
) -> Result<NativeCloseCommand, NativeCloseSourceRefusal> {
    initial_command_from_executing_source(
        state,
        "source-transcript",
        source,
        ProductConversationId::parse("target").unwrap(),
    )
}

#[test]
fn exact_executing_occurrence_derives_stable_correlation_only() {
    let (state, source) = fixture(initial_input());
    let first = derive(&state, &source).unwrap();
    assert_eq!(first, derive(&state, &source).unwrap());
    assert_eq!(first.source_transcript_id(), "source-transcript");
    assert_eq!(first.source(), &source);
}

#[test]
fn idle_source_cannot_replay_even_with_same_historical_ids() {
    let (_, source) = fixture(initial_input());
    assert_eq!(
        derive(&ConvState::Idle, &source),
        Err(NativeCloseSourceRefusal::NotExactExecutingSource)
    );
}

#[test]
fn source_message_and_current_call_must_both_match() {
    let (state, source) = fixture(initial_input());
    for changed in [
        SourceToolCall {
            message_id: "other".into(),
            ..source.clone()
        },
        SourceToolCall {
            tool_use_id: "other".into(),
            ..source.clone()
        },
        SourceToolCall {
            message_id: String::new(),
            ..source.clone()
        },
    ] {
        assert_eq!(
            derive(&state, &changed),
            Err(NativeCloseSourceRefusal::NotExactExecutingSource)
        );
    }
}

#[test]
fn canonical_and_executing_payload_must_agree_exactly() {
    let (mut state, source) = fixture(initial_input());
    let ConvState::ToolExecuting { current_tool, .. } = &mut state else {
        unreachable!()
    };
    current_tool.input = ToolInput::Unknown {
        name: "close_conversation".into(),
        input: json!({"action":"initial", "product_conversation_id":"different-target"}),
    };
    assert_eq!(
        derive(&state, &source),
        Err(NativeCloseSourceRefusal::NotExactExecutingSource)
    );
}

#[test]
fn duplicate_canonical_or_pending_occurrence_is_rejected() {
    let (mut state, source) = fixture(initial_input());
    let ConvState::ToolExecuting {
        assistant_message, ..
    } = &mut state
    else {
        unreachable!()
    };
    assistant_message
        .content
        .push(assistant_message.content[0].clone());
    assert!(derive(&state, &source).is_err());

    let (mut state, source) = fixture(initial_input());
    let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        ..
    } = &mut state
    else {
        unreachable!()
    };
    remaining_tools.push(current_tool.clone());
    assert!(derive(&state, &source).is_err());
}

#[test]
fn unsupported_or_authority_bearing_fields_never_become_initial() {
    for input in [
        json!({"action":"observe", "product_conversation_id":"target"}),
        json!({"action":"explicit_retry", "product_conversation_id":"target"}),
        json!({"action":"initial", "product_conversation_id":"target", "cancelled_predecessor":null}),
        json!({"action":"initial", "product_conversation_id":"target", "cancelled_predecessor":{"attempt_id":"old", "run_ordinal":1}}),
        json!({"action":"initial", "product_conversation_id":"target", "request_id":"forged"}),
        json!({"action":"initial", "product_conversation_id":"different-target"}),
    ] {
        let (state, source) = fixture(input);
        assert_eq!(
            derive(&state, &source),
            Err(NativeCloseSourceRefusal::NotExactExecutingSource)
        );
    }
}

fn observed_reference() -> NativeCloseRunRef {
    NativeCloseRunRef {
        target: ProductConversationId::parse("target").unwrap(),
        attempt_id: phoenix_core::domain::close::CloseAttemptId::parse("retained-attempt").unwrap(),
        run_ordinal: phoenix_core::domain::close::CloseRunOrdinal::parse(2).unwrap(),
    }
}

fn observe_input() -> Value {
    NativeCloseIntent::Observe {
        target: observed_reference().target,
        attempt_id: observed_reference().attempt_id,
        run_ordinal: observed_reference().run_ordinal,
    }.source_input()
}

fn derive_observe(state: &ConvState, source: &SourceToolCall)
    -> Result<NativeCloseCommand, NativeCloseSourceRefusal>
{
    observe_command_from_executing_source(state, "source-transcript", source, observed_reference())
}

#[test]
fn exact_observe_source_binds_the_retained_run_and_rejects_idle_or_initial_sources() {
    let (state, source) = fixture(observe_input());
    let command = derive_observe(&state, &source).unwrap();
    assert_eq!(command, derive_observe(&state, &source).unwrap());
    assert_eq!(command.intent().source_input(), observe_input());
    assert!(derive(&state, &source).is_err());
    assert!(derive_observe(&ConvState::Idle, &source).is_err());
    assert!(derive_observe(&fixture(initial_input()).0, &source).is_err());
    for changed in [
        SourceToolCall { message_id: "other".into(), ..source.clone() },
        SourceToolCall { tool_use_id: "other".into(), ..source.clone() },
    ] {
        assert!(derive_observe(&state, &changed).is_err());
    }
    for changed in [
        NativeCloseRunRef { target: ProductConversationId::parse("other").unwrap(), ..observed_reference() },
        NativeCloseRunRef {
            attempt_id: phoenix_core::domain::close::CloseAttemptId::parse("other").unwrap(),
            ..observed_reference()
        },
        NativeCloseRunRef {
            run_ordinal: phoenix_core::domain::close::CloseRunOrdinal::INITIAL,
            ..observed_reference()
        },
    ] {
        assert!(observe_command_from_executing_source(&state, "source-transcript", &source, changed).is_err());
    }
}

#[test]
fn observe_source_rejects_nonexact_payloads_even_when_canonical_and_executing_agree() {
    for (field, value) in [
        ("action", json!("initial")),
        ("action", json!("explicit_retry")),
        ("attempt_id", json!("other-attempt")),
        ("run_ordinal", json!(1)),
        ("run_ordinal", json!("2")),
        ("request_id", json!("caller-authority")),
        ("cancelled_predecessor", json!("predecessor")),
    ] {
        let mut input = observe_input();
        input[field] = value;
        let (state, source) = fixture(input);
        assert!(derive_observe(&state, &source).is_err(), "{field}");
    }
    let (mut state, source) = fixture(observe_input());
    let ConvState::ToolExecuting { current_tool, .. } = &mut state else { unreachable!() };
    current_tool.input = ToolInput::Unknown {
        name: "close_conversation".into(), input: initial_input(),
    };
    assert!(derive_observe(&state, &source).is_err());
}

#[test]
fn observe_source_rejects_duplicate_canonical_and_pending_occurrences() {
    let (mut state, source) = fixture(observe_input());
    let ConvState::ToolExecuting { assistant_message, .. } = &mut state else { unreachable!() };
    assistant_message.content.push(assistant_message.content[0].clone());
    assert!(derive_observe(&state, &source).is_err());

    let (mut state, source) = fixture(observe_input());
    let ConvState::ToolExecuting { current_tool, remaining_tools, .. } = &mut state else { unreachable!() };
    remaining_tools.push(current_tool.clone());
    assert!(derive_observe(&state, &source).is_err());
}
