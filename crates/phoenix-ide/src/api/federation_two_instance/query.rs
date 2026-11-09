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
