//! Opt-in qualification of shared function-based discovery on configured Chat routes.

use std::{sync::Arc, time::Duration};

use phoenix_core::domain::{
    llm_types::{
        ContentBlock, EffectiveEffort, EffectiveServiceTier, LlmMessage, LlmRequest, LlmResponse,
        MessageRole, PromptCacheKey, SystemContent, ToolDefinition,
    },
    provider_replay::ProviderReplayUpdate,
    responses_replay::ChatResponseSet,
    tool_availability::ToolAvailability,
    tool_discovery::{
        search_tools, tool_search_definition, tool_search_result_text, ToolDiscoveryMode,
        ToolDiscoveryProtocol, ToolDiscoveryResult, ToolSearchQuery, TOOL_SEARCH_NAME,
    },
};
use phoenix_llm::{LlmConfig, LlmService, ModelRegistry};
use serde_json::json;

const REPORT_TOOL: &str = "mcp__numbers__report_number";
const GEMINI_MODELS: [&str; 4] = [
    "google/gemini-4-neon",
    "google/gemini-4-hydrogen",
    "google/gemini-4-carbon",
    "google/gemini-4-argon",
];

fn frozen_catalog() -> Vec<ToolDefinition> {
    let mut tools = vec![ToolDefinition {
        name: REPORT_TOOL.into(),
        description: "Report the requested number.".into(),
        input_schema: json!({"type":"object","properties":{"number":{"type":"integer"}},"required":["number"],"additionalProperties":false}),
        defer_loading: true,
    }];
    tools.extend((0..128).map(|index| ToolDefinition {
        name: format!("mcp__synthetic_archive__retrieve_{index:03}"),
        description:
            "Retrieve an archived synthetic inventory record by its tenant and record key.".into(),
        input_schema: json!({"type":"object","properties":{
            "tenant":{"type":"string","description":"Synthetic inventory tenant key."},
            "record":{"type":"string","description":"Archived synthetic inventory record key."},
            "category":{"type":"string","enum":["blue","green","violet"]},
            "include_metadata":{"type":"boolean"},
            "locale":{"type":"string","description":"Synthetic archive display locale."}
        },"required":["tenant","record"],"additionalProperties":false}),
        defer_loading: true,
    }));
    tools
}

fn initial_request(effort: EffectiveEffort) -> LlmRequest {
    LlmRequest {
        system: vec![SystemContent::new("First use tool_search to discover the optional tool that reports numbers (query: report number). Then use the discovered tool to report the user's number. After its result, reply exactly OK. Do not call any archive tools.")],
        messages: vec![LlmMessage {
            source_message_id: Some("initial-user".into()),
            role: MessageRole::User,
            content: vec![ContentBlock::text("Report number 7 using the optional tool.")],
        }],
        provider_replay: None,
        output_replay: vec![],
        tool_availability: ToolAvailability::all(vec![tool_search_definition()])
            .with_discovery(ToolDiscoveryMode::Local, vec![]).unwrap(),
        max_tokens: 4096,
        effective_effort: effort,
        service_tier: EffectiveServiceTier::Standard,
        telemetry: None,
        cache_key: PromptCacheKey::ephemeral(),
    }
}

async fn complete(service: &dyn LlmService, request: &LlmRequest, streaming: bool) -> LlmResponse {
    if streaming {
        let (tx, mut rx) = tokio::sync::mpsc::channel(128);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let response = service.complete_streaming(request, &tx).await;
        drop(tx);
        drain.await.unwrap();
        response.expect("synthetic shared search streaming request succeeds")
    } else {
        service
            .complete(request)
            .await
            .expect("synthetic shared search request succeeds")
    }
}

fn retain(
    request: &mut LlmRequest,
    response: LlmResponse,
    owner: &str,
    signed: bool,
) -> Vec<ContentBlock> {
    match response.provider_replay {
        Some(ProviderReplayUpdate::ChatCompletions(set)) => {
            let set = set.with_owner_message_id(owner.into());
            let restored: ChatResponseSet =
                serde_json::from_str(&serde_json::to_string(&set).unwrap()).unwrap();
            restored.validate().unwrap();
            assert_eq!(restored.public_content, response.content);
            request.output_replay.push(restored.into());
        }
        None | Some(ProviderReplayUpdate::Clear) if !signed => {}
        _ => panic!("Gemini search and discovered calls retain their private signatures"),
    }
    request.messages.push(LlmMessage {
        source_message_id: Some(owner.into()),
        role: MessageRole::Assistant,
        content: response.content.clone(),
    });
    response.content
}

