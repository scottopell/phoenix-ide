use super::*;
use phoenix_core::domain::tool_discovery::{
    tool_search_definition, ToolDiscoveryMode, ToolDiscoveryProtocol, TOOL_SEARCH_NAME,
};

struct DiscoveryClient {
    inner: MockLlmClient,
    mode: ToolDiscoveryMode,
}

#[async_trait]
impl LlmClient for DiscoveryClient {
    fn tool_discovery_mode(&self) -> Result<ToolDiscoveryMode, LlmError> {
        Ok(self.mode)
    }
    fn freeze_for_request(self: Arc<Self>) -> Result<Arc<dyn LlmClient>, LlmError> {
        Ok(self)
    }
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.inner.complete(request).await
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
}

struct SearchCatalog(DeferredMcpCatalog);

#[async_trait]
impl ToolExecutor for SearchCatalog {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        if call.name() == TOOL_SEARCH_NAME {
            let (_, input, _) = call.into_bound_parts();
            Some(phoenix_tools::Tool::run(&phoenix_tools::ToolSearch, input, ctx).await)
        } else {
            self.0.execute(call, ctx).await
        }
    }
    async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition> {
        let mut definitions = self.0.definitions().await;
        definitions.push(tool_search_definition());
        definitions
    }
}

fn queued_search(mode: ToolDiscoveryMode) -> LlmResponse {
    let public = vec![ContentBlock::tool_use(
        "search-call",
        TOOL_SEARCH_NAME,
        serde_json::json!({"query":"slack thread"}),
    )];
    let mut response =
        LlmResponse::non_streaming(public.clone(), false, phoenix_llm::Usage::default());
    if mode == ToolDiscoveryMode::OpenAiClient {
        response.provider_replay = Some(ProviderReplayUpdate::Responses(
            phoenix_core::domain::responses_replay::ResponsesResponseSet {
                response_id: "native-search-response".into(),
                model: "gpt-6.2-sol".into(),
                owner_message_id: String::new(),
                public_content: public,
                output_items: vec![
                    serde_json::json!({"type":"tool_search_call","id":"search-item","call_id":"search-call","execution":"client","status":"completed","arguments":{"query":"slack thread"}}),
                ],
            },
        ));
    }
    response
}

