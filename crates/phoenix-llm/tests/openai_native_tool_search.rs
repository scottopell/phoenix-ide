//! Opt-in native search qualification on the direct Responses and Codex routes.

use std::sync::Arc;

use phoenix_core::domain::{
    llm_types::{
        ContentBlock, EffectiveEffort, EffectiveServiceTier, LlmMessage, LlmRequest, LlmResponse,
        MessageRole, ModelEffort, PromptCacheKey, SystemContent, ToolDefinition,
    },
    provider_replay::ProviderReplayUpdate,
    tool_availability::ToolAvailability,
    tool_discovery::{
        tool_search_definition, ToolDiscoveryMode, ToolDiscoveryProtocol, ToolDiscoveryResult,
        TOOL_SEARCH_NAME,
    },
};
use phoenix_llm::{LlmConfig, LlmService, ModelRegistry};

const MODEL: &str = "gpt-6.2-sol";

fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "report_number".into(),
        description: "Report the requested number.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{"number":{"type":"integer"}},"required":["number"],"additionalProperties":false}),
        defer_loading: true,
    }
}

fn initial_request(mode: ToolDiscoveryMode) -> LlmRequest {
    let tools = if mode == ToolDiscoveryMode::OpenAiClient {
        vec![tool_search_definition()]
    } else {
        vec![definition()]
    };
    LlmRequest {
        system:vec![SystemContent::new("First use tool search to discover report_number, then use it to report the user's number. After its result, reply exactly OK.")],
        messages:vec![LlmMessage{source_message_id:None,role:MessageRole::User,content:vec![ContentBlock::text("Report number 7 using the tool.")]}],
        tool_availability:ToolAvailability::all(tools).with_discovery(mode,vec![]).unwrap(),
        provider_replay:None,output_replay:vec![],max_tokens:2048,
        effective_effort:EffectiveEffort::explicit(ModelEffort::Low), service_tier:EffectiveServiceTier::Standard,
        telemetry:None, cache_key:PromptCacheKey::ephemeral(),
    }
}

async fn complete(service: &dyn LlmService, request: &LlmRequest, streaming: bool) -> LlmResponse {
    if streaming {
        let (tx, mut rx) = tokio::sync::mpsc::channel(128);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let response = service
            .complete_streaming(request, &tx)
            .await
            .expect("native search provider request succeeds");
        drop(tx);
        drain.await.unwrap();
        response
    } else {
        service
            .complete(request)
            .await
            .expect("native search provider request succeeds")
    }
}

fn retain(request: &mut LlmRequest, mut response: LlmResponse, owner: &str) -> Vec<ContentBlock> {
    let Some(ProviderReplayUpdate::Responses(set)) = response.provider_replay.take() else {
        panic!("native tool round keeps private Responses replay")
    };
    request
        .output_replay
        .push(set.with_owner_message_id(owner.into()).into());
    let public = response.content;
    request.messages.push(LlmMessage {
        source_message_id: Some(owner.into()),
        role: MessageRole::Assistant,
        content: public.clone(),
    });
    public
}

