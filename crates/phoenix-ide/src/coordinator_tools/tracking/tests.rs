use super::*;
use crate::conversation_action_tools::tests::native_runtime;
use phoenix_core::domain::{db_schema::MessageContent, llm_types::ContentBlock};
fn context(source: &str, with_call: bool) -> ToolContext {
    struct Offline;
    impl phoenix_core::llm_service::LlmSelector for Offline {
        fn get(&self, _: &str) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
        fn default_service(&self) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
    }
    let ctx = ToolContext::new_without_filesystem(
        tokio_util::sync::CancellationToken::new(),
        source.into(),
        Arc::new(phoenix_tools::BrowserSessionManager::default()),
        Arc::new(phoenix_tools::BashHandleRegistry::new()),
        Arc::new(Offline),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(phoenix_tools::TmuxRegistry::new()),
        phoenix_core::work_scope::ResourceScopeKey::Coordinator,
        phoenix_core::work_scope::ResourceAuthority::Work,
    );
    if with_call {
        ctx.with_svg_assistant_message_id("tracking-message")
            .with_tool_use_id("tracking-call")
    } else {
        ctx
    }
}
#[tokio::test]
async fn tracking_native_registry_and_closed_requests_use_real_application_service() {
    let (runtime, db, source) = native_runtime().await;
    let registry = runtime
        .coordinator_tool_registry(&source, None)
        .await
        .unwrap();
    assert!(registry
        .definitions()
        .iter()
        .any(|definition| definition.name == "tracking"));
    for registry in [
        phoenix_tools::ToolRegistry::direct(Vec::new()),
        phoenix_tools::ToolRegistry::for_subagent_work(),
        phoenix_tools::ToolRegistry::for_subagent_explore_no_sandbox(),
    ] {
        assert!(!registry
            .definitions()
            .iter()
            .any(|definition| definition.name == "tracking"));
    }
    let tool = TrackingTool { runtime };
    let schema = tool.input_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema.get("oneOf").is_none());
    assert_eq!(schema["additionalProperties"], false);
    let branches = schema["properties"]["request"]["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 7);
    for branch in branches {
        assert_eq!(branch["additionalProperties"], false);
    }
    for invalid in [
        json!({"instance":"remote","request":{"op":"list"}}),
        json!({"request":{"op":"list","after":42}}),
        json!({"request":{"op":"read","id":"x","source":"spoof"}}),
    ] {
        assert!(tool
            .run_inner(invalid, context(&source, true))
            .await
            .is_err());
    }
    assert!(tool
        .run_inner(json!({"request":{"op":"list"}}), context(&source, false))
        .await
        .is_err());
    let record = json!({"op":"record","record":{"kind":"bookmark","title":"Why the earlier design mattered","note":"Useful question, not a fresh commission","resumption":"Explain the stopping point when the user returns","references":[],"evidence":[]}});
    db.add_message(
        "tracking-message",
        &source,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: "tracking-call".into(),
            name: "tracking".into(),
            input: json!({"request":record}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    let receipt = tool
        .run_inner(json!({"request":record}), context(&source, true))
        .await
        .unwrap();
    assert_eq!(receipt["outcome"], "applied");
    let detail = tool
        .run_inner(
            json!({"request":{"op":"read","id":receipt["item_id"]}}),
            context(&source, true),
        )
        .await
        .unwrap();
    assert_eq!(detail["item"]["kind"], "bookmark");
    assert_eq!(detail["item"]["resumption"]["state"], "reviewed");
    assert!(detail["item"]["resumption"]["resumption"]
        .as_str()
        .unwrap()
        .contains("stopping point"));
    let replay = tool
        .run_inner(json!({"request":record}), context(&source, true))
        .await
        .unwrap();
    assert_eq!(replay["outcome"], "replayed");
    let ordinary = db
        .create_conversation("tracking-ordinary", "ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    assert!(tool
        .run_inner(
            json!({"request":{"op":"list"}}),
            context(&ordinary.id, true)
        )
        .await
        .is_err());
}
