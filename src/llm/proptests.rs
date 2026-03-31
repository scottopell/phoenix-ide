//! Property-based tests for LLM provider translation layers
//!
//! These tests verify that the translation between our internal types
//! and provider wire formats preserves key invariants:
//! - Empty responses are rejected
//! - Message translation never produces empty output
//! - Content is preserved through translation

#![allow(clippy::redundant_closure_for_method_calls)]

use super::anthropic::{self, AnthropicContentBlock, AnthropicResponse, AnthropicUsage};
use super::openai::{
    self, ResponsesApiContentPart, ResponsesApiFunctionOutput, ResponsesApiInputItem,
    ResponsesApiMessageContent, ResponsesApiOutputPart,
};
use super::types::{
    ContentBlock, ImageSource, LlmMessage, LlmRequest, MessageRole, ToolReference,
    ToolSearchResultContent,
};
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

/// Tool result block (no images — for use in general message tests)
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
                images: vec![],
                is_error,
            },
        )
}

/// Image source
fn arb_image_source() -> impl Strategy<Value = ImageSource> {
    (
        prop_oneof![
            Just("image/png".to_string()),
            Just("image/jpeg".to_string()),
        ],
        "[a-zA-Z0-9+/]{10,50}",
    )
        .prop_map(|(media_type, data)| ImageSource::Base64 { media_type, data })
}

/// Simple JSON value (no deeply nested structures)
fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        (-1000i64..1000).prop_map(|n| serde_json::Value::Number(n.into())),
        "[a-zA-Z0-9 ]{0,50}".prop_map(serde_json::Value::String),
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

/// Assistant message (text and tool use)
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
// Group A — Anthropic response validation
// ============================================================================

proptest! {

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

    /// Ok(resp) implies non-empty content
    #[test]
    fn prop_anthropic_normalize_ok_implies_nonempty(
        text in "[a-zA-Z0-9 ]{1,100}"
    ) {
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
// Group E — Anthropic content preservation
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
                (ContentBlock::Text { .. }, AnthropicContentBlock::Text { .. })
                | (ContentBlock::Image { .. }, AnthropicContentBlock::Image { .. })
                | (ContentBlock::ToolUse { .. }, AnthropicContentBlock::ToolUse { .. })
                | (ContentBlock::ToolResult { .. }, AnthropicContentBlock::ToolResult { .. })
                | (ContentBlock::ServerToolUse { .. }, AnthropicContentBlock::ServerToolUse { .. })
                | (ContentBlock::ToolSearchToolResult { .. }, AnthropicContentBlock::ToolSearchToolResult { .. })
                | (ContentBlock::WebSearchToolResult { .. }, AnthropicContentBlock::WebSearchToolResult { .. })
                | (ContentBlock::WebFetchToolResult { .. }, AnthropicContentBlock::WebFetchToolResult { .. })
                | (ContentBlock::CodeExecutionToolResult { .. }, AnthropicContentBlock::CodeExecutionToolResult { .. })
                | (ContentBlock::BashCodeExecutionToolResult { .. }, AnthropicContentBlock::BashCodeExecutionToolResult { .. })
                | (ContentBlock::TextEditorCodeExecutionToolResult { .. }, AnthropicContentBlock::TextEditorCodeExecutionToolResult { .. })
                | (ContentBlock::McpToolUse { .. }, AnthropicContentBlock::McpToolUse { .. })
                | (ContentBlock::McpToolResult { .. }, AnthropicContentBlock::McpToolResult { .. }) => {}
                _ => prop_assert!(false, "Type mismatch: {:?} vs {:?}", orig, trans),
            }
        }
    }
}

// ============================================================================
// Group G — Anthropic ToolResult image channel invariants
// ============================================================================