fn call_id(content: &[ContentBlock], name: &str, number: Option<i64>) -> String {
    let calls = content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse {
                id,
                name: actual,
                input,
            } = block
            {
                assert_eq!(actual, name);
                if let Some(number) = number {
                    assert_eq!(
                        input.get("number").and_then(serde_json::Value::as_i64),
                        Some(number)
                    );
                }
                Some(id.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    calls[0].clone()
}

#[allow(clippy::too_many_lines)]
async fn qualify(service: &dyn LlmService, mode: ToolDiscoveryMode, streaming: bool) {
    let mut request = initial_request(mode);
    let first = complete(service, &request, streaming).await;
    assert!(!first.end_turn);
    let function = if mode == ToolDiscoveryMode::OpenAiClient {
        let public = retain(&mut request, first, "search-owner");
        let search_id = call_id(&public, TOOL_SEARCH_NAME, None);
        request.messages.push(LlmMessage {
            source_message_id: Some("search-result".into()),
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: search_id.clone(),
                content: "Loaded report_number".into(),
                is_error: false,
                images: vec![],
            }],
        });
        request.tool_availability =
            ToolAvailability::all(vec![tool_search_definition(), definition()])
                .with_discovery(
                    mode,
                    vec![ToolDiscoveryResult {
                        owner_message_id: "search-owner".into(),
                        tool_call_id: search_id,
                        result_message_id: "search-result".into(),
                        protocol: ToolDiscoveryProtocol::OpenAiClient,
                        tools: vec![definition()],
                    }],
                )
                .unwrap();
        complete(service, &request, streaming).await
    } else {
        let Some(ProviderReplayUpdate::Responses(set)) = &first.provider_replay else {
            panic!("hosted search replay retained")
        };
        for kind in ["tool_search_call", "tool_search_output"] {
            assert!(set.output_items.iter().any(|item| item["type"] == kind));
        }
        first
    };
    assert!(!function.end_turn);
    let public = retain(&mut request, function, "function-owner");
    let id = call_id(&public, "report_number", Some(7));
    request.messages.push(LlmMessage {
        source_message_id: Some("function-result".into()),
        role: MessageRole::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: id,
            content: "{\"reported\":7}".into(),
            is_error: false,
            images: vec![],
        }],
    });
    let answer = complete(service, &request, streaming).await;
    assert!(answer.end_turn);
    assert!(matches!(
        answer.provider_replay,
        Some(ProviderReplayUpdate::Clear)
    ));
    assert!(answer
        .content
        .iter()
        .any(|block| matches!(block,ContentBlock::Text{text} if text.trim()=="OK")));
    request.messages.push(LlmMessage {
        source_message_id: Some("final-owner".into()),
        role: MessageRole::Assistant,
        content: answer.content,
    });
    request.output_replay.clear();
    request.messages.push(LlmMessage {
        source_message_id: None,
        role: MessageRole::User,
        content: vec![ContentBlock::text(
            "New turn: report number 8 using report_number.",
        )],
    });
    let next = complete(service, &request, streaming).await;
    assert!(!next.end_turn);
    // A provider may perform another hosted discovery internally; only its actual function is public.
    assert!(!call_id(&next.content, "report_number", Some(8)).is_empty());
    println!("{mode:?}: streaming={streaming} native exchange and settled next turn passed");
}

#[tokio::test]
#[ignore = "uses configured direct Responses credentials and incurs small inference charges"]
async fn direct_hosted_tool_search_roundtrip() {
    let mut config = LlmConfig::from_env(Arc::new(
        phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect(),
    ));
    config.use_codex_auth = false;
    let registry = ModelRegistry::new(&config);
    let service = registry.get(MODEL).expect("direct model registered");
    assert!(!service.uses_codex_bridge());
    assert_eq!(
        service.tool_discovery_mode(),
        ToolDiscoveryMode::OpenAiHosted
    );
    for streaming in [false, true] {
        qualify(service.as_ref(), ToolDiscoveryMode::OpenAiHosted, streaming).await;
    }
}

#[tokio::test]
#[ignore = "requires Phoenix-native Codex login credentials and incurs small inference charges"]
async fn codex_native_client_tool_search_roundtrip() {
    let config = LlmConfig::from_env(Arc::new(
        phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect(),
    ));
    assert!(
        config.use_codex_auth,
        "Sign in with Codex from Phoenix before running this qualification"
    );
    let registry = ModelRegistry::new(&config);
    let service = registry.get(MODEL).expect("Codex model registered");
    assert!(service.uses_codex_bridge());
    assert_eq!(
        service.tool_discovery_mode(),
        ToolDiscoveryMode::OpenAiClient
    );
    for streaming in [false, true] {
        qualify(service.as_ref(), ToolDiscoveryMode::OpenAiClient, streaming).await;
    }
}
