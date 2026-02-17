//! Property-based tests for LLM provider translation layers
//!
//! These tests verify that the translation between our internal types
//! and provider wire formats preserves key invariants:
//! - Empty responses are rejected
//! - Tool calls with empty names are rejected
//! - Invalid JSON arguments are rejected
//! - Message translation never produces empty output
//! - Content is preserved through translation

#![allow(clippy::redundant_closure_for_method_calls)]

use super::anthropic::{self, AnthropicContentBlock, AnthropicResponse, AnthropicUsage};
use super::openai::{
    self, OpenAIChoice, OpenAIContent, OpenAIFunctionCall, OpenAIMessage, OpenAIResponse,
    OpenAIToolCall, OpenAIUsage,
};
use super::types::{ContentBlock, ImageSource, LlmMessage, MessageRole};
use proptest::prelude::*;

// ============================================================================
// Strategies — core generators
// ============================================================================

/// Non-empty text block
fn arb_text_block() -> impl Strategy<Value = ContentBlock> {
    "[a-zA-Z0-9 _.!?,]{1,100}".prop_map(|text| ContentBlock::Text { text })
}

/// Image block with base64 source
fn arb_image_block() -> impl Strategy<Value = ContentBlock> {
    (
        prop_oneof![
            Just("image/png".to_string()),
            Just("image/jpeg".to_string()),
        ],
        "[a-zA-Z0-9+/]{10,50}",
    )
        .prop_map(|(media_type, data)| ContentBlock::Image {
            source: ImageSource::Base64 { media_type, data },
        })
}

/// Tool use block with non-empty id/name and valid JSON input
fn arb_tool_use_block() -> impl Strategy<Value = ContentBlock> {
    (
        "[a-z0-9_]{5,20}", // id
        "[a-z_]{3,20}",    // name
        arb_json_value(),  // input
    )
        .prop_map(|(id, name, input)| ContentBlock::ToolUse { id, name, input })
}

/// Tool result block
fn arb_tool_result_block() -> impl Strategy<Value = ContentBlock> {
    (
        "[a-z0-9_]{5,20}",          // tool_use_id
        "[a-zA-Z0-9 _.!?,]{0,100}", // content
        any::<bool>(),              // is_error
    )
        .prop_map(
            |(tool_use_id, content, is_error)| ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            },
        )
}

/// Simple JSON value (no deeply nested structures)
fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        (-1000i64..1000).prop_map(|n| serde_json::Value::Number(n.into())),
        "[a-zA-Z0-9 ]{0,50}".prop_map(|s| serde_json::Value::String(s)),
        // Small object with string values
        proptest::collection::hash_map("[a-z_]{1,10}", "[a-zA-Z0-9 ]{0,30}", 0..5).prop_map(|m| {
            serde_json::Value::Object(
                m.into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect(),
            )
        }),
    ]
}

/// User message (text, images, tool results — no tool use)
fn arb_user_message() -> impl Strategy<Value = LlmMessage> {
    proptest::collection::vec(
        prop_oneof![
            3 => arb_text_block(),
            1 => arb_image_block(),
            2 => arb_tool_result_block(),
        ],
        1..6,
    )
    .prop_map(|content| LlmMessage {
        role: MessageRole::User,
        content,
    })
}

/// Assistant message (text and tool use — no images or tool results typically,
/// but we test the edge case of tool results in assistant messages for bug #4)
fn arb_assistant_message() -> impl Strategy<Value = LlmMessage> {
    proptest::collection::vec(
        prop_oneof![
            3 => arb_text_block(),
            3 => arb_tool_use_block(),
        ],
        1..6,
    )
    .prop_map(|content| LlmMessage {
        role: MessageRole::Assistant,
        content,
    })
}

/// Any valid message
fn arb_message() -> impl Strategy<Value = LlmMessage> {
    prop_oneof![arb_user_message(), arb_assistant_message(),]
}

// ============================================================================
// Strategies — provider-specific response generators
// ============================================================================

/// Build an OpenAI response with given content and optional tool calls
fn make_openai_response(
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
    finish_reason: Option<String>,
) -> OpenAIResponse {
    OpenAIResponse {
        choices: vec![OpenAIChoice {
            message: OpenAIMessage {
                role: "assistant".to_string(),
                content: content.map(OpenAIContent::Text),
                tool_calls,
                tool_call_id: None,
            },
            finish_reason,
        }],
        usage: OpenAIUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        },
    }
}

/// Build an OpenAI tool call
fn make_openai_tool_call(id: &str, name: &str, arguments: &str) -> OpenAIToolCall {
    OpenAIToolCall {
        id: id.to_string(),
        r#type: "function".to_string(),
        function: OpenAIFunctionCall {
            name: name.to_string(),
            arguments: arguments.to_string(),
        },
    }
}

/// Build an Anthropic response
fn make_anthropic_response(
    content: Vec<AnthropicContentBlock>,
    stop_reason: Option<&str>,
) -> AnthropicResponse {
    AnthropicResponse {
        content,
        stop_reason: stop_reason.map(String::from),
        usage: AnthropicUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    }
}

