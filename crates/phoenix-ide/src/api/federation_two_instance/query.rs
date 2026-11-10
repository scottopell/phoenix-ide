use super::*;
use phoenix_tools::{Tool, ToolContext};
use serde_json::{json, Value};
use std::sync::Arc;

fn query_tool(instance: &TestInstance) -> Arc<dyn Tool> {
    let db = instance.state.db.clone();
    let global_read =
        crate::api::global_read::GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));
    let send_chat = Arc::new(crate::send_chat_service::SendChatApplicationService::new(
        db.clone(),
        instance.state.runtime.clone(),
    ));
    let handles = Arc::new(crate::tools::BashHandleRegistry::new());
    let load = crate::api::work_scope_load::WorkScopeLoadService::new(
        &handles,
        crate::api::resource_monitor::ResourceMonitor::new(),
        phoenix_terminal::ActiveTerminals::new(),
    );

    crate::coordinator_tools::tools(global_read, send_chat, load, &db)
        .into_iter()
        .find(|tool| tool.name() == "query_database")
        .expect("Coordinator registry must expose query_database")
}

fn test_context() -> ToolContext {
    struct NoLlm;

    impl phoenix_core::llm_service::LlmSelector for NoLlm {
        fn get(
            &self,
            _model_id: &str,
        ) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }

        fn default_service(&self) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
    }

    ToolContext::new_without_filesystem(
        CancellationToken::new(),
        "test-transcript".into(),
        Arc::new(crate::tools::BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(NoLlm),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(crate::tools::TmuxRegistry::new()),
        crate::work_scope::ResourceScopeKey::Coordinator,
        crate::work_scope::ResourceAuthority::Work,
    )
}

async fn insert_sentinel(instance: &TestInstance, title: &str) {
    instance
        .state
        .db
        .create_conversation("query-sentinel", title, "/tmp", true, None, None)
        .await
        .unwrap();
}

async fn run_query(tool: &Arc<dyn Tool>, selector: &str) -> phoenix_tools::ToolOutput {
    tool.run(
        json!({
            "sql": "SELECT title, 7, 3.5, NULL FROM conversations WHERE id = 'query-sentinel'",
            "instance": selector
        }),
        test_context(),
    )
    .await
}

#[tokio::test]
async fn coordinator_inline_research_queries_saved_peer_by_uuid_and_name() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    caller.connect_to(&receiver).await;

    insert_sentinel(&caller, "caller-sentinel").await;
    insert_sentinel(&receiver, "receiver-sentinel").await;

    let caller_id = caller.state.db.instance_id().await.unwrap();
    let receiver_id = receiver.state.db.instance_id().await.unwrap();
    let db = caller.state.db.clone();
    let service =
        crate::api::global_read::GlobalReadService::new(db.clone(), Arc::new(db.fts_retriever()));
    let tool = crate::tools::ToolRegistry::coordinator_research(
        crate::coordinator_tools::research_tools(service, db),
    )
    .find_tool("query_database")
    .expect("inline research registry must expose query_database");

    let receiver_selector = receiver_id.to_string();
    for selector in [receiver_selector.as_str(), "isolated receiver"] {
        let output = run_query(&tool, selector).await;
        assert!(
            output.is_success(),
            "selector {selector:?}: {}",
            output.output()
        );
        let response: Value = serde_json::from_str(output.output()).unwrap();
        assert_eq!(response["destination_instance_id"], receiver_id.to_string());
        assert_eq!(response["caller_instance_id"], caller_id.to_string());
        assert_eq!(
            response["result"]["rows"][0][0]["value"],
            "Receiver Sentinel"
        );
        assert!(!output.output().contains("Caller Sentinel"));
    }

    receiver.stop().await;
    caller.stop().await;
}

#[tokio::test]
async fn unified_query_routes_by_uuid_and_name_with_typed_attribution_and_no_fallback() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    caller.connect_to(&receiver).await;

    insert_sentinel(&caller, "caller-sentinel").await;
    insert_sentinel(&receiver, "receiver-sentinel").await;

    let caller_id = caller.state.db.instance_id().await.unwrap();
    let receiver_id = receiver.state.db.instance_id().await.unwrap();
    let receiver_selector = receiver_id.to_string();
    let tool = query_tool(&caller);

    for selector in [receiver_selector.as_str(), "isolated receiver"] {
        let output = run_query(&tool, selector).await;
        assert!(
            output.is_success(),
            "selector {selector:?} failed: {}",
            output.output()
        );
        let response: Value = serde_json::from_str(output.output()).unwrap();
        assert_eq!(response["destination_instance_id"], receiver_id.to_string());
        assert_eq!(response["caller_instance_id"], caller_id.to_string());
        assert_eq!(
            response["result"]["rows"][0],
            json!([
                {"type": "text", "value": "Receiver Sentinel"},
                {"type": "integer", "value": 7},
                {"type": "real", "value": {"kind": "finite", "value": 3.5}},
                {"type": "null"}
            ])
        );
    }

    receiver.stop().await;
    let unavailable = run_query(&tool, &receiver_selector).await;
    assert!(
        !unavailable.is_success(),
        "unavailable remote query fell back locally: {}",
        unavailable.output()
    );
    assert!(!unavailable.output().contains("Caller Sentinel"));

    caller.stop().await;
}

