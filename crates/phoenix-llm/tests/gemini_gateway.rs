//! Explicit opt-in qualification of Gemini tool rounds against the configured gateway.

use phoenix_core::domain::{
    llm_types::{
        ContentBlock, EffectiveEffort, EffectiveServiceTier, LlmMessage, LlmRequest, MessageRole,
        ModelEffort, PromptCacheKey, SystemContent, ToolDefinition,
    },
    provider_replay::ProviderReplayUpdate,
    tool_availability::ToolAvailability,
};
use phoenix_llm::{LlmConfig, ModelRegistry};

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[ignore = "uses configured gateway credentials and incurs small inference charges"]
async fn gemini_gateway_signed_tool_rounds() {
    let config = LlmConfig::from_env(std::sync::Arc::new(
        phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect(),
    ));
    assert!(config.openai_chat_completions_base_url.is_some());
    let registry = ModelRegistry::new(&config);
    for model in [
        "google/gemini-4-neon",
        "google/gemini-4-hydrogen",
        "google/gemini-4-carbon",
        "google/gemini-4-argon",
    ] {
        let client = registry
            .get(model)
            .expect("Gemini gateway model registered");
        for streaming in [false, true] {
            let mut request = LlmRequest {
                system: vec![SystemContent::new("Use report_number to report the user's number. After receiving the result, reply with exactly OK.")],
                messages: vec![LlmMessage { source_message_id: None, role: MessageRole::User, content: vec![ContentBlock::text("Report number 7 with the tool.")] }],
                provider_replay: None,
                output_replay: vec![],
                tool_availability: ToolAvailability::all(vec![ToolDefinition { defer_loading: false, name: "report_number".into(), description: "Report the requested number.".into(), input_schema: serde_json::json!({"type":"object","properties":{"number":{"type":"integer"}},"required":["number"],"additionalProperties":false}) }]),
                max_tokens: 2048,
                effective_effort: EffectiveEffort::explicit(ModelEffort::Low),
                service_tier: EffectiveServiceTier::Standard,
                telemetry: None,
                cache_key: PromptCacheKey::ephemeral(),
            };
            let (tx, mut rx) = tokio::sync::mpsc::channel(128);
            let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let response = if streaming {
                client.complete_streaming(&request, &tx).await
            } else {
                client.complete(&request).await
            }
            .expect("first gateway tool call succeeds");
            assert!(!response.end_turn);
            if let Some(reasoning) = response.usage.reasoning_tokens {
                assert!(response.usage.output_tokens >= reasoning);
            }
            let call_id = response
                .content
                .iter()
                .find_map(|block| {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        assert_eq!(name, "report_number");
                        assert_eq!(input, &serde_json::json!({"number":7}));
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .expect("provider issued report_number");
            let Some(ProviderReplayUpdate::ChatCompletions(set)) = response.provider_replay else {
                panic!("provider signature retained privately")
            };
            assert!(!set.tool_signatures.is_empty());
            let set = set.with_owner_message_id("assistant-tool".into());
            // A fresh deserialized replay value exercises the restart representation.
            let restored = serde_json::from_str::<
                phoenix_core::domain::responses_replay::ChatResponseSet,
            >(&serde_json::to_string(&set).unwrap())
            .unwrap();
            request.output_replay.push(restored.into());
            request.messages.push(LlmMessage {
                source_message_id: Some("assistant-tool".into()),
                role: MessageRole::Assistant,
                content: response.content,
            });
            request.messages.push(LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content: "{\"reported\":7}".into(),
                    is_error: false,
                    images: vec![],
                }],
            });
            let answer = if streaming {
                client.complete_streaming(&request, &tx).await
            } else {
                client.complete(&request).await
            }
            .expect("signed gateway tool continuation succeeds");
            assert!(answer.end_turn);
            assert!(answer
                .content
                .iter()
                .any(|block| matches!(block,ContentBlock::Text { text } if text.trim() == "OK")));
            request.messages.push(LlmMessage {
                source_message_id: Some("assistant-final".into()),
                role: MessageRole::Assistant,
                content: answer.content,
            });
            request.output_replay.clear();
            request.messages.push(LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text(
                    "New turn: reply with exactly DONE. No tool is needed.",
                )],
            });
            let settled = if streaming {
                client.complete_streaming(&request, &tx).await
            } else {
                client.complete(&request).await
            }
            .expect("settled tool history remains usable after private replay retires");
            assert!(settled.end_turn);
            println!("{model}: streaming={streaming} signed tool round passed");
            drop(tx);
            drain.await.unwrap();
        }
    }
}
