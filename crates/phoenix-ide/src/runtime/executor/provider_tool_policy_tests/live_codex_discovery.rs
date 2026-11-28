//! Opt-in bridge qualification through the runtime, real tools, and `SQLite`.

use super::*;
use phoenix_core::domain::tool_discovery::{
    native_client_search_call, search_tools, tool_search_definition, tool_search_result_text,
    ToolDiscoveryMode, ToolDiscoveryProtocol, ToolSearchQuery, TOOL_SEARCH_NAME,
};
use phoenix_llm::{LlmConfig, LlmService, ToolDefinition};

const MODEL: &str = "gpt-6.2-sol";
const REPORT_TOOL: &str = "mcp__numbers__report_number";

fn schema_proof() -> &'static str {
    static PROOF: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PROOF.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

struct LiveCodexClient {
    service: Arc<dyn LlmService>,
    requests: Mutex<Vec<LlmRequest>>,
}

#[async_trait]
impl LlmClient for LiveCodexClient {
    fn tool_discovery_mode(&self) -> Result<ToolDiscoveryMode, LlmError> {
        Ok(self.service.tool_discovery_mode())
    }

    fn freeze_for_request(self: Arc<Self>) -> Result<Arc<dyn LlmClient>, LlmError> {
        Ok(self)
    }

    fn continuation_route_key(&self) -> Result<String, LlmError> {
        Ok(self.service.continuation_route_key())
    }

    async fn count_input(
        &self,
        request: &LlmRequest,
    ) -> Result<Option<phoenix_llm::ProviderInputCount>, LlmError> {
        self.service.count_input(request).await
    }

    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        self.service.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunks: &mpsc::Sender<phoenix_llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        self.service.complete_streaming(request, chunks).await
    }

    fn model_id(&self) -> &str {
        self.service.model_id()
    }
}

#[derive(Default)]
struct SyntheticNumbers {
    searches: Mutex<Vec<serde_json::Value>>,
    executions: Mutex<Vec<serde_json::Value>>,
}

fn report_definition() -> ToolDefinition {
    ToolDefinition {
        name: REPORT_TOOL.into(),
        description: "Report the requested number.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{
            "number":{"type":"integer"},
            "proof":{"type":"string","const":schema_proof(),"description":"Required verification value; use exactly the const from this schema."}
        },"required":["number","proof"],"additionalProperties":false}),
        defer_loading: true,
    }
}

#[async_trait]
impl ToolExecutor for SyntheticNumbers {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        let name = call.name().to_owned();
        let (_, input, _) = call.into_bound_parts();
        if name == TOOL_SEARCH_NAME {
            let query = ToolSearchQuery::parse_input(&input).unwrap();
            let catalog = ctx.tool_search_catalog();
            assert_eq!(catalog.iter().filter(|tool| tool.defer_loading).count(), 65);
            let selected = search_tools(catalog, &query);
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].name, REPORT_TOOL);
            let expected = tool_search_result_text(&selected);
            self.searches.lock().unwrap().push(input.clone());
            let output = phoenix_tools::Tool::run(&phoenix_tools::ToolSearch, input, ctx).await;
            assert!(output.is_success());
            assert_eq!(output.output(), expected);
            Some(output)
        } else if name == REPORT_TOOL {
            assert_eq!(
                input,
                serde_json::json!({"number":7,"proof":schema_proof()})
            );
            self.executions.lock().unwrap().push(input);
            Some(ToolOutput::success("{\"reported\":7}"))
        } else {
            None
        }
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = vec![tool_search_definition(), report_definition()];
        definitions.extend((0..64).map(|index| ToolDefinition {
            name: format!("mcp__synthetic_archive__retrieve_{index:03}"),
            description: "Retrieve a synthetic archive record by its key.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"record_key":{"type":"string"}},"required":["record_key"],"additionalProperties":false}),
            defer_loading: true,
        }));
        definitions
    }
}

type LiveRuntime = TestRuntime<LiveCodexClient, SyntheticNumbers>;

async fn wait_provider(runtime: &mut LiveRuntime) {
    tokio::time::timeout(
        Duration::from_secs(180),
        runtime.llm_task_handle.take().expect("live provider task"),
    )
    .await
    .expect("synthetic bridge request completes within three minutes")
    .unwrap();
}