fn one_call(content: &[ContentBlock], expected: &str) -> (String, serde_json::Value) {
    let calls = content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, name, input } = block {
                assert_eq!(
                    name, expected,
                    "only the expected discovery/function is executable"
                );
                Some((id.clone(), input.clone()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1, "one synthetic tool operation per response");
    calls.into_iter().next().unwrap()
}

fn result(request: &mut LlmRequest, source: &str, call: String, text: String) {
    request.messages.push(LlmMessage {
        source_message_id: Some(source.into()),
        role: MessageRole::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: call,
            content: text,
            images: vec![],
            is_error: false,
        }],
    });
}

async fn qualify(
    service: Arc<dyn LlmService>,
    effort: EffectiveEffort,
    streaming: bool,
    signed: bool,
) {
    assert_eq!(service.tool_discovery_mode(), ToolDiscoveryMode::Local);
    let catalog = frozen_catalog();
    let mut request = initial_request(effort);
    let full_schema_bytes = serde_json::to_vec(&catalog).unwrap().len();
    let initial_schema_bytes = serde_json::to_vec(request.tool_availability.declarations())
        .unwrap()
        .len();
    assert_eq!(request.tool_availability.declarations().len(), 1);
    assert_eq!(
        request.tool_availability.declarations()[0].name,
        TOOL_SEARCH_NAME
    );
    assert!(initial_schema_bytes * 20 < full_schema_bytes);
    let first = complete(service.as_ref(), &request, streaming).await;
    assert!(!first.end_turn);
    // Compare actual initial prompt usage with a conservative byte/4 estimate for just the hidden schemas.
    assert!(first.usage.input_tokens < u64::try_from(full_schema_bytes / 4).unwrap());
    let public = retain(&mut request, first, "search-owner", signed);
    let (call, input) = one_call(&public, TOOL_SEARCH_NAME);
    let query = ToolSearchQuery::parse_input(&input).expect("model emits the shared search schema");
    let selected = search_tools(&catalog, &query);
    assert!(!selected.is_empty());
    assert_eq!(selected[0].name, REPORT_TOOL);
    let text = tool_search_result_text(&selected);
    result(&mut request, "search-result", call.clone(), text);
    request.tool_availability = ToolAvailability::all(
        std::iter::once(tool_search_definition())
            .chain(selected.iter().cloned())
            .collect(),
    )
    .with_discovery(
        ToolDiscoveryMode::Local,
        vec![ToolDiscoveryResult {
            owner_message_id: "search-owner".into(),
            tool_call_id: call,
            result_message_id: "search-result".into(),
            protocol: ToolDiscoveryProtocol::Function,
            tools: selected,
        }],
    )
    .unwrap();
    let function = complete(service.as_ref(), &request, streaming).await;
    assert!(!function.end_turn);
    let public = retain(&mut request, function, "function-owner", signed);
    let (call, input) = one_call(&public, REPORT_TOOL);
    assert_eq!(input, json!({"number":7}));
    result(
        &mut request,
        "function-result",
        call,
        json!({"reported":7}).to_string(),
    );
    let answer = complete(service.as_ref(), &request, streaming).await;
    assert!(answer.end_turn);
    assert!(answer
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Text { text } if text.trim()=="OK")));
}

#[tokio::test]
#[ignore = "uses configured GLM/Gemini gateway credentials and incurs small inference charges"]
async fn configured_shared_search_roundtrips() {
    let config = LlmConfig::from_env(Arc::new(
        phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect(),
    ));
    let registry = ModelRegistry::new(&config);
    let mut models = registry
        .available_models()
        .into_iter()
        .filter(|id| {
            let id = id.to_lowercase();
            id.contains("glm") && (id.contains("5-3") || id.contains("5.3"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        models.len(),
        2,
        "both configured GLM 5.3 and Flash are qualified"
    );
    models.extend(GEMINI_MODELS.map(str::to_owned));
    let mut cases = tokio::task::JoinSet::new();
    for model in models {
        let service = registry
            .get(&model)
            .expect("configured qualification model registered");
        let effort = registry.effective_effort(&model, None);
        for streaming in [false, true] {
            let service = Arc::clone(&service);
            let model = model.clone();
            cases.spawn(async move {
                println!("{model}: streaming={streaming} shared search starting");
                tokio::time::timeout(
                    Duration::from_secs(180),
                    qualify(service, effort, streaming, model.starts_with("google/")),
                )
                .await
                .expect("synthetic shared search case finishes within three minutes");
                println!("{model}: streaming={streaming} shared search passed");
            });
        }
    }
    let mut failures = vec![];
    while let Some(result) = cases.join_next().await {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "{} shared search cases failed: {failures:?}",
        failures.len()
    );
}
