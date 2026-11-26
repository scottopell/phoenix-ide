use super::{NativeCloseCommand, NativeCloseCommandError, NativeCloseIntent};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use uuid::Version;

fn command(transcript: &str, message: &str, tool_use: &str, target: &str) -> NativeCloseCommand {
    NativeCloseCommand::new(
        transcript.into(),
        SourceToolCall {
            message_id: message.into(),
            tool_use_id: tool_use.into(),
        },
        NativeCloseIntent::Initial {
            target: ProductConversationId::parse(target).unwrap(),
        },
    )
    .unwrap()
}

#[test]
fn same_source_and_intent_are_stable() {
    let first = command("source", "message", "tool", "target");
    assert_eq!(first, command("source", "message", "tool", "target"));
    assert_eq!(first.request_id().get_version(), Some(Version::Sha1));
    assert_eq!(first.fingerprint().len(), 64);
    assert_eq!(first.source_transcript_id(), "source");
    assert_eq!(first.source().message_id, "message");
    assert!(matches!(first.intent(), NativeCloseIntent::Initial { .. }));
}

#[test]
fn source_tuple_is_length_framed_and_each_component_changes_request_identity() {
    let base = command("ab", "c", "d", "target");
    for (transcript, message, tool_use) in [
        ("a", "bc", "d"), // Same concatenated bytes, different boundary.
        ("other", "c", "d"),
        ("ab", "other", "d"),
        ("ab", "c", "other"),
    ] {
        assert_ne!(
            base.request_id(),
            command(transcript, message, tool_use, "target").request_id()
        );
    }
    assert_ne!(
        command("a", "bc", "d", "target").request_id(),
        command("a", "b", "cd", "target").request_id()
    );
}

#[test]
fn target_changes_fingerprint_but_not_source_request_identity() {
    let first = command("source", "message", "tool", "first");
    let changed_target = command("source", "message", "tool", "second");
    let changed_source = command("source-2", "message", "tool", "first");
    assert_eq!(first.request_id(), changed_target.request_id());
    assert_ne!(first.fingerprint(), changed_target.fingerprint());
    assert_ne!(first.request_id(), changed_source.request_id());
    assert_eq!(first.fingerprint(), changed_source.fingerprint());
}

#[test]
fn observe_fingerprint_binds_intent_target_attempt_and_exact_ordinal_not_source() {
    use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};

    let observe = |source: &str, target: &str, attempt: &str, ordinal: i64| {
        NativeCloseCommand::new(
            source.into(),
            SourceToolCall {
                message_id: "message".into(),
                tool_use_id: "tool".into(),
            },
            NativeCloseIntent::Observe {
                target: ProductConversationId::parse(target).unwrap(),
                attempt_id: CloseAttemptId::parse(attempt).unwrap(),
                run_ordinal: CloseRunOrdinal::parse(ordinal).unwrap(),
            },
        )
        .unwrap()
    };
    let base = observe("source", "ab", "c", 1);
    assert_eq!(base, observe("source", "ab", "c", 1));
    assert_eq!(base.fingerprint().len(), 64);
    for changed in [
        command("source", "message", "tool", "ab"),
        observe("source", "other", "c", 1),
        observe("source", "ab", "other", 1),
        observe("source", "ab", "c", 2),
        observe("source", "a", "bc", 1),
    ] {
        assert_eq!(base.request_id(), changed.request_id());
        assert_ne!(base.fingerprint(), changed.fingerprint());
    }
    let other_source = observe("other-source", "ab", "c", 1);
    assert_ne!(base.request_id(), other_source.request_id());
    assert_eq!(base.fingerprint(), other_source.fingerprint());
    assert_eq!(
        base.intent().source_input(),
        serde_json::json!({"request": {
            "action": "observe", "product_conversation_id": "ab",
            "attempt_id": "c", "run_ordinal": 1,
        }})
    );
}

#[test]
fn empty_source_fields_are_rejected() {
    for (transcript, message, tool_use) in [
        ("", "message", "tool"),
        ("source", "", "tool"),
        ("source", "message", ""),
        (" \t", "message", "tool"),
        ("source", " \t", "tool"),
        ("source", "message", " \t"),
    ] {
        assert_eq!(
            NativeCloseCommand::new(
                transcript.into(),
                SourceToolCall {
                    message_id: message.into(),
                    tool_use_id: tool_use.into(),
                },
                NativeCloseIntent::Initial {
                    target: ProductConversationId::parse("target").unwrap(),
                },
            ),
            Err(NativeCloseCommandError::EmptySource)
        );
    }
}

fn source_intents() -> Vec<NativeCloseIntent> {
    use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};

    let target = ProductConversationId::parse("target").unwrap();
    let attempt_id = CloseAttemptId::parse("attempt").unwrap();
    vec![
        NativeCloseIntent::Initial {
            target: target.clone(),
        },
        NativeCloseIntent::Observe {
            target: target.clone(),
            attempt_id: attempt_id.clone(),
            run_ordinal: CloseRunOrdinal::parse(2).unwrap(),
        },
        NativeCloseIntent::ExplicitRetry {
            target,
            attempt_id,
            failed_run_ordinal: CloseRunOrdinal::parse(2).unwrap(),
        },
    ]
}

pub(super) fn executing_state(
    source: &SourceToolCall,
    assistant_input: serde_json::Value,
    current_input: serde_json::Value,
) -> phoenix_core::domain::sm_state::ConvState {
    use phoenix_core::domain::llm_types::ContentBlock;
    use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};

    ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &source.tool_use_id,
            ToolInput::Unknown {
                name: "close_conversation".into(),
                input: current_input,
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
                input: assistant_input,
            }],
            None,
            None,
        ),
    }
}

