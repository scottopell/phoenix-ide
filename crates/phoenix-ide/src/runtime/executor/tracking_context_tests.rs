use super::*;
use std::path::PathBuf;
use crate::runtime::testing::MockLlmClient;
use crate::runtime::traits::{DatabaseStorage, ToolRegistryExecutor};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_db::coordinator_tracking::{
    TrackingKind, TrackingMutation, TrackingRecord, TrackingResumption,
};

type Runtime = ConversationRuntime<DatabaseStorage, Arc<MockLlmClient>, ToolRegistryExecutor>;
async fn runtime(
    manager: &Arc<crate::runtime::RuntimeManager>,
    db: &crate::db::Database,
    id: &str,
    llm: Arc<MockLlmClient>,
) -> Runtime {
    let mut context = ConvContext::new(id, PathBuf::from("/tmp"), "test-model", 200_000, 16_384);
    context.is_coordinator = true;
    let (tx, rx) = mpsc::channel(16);
    let max = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let registry = manager.coordinator_tool_registry(id, None).await.unwrap();
    ConversationRuntime::new(
        context,
        ConvState::LlmRequesting { attempt: 1 },
        DatabaseStorage::new(db.clone()),
        llm,
        ToolRegistryExecutor::builtin_only(registry, Arc::from([])),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        rx,
        tx,
        SseBroadcaster::new(128, max),
    )
}
async fn save(db: &crate::db::Database, source: &str, kind: TrackingKind, title: &str) -> String {
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: uuid::Uuid::new_v4().to_string(),
    };
    let mutation = TrackingMutation::Record {
        record: TrackingRecord {
            kind,
            title: title.into(),
            note: "Preserved user intent; not fresh worker status".into(),
            resumption: "Return to the exact question without commissioning work".into(),
            references: vec![],
            evidence: vec![],
        },
    };
    db.add_message(
        &call.message_id,
        source,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call.tool_use_id.clone(),
            name: "tracking".into(),
            input: serde_json::json!({"request":mutation}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    db.mutate_coordinator_tracking(source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap()
        .item_id
}
async fn dispatch(runtime: &mut Runtime, llm: &MockLlmClient) {
    llm.queue_response(phoenix_llm::LlmResponse {
        provider_replay: None,
        content: vec![ContentBlock::text("Current question answered")],
        end_turn: true,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    });
    runtime.execute_effect(Effect::RequestLlm).await.unwrap();
    runtime.llm_task_handle.take().unwrap().await.unwrap();
}
fn request_text(request: &phoenix_llm::LlmRequest) -> String {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[tokio::test]
async fn tracking_production_registry_prompt_prefix_and_real_continuation_recover_manual_intent() {
    let (manager, db, source) = crate::conversation_action_tools::tests::native_runtime().await;
    let request = save(
        &db,
        &source,
        TrackingKind::RequestedWork,
        "Finish user-requested keyboard fix",
    )
    .await;
    let bookmark = save(
        &db,
        &source,
        TrackingKind::Bookmark,
        "Why did the design choose this boundary?",
    )
    .await;
    let llm = Arc::new(MockLlmClient::new("test-model"));
    let mut current = runtime(&manager, &db, &source, llm.clone()).await;
    dispatch(&mut current, &llm).await;
    let first = llm.recorded_requests().last().unwrap().clone();
    assert!(first.tool_availability.declarations().iter().any(|tool| tool.name == "tracking"));
    assert!(request_text(&first).contains("Finish user-requested keyboard fix"));
    assert!(request_text(&first).contains("Why did the design choose this boundary?"));
    let original = db.get_messages(&source).await.unwrap();
    dispatch(&mut current, &llm).await;
    let after_loop=db.get_messages(&source).await.unwrap();
    assert_eq!(after_loop.len(),original.len(),"tool/model loops append no unchanged snapshot");
    for (earlier,current) in original.iter().zip(&after_loop){assert_eq!(earlier.message_id,current.message_id);assert_eq!(earlier.content,current.content);}
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: "append-new-pause".into(),
    };
    let mutation = TrackingMutation::AppendEvidence {
        id: request.clone(),
        expected_revision: 1,
        evidence: vec![phoenix_db::coordinator_tracking::TrackingEvidence {
            instance_id: db.instance_id().await.unwrap().to_string(),
            transcript_id: source.clone(),
            message_id: "later-pause".into(),
            quotation: "Do not proceed until I lift the pause".into(),
        }],
    };
    db.add_message(
        &call.message_id,
        &source,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call.tool_use_id.clone(),
            name: "tracking".into(),
            input: serde_json::json!({"request":mutation}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    db.mutate_coordinator_tracking(&source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    let sequence = current.broadcast_tx.next_seq();
    db.add_message_with_seq_and_origin(
        "new-normal-input",
        &source,
        sequence,
        &MessageContent::User(crate::db::UserContent::new(
            "Let's return to this after our detour",
        )),
        None,
        None,
        &phoenix_core::domain::db_schema::InputOrigin::UserApi,
    )
    .await
    .unwrap();
    current.tracking_observation_pending = true;
    dispatch(&mut current, &llm).await;
    let changed = llm.recorded_requests().last().unwrap().clone();
    assert_eq!(first.system[0].text, changed.system[0].text);
    assert_eq!(first.system[0].cache, changed.system[0].cache);
    for (earlier,current) in first.messages.iter().zip(&changed.messages){
        assert_eq!(earlier.role,current.role);assert_eq!(earlier.source_message_id,current.source_message_id);assert_eq!(earlier.content,current.content);
    }
    assert!(changed.messages.len()>=first.messages.len());
    assert!(request_text(&changed).contains("\"review_required\":true"));
    assert!(matches!(
        db.read_coordinator_tracking(&source, &request, 0)
            .await
            .unwrap()
            .item
            .resumption,
        TrackingResumption::ReviewRequired { .. }
    ));
    db.update_conversation_state(
        &source,
        &ConvState::ContextExhausted {
            summary: "Detour context; original requested keyboard work and design bookmark omitted"
                .into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&source).await.unwrap() {
        crate::db::ContinueOutcome::Created(next) => next,
        other => panic!("{other:?}"),
    };
    let mut next = runtime(&manager, &db, &successor.id, llm.clone()).await;
    dispatch(&mut next, &llm).await;
    let recovered = llm.recorded_requests().last().unwrap().clone();
    assert_eq!(first.system[0].text, recovered.system[0].text);
    assert!(request_text(&recovered).contains("Finish user-requested keyboard fix"));
    assert!(request_text(&recovered).contains("Why did the design choose this boundary?"));
    assert_eq!(
        db.read_coordinator_tracking(&successor.id, &bookmark, 0)
            .await
            .unwrap()
            .item
            .kind,
        TrackingKind::Bookmark
    );
    assert_eq!(
        db.list_coordinator_tracking(&successor.id, None, false)
            .await
            .unwrap()
            .review_required_count,
        1
    );
    assert!(recovered.system[0]
        .text
        .contains("Goal/turn/PR completion does not automatically resolve broader intent"));
}