// ============================================================================
// Group A — Response validation (verifies bug #1 fix)
// ============================================================================

proptest! {


    /// Empty OpenAI response (no content, no tool calls) → Err
    #[test]
    fn prop_openai_normalize_rejects_empty(
        finish_reason in proptest::option::of("[a-z_]{3,10}")
    ) {
        let resp = make_openai_response(None, None, finish_reason);
        let result = openai::test_helpers::normalize_response(resp);
        prop_assert!(result.is_err(), "Expected error for empty OpenAI response");
    }

    /// Empty Anthropic response → Err
    #[test]
    fn prop_anthropic_normalize_rejects_empty(
        stop_reason in proptest::option::of("[a-z_]{3,10}")
    ) {
        let resp = AnthropicResponse {
            content: vec![],
            stop_reason,
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };
        let result = anthropic::test_helpers::normalize_response(resp);
        prop_assert!(result.is_err(), "Expected error for empty Anthropic response");
    }

    /// For all providers: Ok(resp) implies non-empty content
    #[test]
    fn prop_normalize_ok_implies_nonempty(
        text in "[a-zA-Z0-9 ]{1,100}"
    ) {
        // OpenAI
        let openai_resp = make_openai_response(
            Some(text.clone()), None, Some("stop".to_string()),
        );
        if let Ok(resp) = openai::test_helpers::normalize_response(openai_resp) {
            prop_assert!(!resp.content.is_empty(), "OpenAI Ok response had empty content");
        }

        // Anthropic
        let anth_resp = make_anthropic_response(
            vec![AnthropicContentBlock::Text { text }],
            Some("end_turn"),
        );
        if let Ok(resp) = anthropic::test_helpers::normalize_response(anth_resp) {
            prop_assert!(!resp.content.is_empty(), "Anthropic Ok response had empty content");
        }
    }
}

// ============================================================================
// Group B — Tool call integrity (catches bug #2)
// ============================================================================

proptest! {


    /// N tool calls with non-empty names → exactly N ToolUse blocks in output
    #[test]
    fn prop_openai_normalize_preserves_named_tools(
        calls in proptest::collection::vec(
            (
                "[a-z0-9]{5,15}",
                "[a-z_]{3,15}",
                arb_json_value(),
            ),
            1..5,
        ),
    ) {
        let n = calls.len();
        let tool_calls: Vec<OpenAIToolCall> = calls
            .into_iter()
            .map(|(id, name, args)| {
                make_openai_tool_call(&id, &name, &serde_json::to_string(&args).unwrap())
            })
            .collect();
        let resp = make_openai_response(None, Some(tool_calls), Some("tool_calls".to_string()));

        let result = openai::test_helpers::normalize_response(resp);
        prop_assert!(result.is_ok(), "Expected Ok for valid tool calls");
        let llm_resp = result.unwrap();
        let tool_count = llm_resp.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count();
        prop_assert_eq!(tool_count, n, "Expected {} tool calls, got {}", n, tool_count);
    }

    /// Response with empty-name tool call → Err
    #[test]
    fn prop_openai_normalize_rejects_empty_name_tools(
        id in "[a-z0-9]{5,15}",
        args in arb_json_value(),
    ) {
        let tc = make_openai_tool_call(&id, "", &serde_json::to_string(&args).unwrap());
        let resp = make_openai_response(None, Some(vec![tc]), Some("tool_calls".to_string()));
        let result = openai::test_helpers::normalize_response(resp);
        prop_assert!(result.is_err(), "Expected error for empty-name tool call");
    }
}

// ============================================================================
// Group C — JSON argument fidelity (catches bug #3)
// ============================================================================

proptest! {


    /// Valid JSON arguments survive normalize: round-trip check
    #[test]
    fn prop_normalize_valid_json_roundtrips(
        id in "[a-z0-9]{5,15}",
        name in "[a-z_]{3,15}",
        value in arb_json_value(),
    ) {
        let json_str = serde_json::to_string(&value).unwrap();
        let tc = make_openai_tool_call(&id, &name, &json_str);
        let resp = make_openai_response(None, Some(vec![tc]), Some("tool_calls".to_string()));

        let result = openai::test_helpers::normalize_response(resp);
        prop_assert!(result.is_ok(), "Expected Ok for valid JSON args");

        let llm_resp = result.unwrap();
        let tool_uses: Vec<_> = llm_resp.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { input, .. } => Some(input),
            _ => None,
        }).collect();

        prop_assert_eq!(tool_uses.len(), 1);
        // Round-trip: serialized then parsed should equal original
        let round_tripped: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(tool_uses[0]).unwrap()
        ).unwrap();
        prop_assert_eq!(&round_tripped, &value, "JSON did not round-trip");
    }

    /// Invalid JSON in tool arguments → Err
    #[test]
    fn prop_normalize_rejects_invalid_json_args(
        id in "[a-z0-9]{5,15}",
        name in "[a-z_]{3,15}",
    ) {
        // These are definitely invalid JSON
        let invalid_jsons = vec!["{invalid", "not json at all", "{key: unquoted}", "[,]"];
        for invalid in invalid_jsons {
            let tc = make_openai_tool_call(&id, &name, invalid);
            let resp = make_openai_response(None, Some(vec![tc]), Some("tool_calls".to_string()));

            let result = openai::test_helpers::normalize_response(resp);
            prop_assert!(result.is_err(), "Expected error for invalid JSON args: {}", invalid);
        }
    }
}