async fn accept_tool(runtime: &mut LiveRuntime) {
    let (_, outcome) = runtime.llm_outcome_rx.try_recv().unwrap();
    assert!(
        matches!(&outcome, LlmOutcome::Response { end_turn: false, tool_calls, .. } if tool_calls.len()==1)
    );
    Box::pin(runtime.process_outcome(EffectOutcome::Llm(outcome)))
        .await
        .unwrap();
    assert!(runtime
        .observe_tool_preparation_for_test()
        .await
        .unwrap()
        .is_none());
    let (_, outcome) =
        tokio::time::timeout(Duration::from_secs(30), runtime.tool_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
    Box::pin(runtime.process_outcome(EffectOutcome::Tool(outcome)))
        .await
        .unwrap();
    wait_provider(runtime).await;
}

#[tokio::test]
#[ignore = "requires Phoenix-native Codex login and incurs small inference charges"]
#[allow(clippy::too_many_lines)]
async fn codex_native_discovery_runtime_sqlite_roundtrip() {
    let config = LlmConfig::from_env(Arc::new(
        phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect(),
    ));
    assert!(
        config.use_codex_auth,
        "Sign in with Codex from Phoenix first"
    );
    let registry = ModelRegistry::new(&config);
    let service = registry.get(MODEL).expect("built-in Codex model available");
    assert!(service.uses_codex_bridge());
    assert_eq!(
        service.tool_discovery_mode(),
        ToolDiscoveryMode::OpenAiClient
    );
    let client = Arc::new(LiveCodexClient {
        service,
        requests: Mutex::new(vec![]),
    });
    let tools = Arc::new(SyntheticNumbers::default());
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("live-codex-discovery.db");
    let db = Database::open_migrated_for_test(path.to_str().unwrap())
        .await
        .unwrap();
    db.create_conversation(
        CONVERSATION,
        CONVERSATION,
        directory.path().to_str().unwrap(),
        true,
        None,
        Some(MODEL),
    )
    .await
    .unwrap();
    db.add_message("synthetic-user", CONVERSATION, &MessageContent::user(
        "Synthetic tool protocol check. First use tool_search with query report_number and limit 1 to discover the optional number reporting tool. Then call the discovered tool with number 7. After its successful result reply exactly OK. Do not use any archive tools."
    ), None, None).await.unwrap();
    let mut runtime = runtime(db.clone(), directory.path(), client.clone(), tools.clone());
    runtime.execute_effect(Effect::RequestLlm).await.unwrap();
    wait_provider(&mut runtime).await;
    {
        let requests = client.requests.lock().unwrap();
        let initial = &requests[0];
        assert_eq!(
            initial.tool_availability.discovery_mode(),
            ToolDiscoveryMode::OpenAiClient
        );
        assert!(initial
            .tool_availability
            .declarations()
            .iter()
            .all(|tool| !tool.defer_loading));
        assert_eq!(
            initial
                .tool_availability
                .declarations()
                .iter()
                .filter(|tool| tool.name == TOOL_SEARCH_NAME)
                .count(),
            1
        );
        assert!(!initial
            .tool_availability
            .declarations()
            .iter()
            .any(|tool| tool.name == REPORT_TOOL));
        assert!(!format!("{initial:?}").contains(schema_proof()));
        let metrics = initial
            .telemetry
            .as_ref()
            .unwrap()
            .attempt_capture
            .finalized()
            .unwrap();
        assert_eq!(metrics.transport, phoenix_llm::LlmTransport::Websocket);
    }
    assert_eq!(
        db.load_tool_search_catalog(CONVERSATION)
            .await
            .unwrap()
            .iter()
            .filter(|tool| tool.defer_loading)
            .count(),
        65
    );
    assert!(!db
        .load_tool_admission_policy(CONVERSATION)
        .await
        .unwrap()
        .is_callable(REPORT_TOOL));
    accept_tool(&mut runtime).await;
    assert_eq!(tools.searches.lock().unwrap().len(), 1);
    assert!(tools.executions.lock().unwrap().is_empty());
    let executed_search = tools.searches.lock().unwrap()[0].clone();
    let selection = {
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let next = &requests[1];
        let [selection] = next.tool_availability.discovery_results() else {
            panic!("one durable native selection");
        };
        assert_eq!(selection.protocol, ToolDiscoveryProtocol::OpenAiClient);
        assert_eq!(selection.tools.len(), 1);
        assert_eq!(selection.tools[0].name, REPORT_TOOL);
        assert_eq!(
            selection.tools[0].input_schema,
            report_definition().input_schema
        );
        assert!(next.tool_availability.is_callable(REPORT_TOOL));
        assert!(next
            .output_replay
            .iter()
            .filter_map(phoenix_core::domain::responses_replay::ProviderResponseSet::as_responses)
            .any(|set| set.owner_message_id == selection.owner_message_id
                && set
                    .output_items
                    .iter()
                    .any(|item| native_client_search_call(item)
                        .unwrap()
                        .is_some_and(|call| call.call_id == selection.tool_call_id
                            && call.arguments == executed_search))));
        selection.clone()
    };
    let saved = db.get_messages(CONVERSATION).await.unwrap();
    assert!(saved.iter().any(|message| message.message_id==selection.owner_message_id && matches!(&message.content, MessageContent::Agent(blocks) if blocks.iter().any(|block| matches!(block,ContentBlock::ToolUse { id, name, .. } if id==&selection.tool_call_id && name==TOOL_SEARCH_NAME)))));
    assert!(saved.iter().any(|message| message.message_id==selection.result_message_id && matches!(&message.content,MessageContent::Tool(result) if !result.is_error && result.tool_use_id==selection.tool_call_id && result.content()==tool_search_result_text(&selection.tools))));
    accept_tool(&mut runtime).await;
    assert_eq!(
        &*tools.executions.lock().unwrap(),
        &[serde_json::json!({"number":7,"proof":schema_proof()})]
    );
    let (_, final_outcome) = runtime.llm_outcome_rx.try_recv().unwrap();
    assert!(
        matches!(&final_outcome,LlmOutcome::Response { end_turn:true, content, tool_calls, .. } if tool_calls.is_empty() && content.iter().any(|block| matches!(block,ContentBlock::Text { text } if text.trim()=="OK")))
    );
    Box::pin(runtime.process_outcome(EffectOutcome::Llm(final_outcome)))
        .await
        .unwrap();
    assert!(db
        .load_output_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_empty());
    assert!(db.get_messages(CONVERSATION).await.unwrap().iter().any(|message| matches!(&message.content, MessageContent::Agent(blocks) if blocks.iter().any(|block| matches!(block,ContentBlock::Text { text } if text.trim()=="OK")))));
    {
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|request| request
            .telemetry
            .as_ref()
            .unwrap()
            .attempt_capture
            .finalized()
            .unwrap()
            .transport
            == phoenix_llm::LlmTransport::Websocket));
    }
    drop(runtime);
    db.pool().close().await;
    println!("Codex native search: actual WebSocket runtime, selector, SQLite ownership and final response passed");
}