proptest! {

    /// A — Anthropic: no-image ToolResult → content is a JSON string (backwards compat)
    #[test]
    fn prop_anthropic_tool_result_no_images_string_content(
        tool_use_id in "[a-z0-9_]{5,20}",
        content in "[a-zA-Z0-9 _.!?,]{0,100}",
        is_error in any::<bool>(),
    ) {
        let block = ContentBlock::ToolResult {
            tool_use_id,
            content,
            images: vec![],
            is_error,
        };
        let msg = LlmMessage { role: MessageRole::User, content: vec![block] };
        let translated = anthropic::test_helpers::translate_message(&msg);
        prop_assert_eq!(translated.content.len(), 1);
        if let super::anthropic::AnthropicContentBlock::ToolResult { content: wire_content, .. } =
            &translated.content[0]
        {
            prop_assert!(
                wire_content.is_string(),
                "Expected string content for no-image ToolResult, got: {:?}",
                wire_content
            );
        } else {
            prop_assert!(false, "Expected ToolResult block");
        }
    }

    /// B — Anthropic: image-bearing ToolResult → content is array [text, ...images]
    #[test]
    fn prop_anthropic_tool_result_with_images_array_content(
        tool_use_id in "[a-z0-9_]{5,20}",
        content in "[a-zA-Z0-9 _.!?,]{0,100}",
        is_error in any::<bool>(),
        images in proptest::collection::vec(arb_image_source(), 1..3usize),
    ) {
        let n_images = images.len();
        let block = ContentBlock::ToolResult {
            tool_use_id,
            content: content.clone(),
            images,
            is_error,
        };
        let msg = LlmMessage { role: MessageRole::User, content: vec![block] };
        let translated = anthropic::test_helpers::translate_message(&msg);
        prop_assert_eq!(translated.content.len(), 1);
        if let super::anthropic::AnthropicContentBlock::ToolResult { content: wire_content, .. } =
            &translated.content[0]
        {
            prop_assert!(
                wire_content.is_array(),
                "Expected array content for image-bearing ToolResult, got: {:?}",
                wire_content
            );
            let arr = wire_content.as_array().unwrap();
            prop_assert_eq!(arr.len(), 1 + n_images, "Expected 1 text + {} image blocks", n_images);
            prop_assert_eq!(&arr[0]["type"], "text", "First block must be text");
            prop_assert_eq!(&arr[0]["text"], content.as_str(), "Text content must match");
            for block in &arr[1..] {
                prop_assert_eq!(&block["type"], "image", "Remaining blocks must be images");
                prop_assert_eq!(&block["source"]["type"], "base64");
            }
        } else {
            prop_assert!(false, "Expected ToolResult block");
        }
    }
}

// ============================================================================
// Group H — Responses API invariants
// ============================================================================

fn make_llm_request(messages: Vec<LlmMessage>) -> LlmRequest {
    LlmRequest {
        system: vec![],
        messages,
        tools: vec![],
        max_tokens: None,
    }
}