// ============================================================================
// Group D — Message translation invariants (catches bug #4)
// ============================================================================

proptest! {


    /// Any LlmMessage → at least one OpenAIMessage
    #[test]
    fn prop_openai_translate_never_empty_output(
        msg in arb_message(),
    ) {
        let result = openai::test_helpers::translate_message(&msg);
        prop_assert!(!result.is_empty(), "translate_message returned empty for {:?}", msg.role);
    }

    /// Every output message has either content, tool_calls, or tool_call_id
    #[test]
    fn prop_openai_translate_messages_have_content_or_tool_id(
        msg in arb_message(),
    ) {
        let messages = openai::test_helpers::translate_message(&msg);
        for m in &messages {
            let has_content = m.content.is_some();
            let has_tool_calls = m.tool_calls.is_some();
            let has_tool_call_id = m.tool_call_id.is_some();
            prop_assert!(
                has_content || has_tool_calls || has_tool_call_id,
                "OpenAI message has neither content, tool_calls, nor tool_call_id: role={}",
                m.role,
            );
        }
    }
}

// ============================================================================
// Group E — Content preservation
// ============================================================================

proptest! {


    /// Anthropic translate is 1:1 (same number of content blocks, types match)
    #[test]
    fn prop_anthropic_translate_bijective(
        msg in arb_message(),
    ) {
        let translated = anthropic::test_helpers::translate_message(&msg);
        prop_assert_eq!(
            translated.content.len(),
            msg.content.len(),
            "Anthropic translation changed content block count"
        );

        // Verify type correspondence
        for (orig, trans) in msg.content.iter().zip(translated.content.iter()) {
            match (orig, trans) {
                (ContentBlock::Text { .. }, AnthropicContentBlock::Text { .. }) => {}
                (ContentBlock::Image { .. }, AnthropicContentBlock::Image { .. }) => {}
                (ContentBlock::ToolUse { .. }, AnthropicContentBlock::ToolUse { .. }) => {}
                (ContentBlock::ToolResult { .. }, AnthropicContentBlock::ToolResult { .. }) => {}
                _ => prop_assert!(false, "Type mismatch: {:?} vs {:?}", orig, trans),
            }
        }
    }

    /// All Text blocks appear in translated OpenAI output
    #[test]
    fn prop_openai_translate_preserves_text(
        msg in arb_assistant_message(),
    ) {
        let text_blocks: Vec<&str> = msg.content.iter().filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect();

        let messages = openai::test_helpers::translate_message(&msg);
        let mut all_text = String::new();
        for m in &messages {
            if let Some(OpenAIContent::Text(t)) = &m.content {
                all_text.push_str(t);
            }
        }

        for text in &text_blocks {
            prop_assert!(
                all_text.contains(text),
                "Text '{}' not found in translated output '{}'",
                text,
                all_text,
            );
        }
    }

    /// N ToolUse blocks → N tool_calls
    #[test]
    fn prop_openai_translate_preserves_tool_use_count(
        msg in arb_assistant_message(),
    ) {
        let tool_use_count = msg.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count();
        let messages = openai::test_helpers::translate_message(&msg);

        let translated_count: usize = messages.iter()
            .filter_map(|m| m.tool_calls.as_ref())
            .map(|tcs| tcs.len())
            .sum();

        prop_assert_eq!(
            translated_count,
            tool_use_count,
            "ToolUse count mismatch"
        );
    }

    /// Each ToolResult → separate role="tool" message with matching tool_call_id
    #[test]
    fn prop_openai_translate_tool_results_become_tool_role(
        msg in arb_user_message(),
    ) {
        let tool_result_ids: Vec<&str> = msg.content.iter().filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        }).collect();

        let messages = openai::test_helpers::translate_message(&msg);
        let tool_messages: Vec<_> = messages.iter()
            .filter(|m| m.role == "tool")
            .collect();

        prop_assert_eq!(
            tool_messages.len(),
            tool_result_ids.len(),
            "Expected {} tool messages, got {}",
            tool_result_ids.len(),
            tool_messages.len(),
        );

        for (expected_id, tool_msg) in tool_result_ids.iter().zip(tool_messages.iter()) {
            prop_assert_eq!(
                tool_msg.tool_call_id.as_deref(),
                Some(*expected_id),
                "tool_call_id mismatch"
            );
        }
    }
}

// ============================================================================
// Group F — Serialization safety
// ============================================================================

proptest! {


    /// Translated messages serialize without error
    #[test]
    fn prop_translated_request_serializes(
        msg in arb_message(),
    ) {
        // OpenAI translation
        let openai_msgs = openai::test_helpers::translate_message(&msg);
        for m in &openai_msgs {
            let result = serde_json::to_value(m);
            prop_assert!(result.is_ok(), "OpenAI message failed to serialize: {:?}", result.err());
        }
    }
}