#[test]
fn legacy_fingerprint_bytes_are_unchanged_and_wrapping_is_domain_separated() {
    use sha2::{Digest, Sha256};

    let source = SourceToolCall {
        message_id: "message".into(),
        tool_use_id: "tool".into(),
    };
    for intent in source_intents() {
        let wrapped =
            NativeCloseCommand::new("transcript".into(), source.clone(), intent.clone()).unwrap();
        let legacy = NativeCloseCommand::new_with_source_input(
            "transcript".into(),
            source.clone(),
            intent.clone(),
            &intent.legacy_source_input(),
        )
        .unwrap();
        let mut bytes = b"phoenix.native_close.intent.v1\0".to_vec();
        // This is the historical algorithm: action, target, then exact run coordinates.
        super::append_field(&mut bytes, intent.receipt_intent().as_bytes());
        super::append_field(&mut bytes, intent.target().as_str().as_bytes());
        match &intent {
            NativeCloseIntent::Initial { .. } => {},
            NativeCloseIntent::Observe {
                attempt_id,
                run_ordinal,
                ..
            } => {
                super::append_field(&mut bytes, attempt_id.as_str().as_bytes());
                super::append_field(&mut bytes, &run_ordinal.get().to_be_bytes());
            },
            NativeCloseIntent::ExplicitRetry {
                attempt_id,
                failed_run_ordinal,
                ..
            } => {
                super::append_field(&mut bytes, attempt_id.as_str().as_bytes());
                super::append_field(&mut bytes, &failed_run_ordinal.get().to_be_bytes());
            },
        }
        assert_eq!(legacy.fingerprint(), format!("{:x}", Sha256::digest(&bytes)));
        assert_ne!(legacy.fingerprint(), wrapped.fingerprint());
        assert_eq!(legacy.request_id(), wrapped.request_id());
        assert_eq!(
            NativeCloseCommand::new_from_retained_fingerprint(
                "transcript".into(),
                source.clone(),
                intent,
                "unknown-fingerprint",
            ),
            Err(NativeCloseCommandError::InvalidRetainedFingerprint)
        );
    }
}

#[test]
fn supported_source_shapes_are_exact_and_must_agree_within_the_occurrence() {
    let source = SourceToolCall {
        message_id: "message".into(),
        tool_use_id: "tool".into(),
    };
    for intent in source_intents() {
        let wrapped = intent.source_input();
        let flat = intent.legacy_source_input();
        let command =
            NativeCloseCommand::new("transcript".into(), source.clone(), intent.clone()).unwrap();
        assert_eq!(wrapped, serde_json::json!({"request": flat.clone()}));
        for input in [&wrapped, &flat] {
            assert!(intent.matches_source_input(input));
            let command = NativeCloseCommand::new_with_source_input(
                "transcript".into(),
                source.clone(),
                intent.clone(),
                input,
            )
            .unwrap();
            assert_eq!(command.source_input(), *input);
            let retained = NativeCloseCommand::new_from_retained_fingerprint(
                "transcript".into(),
                source.clone(),
                intent.clone(),
                command.fingerprint(),
            )
            .unwrap();
            assert_eq!(retained, command);
            let opposite = if input == &wrapped { &flat } else { &wrapped };
            assert!(!command.matches_source_input(opposite));
            assert!(command
                .verify_executing_source(
                    &executing_state(&source, opposite.clone(), opposite.clone()),
                    &source,
                )
                .is_err());
            let state = executing_state(&source, input.clone(), input.clone());
            assert_eq!(command.verify_executing_source(&state, &source), Ok(()));
            for wrong_source in [
                SourceToolCall {
                    message_id: "other".into(),
                    ..source.clone()
                },
                SourceToolCall {
                    tool_use_id: "other".into(),
                    ..source.clone()
                },
            ] {
                assert!(command
                    .verify_executing_source(&state, &wrong_source)
                    .is_err());
            }
        }
        for (assistant, current) in [(&wrapped, &flat), (&flat, &wrapped)] {
            let state = executing_state(&source, assistant.clone(), current.clone());
            assert!(command.verify_executing_source(&state, &source).is_err());
        }
        for wrapped_shape in [false, true] {
            let mut malformed = vec![serde_json::json!({"request": {"request": flat.clone()}})];
            let mut extra = flat.clone();
            extra["irrelevant"] = serde_json::json!(true);
            malformed.push(extra);
            for key in flat.as_object().unwrap().keys() {
                let mut changed = flat.clone();
                changed[key] = serde_json::json!("altered-coordinate");
                malformed.push(changed);
                let mut missing = flat.clone();
                missing.as_object_mut().unwrap().remove(key);
                malformed.push(missing);
            }
            for input in malformed {
                let input = if wrapped_shape {
                    serde_json::json!({"request": input})
                } else {
                    input
                };
                assert!(!intent.matches_source_input(&input));
                assert!(NativeCloseCommand::new_with_source_input(
                    "transcript".into(),
                    source.clone(),
                    intent.clone(),
                    &input,
                )
                .is_err());
                let state = executing_state(&source, input.clone(), input);
                assert!(command.verify_executing_source(&state, &source).is_err());
            }
        }
        let mut extra_wrapper = wrapped;
        extra_wrapper["irrelevant"] = serde_json::json!(true);
        assert!(!intent.matches_source_input(&extra_wrapper));
    }
}