proptest! {

    /// H1 — Text-only user message → ResponsesApiMessageContent::Text
    #[test]
    fn prop_responses_text_only_message_is_string(
        text in "[a-zA-Z0-9 _.!?,]{1,100}",
    ) {
        let msg = LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: text.clone() }],
        };
        let req = make_llm_request(vec![msg]);
        let responses_req = openai::test_helpers::translate_to_responses_request("gpt-4o", &req);

        prop_assert_eq!(responses_req.input.len(), 1);
        if let ResponsesApiInputItem::Message { content, .. } = &responses_req.input[0] {
            prop_assert!(
                matches!(content, ResponsesApiMessageContent::Text(_)),
                "Expected Text content for text-only message"
            );
        } else {
            prop_assert!(false, "Expected Message item");
        }
    }

    /// H2 — User message with image → Parts containing InputImage with correct data URL
    #[test]
    fn prop_responses_message_with_image_uses_parts(
        text in "[a-zA-Z0-9 _.!?,]{1,50}",
        media_type in prop_oneof![Just("image/png".to_string()), Just("image/jpeg".to_string())],
        data in "[a-zA-Z0-9+/]{10,50}",
    ) {
        let msg = LlmMessage {
            role: MessageRole::User,
            content: vec![
                ContentBlock::Text { text: text.clone() },
                ContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                },
            ],
        };
        let req = make_llm_request(vec![msg]);
        let responses_req = openai::test_helpers::translate_to_responses_request("gpt-4o", &req);

        prop_assert_eq!(responses_req.input.len(), 1);
        if let ResponsesApiInputItem::Message { content, .. } = &responses_req.input[0] {
            if let ResponsesApiMessageContent::Parts(parts) = content {
                let expected_url = format!("data:{media_type};base64,{data}");
                let has_image = parts.iter().any(|p| {
                    matches!(p, ResponsesApiContentPart::InputImage { image_url }
                        if image_url == &expected_url)
                });
                prop_assert!(has_image, "Parts must contain InputImage with correct data URL");
            } else {
                prop_assert!(false, "Expected Parts content for message with image");
            }
        } else {
            prop_assert!(false, "Expected Message item");
        }
    }

    /// H3 — Tool result without images → FunctionCallOutput { output: Text(_) }
    #[test]
    fn prop_responses_tool_result_no_images_is_text(
        tool_use_id in "[a-z0-9_]{5,20}",
        content in "[a-zA-Z0-9 _.!?,]{0,100}",
        is_error in any::<bool>(),
    ) {
        let msg = LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content,
                images: vec![],
                is_error,
            }],
        };
        let req = make_llm_request(vec![msg]);
        let responses_req = openai::test_helpers::translate_to_responses_request("gpt-4o", &req);

        prop_assert_eq!(responses_req.input.len(), 1);
        if let ResponsesApiInputItem::FunctionCallOutput { output, .. } = &responses_req.input[0] {
            prop_assert!(
                matches!(output, ResponsesApiFunctionOutput::Text(_)),
                "Expected Text output for no-image tool result"
            );
        } else {
            prop_assert!(false, "Expected FunctionCallOutput item");
        }
    }

    /// H4 — Tool result with N images → Parts(N+1): Parts[0] is Text, Parts[1..] are ImageUrl
    ///      with correct data URLs
    #[test]
    fn prop_responses_tool_result_with_images_uses_parts(
        tool_use_id in "[a-z0-9_]{5,20}",
        content in "[a-zA-Z0-9 _.!?,]{0,100}",
        is_error in any::<bool>(),
        images in proptest::collection::vec(arb_image_source(), 1..3usize),
    ) {
        let n_images = images.len();
        let expected_urls: Vec<String> = images
            .iter()
            .map(|ImageSource::Base64 { media_type, data }| {
                format!("data:{media_type};base64,{data}")
            })
            .collect();

        let msg = LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content,
                images,
                is_error,
            }],
        };
        let req = make_llm_request(vec![msg]);
        let responses_req = openai::test_helpers::translate_to_responses_request("gpt-4o", &req);

        prop_assert_eq!(responses_req.input.len(), 1);
        if let ResponsesApiInputItem::FunctionCallOutput { output, .. } = &responses_req.input[0] {
            if let ResponsesApiFunctionOutput::Parts(parts) = output {
                prop_assert_eq!(parts.len(), 1 + n_images, "Expected 1 text + {} image parts", n_images);
                prop_assert!(
                    matches!(&parts[0], ResponsesApiOutputPart::Text { .. }),
                    "Parts[0] must be Text"
                );
                for (i, expected_url) in expected_urls.iter().enumerate() {
                    if let ResponsesApiOutputPart::ImageUrl { image_url } = &parts[1 + i] {
                        prop_assert_eq!(
                            &image_url.url, expected_url,
                            "ImageUrl data URL mismatch at index {}", i
                        );
                    } else {
                        prop_assert!(false, "Parts[{}] must be ImageUrl", 1 + i);
                    }
                }
            } else {
                prop_assert!(false, "Expected Parts output for tool result with images");
            }
        } else {
            prop_assert!(false, "Expected FunctionCallOutput item");
        }
    }
}

// ============================================================================
// Server block strategies
// ============================================================================

fn arb_server_tool_use() -> impl Strategy<Value = ContentBlock> {
    (
        "srvtoolu_[a-z0-9]{5,15}",
        prop_oneof![
            Just("tool_search_tool_regex".to_string()),
            Just("web_search".to_string()),
            Just("code_execution".to_string()),
        ],
        arb_json_value(),
    )
        .prop_map(|(id, name, input)| ContentBlock::ServerToolUse { id, name, input })
}