struct ChildQuerySchemaRecorder {
    parent_id: String,
    requests: std::sync::Mutex<Vec<phoenix_llm::LlmRequest>>,
    parent_requests: std::sync::atomic::AtomicUsize,
    child_request_seen: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl phoenix_llm::LlmService for ChildQuerySchemaRecorder {
    async fn complete(
        &self,
        request: &phoenix_llm::LlmRequest,
    ) -> Result<phoenix_llm::LlmResponse, phoenix_llm::LlmError> {
        use phoenix_llm::ContentBlock;

        self.requests.lock().unwrap().push(request.clone());
        let conversation_id = &request.telemetry.as_ref().unwrap().conversation_id;
        if conversation_id != &self.parent_id {
            self.child_request_seen.notify_one();
        }
        let content = if conversation_id == &self.parent_id
            && self
                .parent_requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 0
        {
            ContentBlock::tool_use(
                "spawn-query-child",
                "spawn_agents",
                json!({"tasks": [{"task": "Inspect the query_database tool schema."}]}),
            )
        } else if conversation_id == &self.parent_id {
            ContentBlock::text("Finished")
        } else {
            ContentBlock::tool_use(
                "finish-query-child",
                "submit_result",
                json!({"result": "Child query schema inspected."}),
            )
        };
        let end_turn = matches!(content, ContentBlock::Text { .. });
        Ok(phoenix_llm::LlmResponse::non_streaming(
            vec![content],
            end_turn,
            phoenix_llm::Usage::default(),
        ))
    }

    fn model_id(&self) -> &'static str {
        "claude-sonnet-5"
    }
}

async fn child_query_database_schema(parent_kind: &str) -> Option<Value> {
    let instance = TestInstance::start().await;
    let parent = if parent_kind == "Coordinator" {
        instance
            .state
            .db
            .get_or_create_coordinator(
                Some("claude-sonnet-5"),
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap()
    } else {
        instance
            .state
            .db
            .create_conversation(
                "persisted-query-parent",
                "Persisted query parent",
                "/tmp",
                true,
                None,
                Some("claude-sonnet-5"),
            )
            .await
            .unwrap()
    };
    let recorder = Arc::new(ChildQuerySchemaRecorder {
        parent_id: parent.id.clone(),
        requests: std::sync::Mutex::new(Vec::new()),
        parent_requests: std::sync::atomic::AtomicUsize::new(0),
        child_request_seen: tokio::sync::Notify::new(),
    });
    let manager = Arc::new(crate::runtime::RuntimeManager::new(
        instance.state.db.clone(),
        Arc::new(phoenix_llm::ModelRegistry::for_test_with_sonnet(
            recorder.clone(),
        )),
        instance.state.platform.clone(),
        instance.state.mcp_manager.clone(),
        None,
    ));
    manager.start_sub_agent_handler().await;
    let handle = manager.get_or_create(&parent.id).await.unwrap();
    manager
        .send_event(
            &parent.id,
            crate::state_machine::event::Event::UserMessage {
                text: "Inspect the child query schema".into(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: format!("{parent_kind}-query-schema-user"),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        recorder.child_request_seen.notified(),
    )
    .await
    .expect("RuntimeManager dispatched the child LLM request");
    let mut states = handle.state_rx.clone();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        states
            .wait_for(|state| matches!(state, crate::state_machine::state::ConvState::Idle))
            .await
    })
    .await
    .expect("parent and child runtime requests finish")
    .expect("runtime state remains available");
    let requests = recorder.requests.lock().unwrap().clone();
    let child = requests
        .iter()
        .find(|request| {
            request
                .telemetry
                .as_ref()
                .is_some_and(|telemetry| telemetry.conversation_id != parent.id)
        })
        .expect("RuntimeManager dispatched a child LLM request");
    let schema = child
        .tool_availability
        .declarations()
        .iter()
        .find(|tool| tool.name == "query_database")
        .map(|tool| tool.input_schema.clone());
    manager.begin_process_shutdown();
    manager.drain_process_shutdown().await;
    instance.stop().await;
    schema
}

#[tokio::test]
async fn persisted_parent_eligibility_controls_child_remote_query_selector() {
    let coordinator_schema = child_query_database_schema("Coordinator")
        .await
        .expect("Coordinator child query_database declaration");
    assert!(
        coordinator_schema["properties"].get("instance").is_some(),
        "Coordinator child query_database schema must expose instance: {coordinator_schema}"
    );

    let ordinary_schema = child_query_database_schema("ordinary").await;
    assert!(
        ordinary_schema.is_none(),
        "ordinary Explore child must not receive Coordinator query_database: {ordinary_schema:?}"
    );
}