async fn accept_next_tool(runtime: &mut TestRuntime<DiscoveryClient, SearchCatalog>) {
    let (_, outcome) = runtime.llm_outcome_rx.try_recv().unwrap();
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
    runtime
        .llm_task_handle
        .take()
        .expect("follow-up provider request")
        .await
        .unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn local_and_native_client_search_load_then_execute_and_survive_reopen() {
    for mode in [ToolDiscoveryMode::Local, ToolDiscoveryMode::OpenAiClient] {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("discovery.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let client = Arc::new(DiscoveryClient {
            inner: MockLlmClient::new("gpt-6.2-sol"),
            mode,
        });
        db.create_conversation(
            CONVERSATION,
            CONVERSATION,
            directory.path().to_str().unwrap(),
            true,
            None,
            Some(client.model_id()),
        )
        .await
        .unwrap();
        db.add_message(
            "discovery-user",
            CONVERSATION,
            &MessageContent::user("Find the Slack tool and read the thread"),
            None,
            None,
        )
        .await
        .unwrap();
        client.inner.queue_response(queued_search(mode));
        client.inner.queue_response(LlmResponse::non_streaming(
            vec![ContentBlock::tool_use(
                "mcp-call",
                MCP_TOOL,
                serde_json::json!({"thread":"fixture"}),
            )],
            false,
            phoenix_llm::Usage::default(),
        ));
        let tools = Arc::new(SearchCatalog(DeferredMcpCatalog::new(true)));
        let mut first = runtime(db.clone(), directory.path(), client.clone(), tools.clone());
        dispatch(&mut first).await;
        let initial = client.inner.recorded_requests().remove(0);
        assert_eq!(initial.tool_availability.discovery_mode(), mode);
        assert!(initial.tool_availability.is_callable(TOOL_SEARCH_NAME));
        assert!(!initial
            .tool_availability
            .declarations()
            .iter()
            .any(|tool| tool.name == MCP_TOOL));
        assert!(!db
            .load_tool_admission_policy(CONVERSATION)
            .await
            .unwrap()
            .is_callable(MCP_TOOL));
        assert!(db
            .load_tool_search_catalog(CONVERSATION)
            .await
            .unwrap()
            .iter()
            .any(|tool| tool.name == MCP_TOOL));
        accept_next_tool(&mut first).await;
        let next = client.inner.recorded_requests().remove(1);
        assert!(next.tool_availability.is_callable(MCP_TOOL));
        assert!(next
            .tool_availability
            .declarations()
            .iter()
            .any(|tool| tool.name == MCP_TOOL));
        let [selection] = next.tool_availability.discovery_results() else {
            panic!("one committed discovery result")
        };
        assert_eq!(selection.tool_call_id, "search-call");
        assert_eq!(selection.tools.len(), 1);
        assert_eq!(
            selection.protocol,
            if mode == ToolDiscoveryMode::Local {
                ToolDiscoveryProtocol::Function
            } else {
                ToolDiscoveryProtocol::OpenAiClient
            }
        );
        assert!(tools.0.recorded_executions().is_empty());
        accept_next_tool(&mut first).await;
        assert_eq!(tools.0.recorded_executions().len(), 1);
        let result_id = selection.result_message_id.clone();
        drop(first);
        drop(tools);
        db.pool().close().await;
        drop(db);
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        let mut resumed = runtime(
            reopened.clone(),
            directory.path(),
            client.clone(),
            Arc::new(SearchCatalog(DeferredMcpCatalog::new(true))),
        );
        dispatch(&mut resumed).await;
        resumed.state = ConvState::LlmRequesting { attempt: 3 };
        dispatch(&mut resumed).await;
        for request in &client.inner.recorded_requests()[3..] {
            assert!(request.tool_availability.is_callable(MCP_TOOL));
            assert!(request
                .tool_availability
                .discovery_result(&result_id, "search-call")
                .is_some());
        }
        drop(resumed);
        reopened.pool().close().await;
    }
}

#[test]
fn discovery_schema_accounting_tracks_wire_placement_and_hidden_catalog_savings() {
    let mut deferred = tool_search_definition();
    deferred.name = "mcp__large__read".into();
    deferred.defer_loading = true;
    deferred.input_schema = serde_json::json!({"description":"long schema ".repeat(5000)});
    let eager = phoenix_core::domain::tool_availability::ToolAvailability::all(vec![
        tool_search_definition(),
        deferred.clone(),
    ]);
    let local = fresh_discovery_availability(
        vec![tool_search_definition(), deferred.clone()],
        ToolDiscoveryMode::Local,
    );
    let hosted = eager
        .clone()
        .with_discovery(ToolDiscoveryMode::OpenAiHosted, vec![])
        .unwrap();
    let selected = phoenix_core::domain::tool_discovery::ToolDiscoveryResult {
        owner_message_id: "o".into(),
        tool_call_id: "c".into(),
        result_message_id: "r".into(),
        protocol: ToolDiscoveryProtocol::OpenAiClient,
        tools: vec![deferred],
    };
    let client = local
        .clone()
        .with_discovery(ToolDiscoveryMode::OpenAiClient, vec![selected])
        .unwrap();
    let baseline = estimate_fixed_request_tokens("system", &eager);
    assert!(estimate_fixed_request_tokens("system", &local) * 10 < baseline);
    assert!(estimate_fixed_request_tokens("system", &hosted) * 10 < baseline);
    assert!(estimate_fixed_request_tokens("system", &client) > baseline / 2);
}