fn arb_tool_search_tool_result() -> impl Strategy<Value = ContentBlock> {
    (
        "srvtoolu_[a-z0-9]{5,15}",
        proptest::collection::vec("[a-z_]{3,20}", 0..5),
    )
        .prop_map(|(tool_use_id, tool_names)| ContentBlock::ToolSearchToolResult {
            tool_use_id,
            content: ToolSearchResultContent {
                r#type: "tool_search_tool_search_result".into(),
                tool_references: tool_names
                    .into_iter()
                    .map(|n| ToolReference {
                        r#type: "tool_reference".into(),
                        tool_name: n,
                    })
                    .collect(),
                error_code: None,
            },
        })
}

fn arb_opaque_server_result() -> impl Strategy<Value = ContentBlock> {
    ("srvtoolu_[a-z0-9]{5,15}", arb_json_value()).prop_flat_map(|(id, content)| {
        prop_oneof![
            Just(ContentBlock::WebSearchToolResult {
                tool_use_id: id.clone(),
                content: content.clone(),
            }),
            Just(ContentBlock::WebFetchToolResult {
                tool_use_id: id.clone(),
                content: content.clone(),
            }),
            Just(ContentBlock::CodeExecutionToolResult {
                tool_use_id: id.clone(),
                content: content.clone(),
            }),
            Just(ContentBlock::BashCodeExecutionToolResult {
                tool_use_id: id.clone(),
                content: content.clone(),
            }),
        ]
    })
}

fn arb_mcp_tool_use() -> impl Strategy<Value = ContentBlock> {
    (
        "mcptoolu_[a-z0-9]{5,15}",
        "[a-z_]{3,20}",
        "[a-z_]{3,15}",
        arb_json_value(),
    )
        .prop_map(|(id, name, server_name, input)| ContentBlock::McpToolUse {
            id,
            name,
            server_name,
            input,
        })
}

/// Any ContentBlock variant
fn arb_content_block() -> impl Strategy<Value = ContentBlock> {
    prop_oneof![
        3 => arb_text_block(),
        1 => arb_image_block(),
        2 => arb_tool_use_block(),
        2 => arb_tool_result_block(),
        2 => arb_server_tool_use(),
        1 => arb_tool_search_tool_result(),
        2 => arb_opaque_server_result(),
        1 => arb_mcp_tool_use(),
    ]
}

// ============================================================================
// ContentBlock property tests
// ============================================================================

proptest! {
    /// Every ContentBlock round-trips through JSON: deserialize(serialize(x)) == x
    #[test]
    fn prop_content_block_serde_round_trip(block in arb_content_block()) {
        let json = serde_json::to_value(&block).unwrap();
        let round_tripped: ContentBlock = serde_json::from_value(json).unwrap();
        prop_assert_eq!(block, round_tripped);
    }

    /// Every serialized ContentBlock has a non-empty snake_case "type" tag
    #[test]
    fn prop_content_block_type_tag_valid(block in arb_content_block()) {
        let json = serde_json::to_value(&block).unwrap();
        let type_str = json.get("type").and_then(|v| v.as_str());
        prop_assert!(type_str.is_some(), "missing type field");
        let t = type_str.unwrap();
        prop_assert!(!t.is_empty(), "empty type string");
        prop_assert!(
            t.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "type tag not snake_case: {t}"
        );
    }

    /// tool_uses() only returns ToolUse -- never server or MCP blocks
    #[test]
    fn prop_tool_uses_only_returns_tool_use(
        blocks in proptest::collection::vec(arb_content_block(), 0..20)
    ) {
        let response = super::types::LlmResponse {
            content: blocks,
            end_turn: false,
            usage: super::types::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        };
        for (id, _name, _input) in response.tool_uses() {
            prop_assert!(
                !id.starts_with("srvtoolu_"),
                "tool_uses() returned a server tool: {id}"
            );
            prop_assert!(
                !id.starts_with("mcptoolu_"),
                "tool_uses() returned an MCP tool: {id}"
            );
        }
    }
}
