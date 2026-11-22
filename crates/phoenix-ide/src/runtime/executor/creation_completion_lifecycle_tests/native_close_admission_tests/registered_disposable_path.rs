use super::*;

#[tokio::test]
async fn registered_close_captures_and_retires_disposable_empty_work_scope() {
    use crate::coordinator_tools;
    use crate::send_chat_service::SendChatApplicationService;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let retriever = Arc::new(fixture.db.fts_retriever());
    let read_service =
        crate::api::global_read::GlobalReadService::new(fixture.db.clone(), retriever);
    let send_chat = Arc::new(SendChatApplicationService::new(
        fixture.db.clone(),
        Arc::clone(&backend),
    ));
    let registered_tools = coordinator_tools::tools(
        read_service,
        send_chat,
        crate::api::work_scope_load::WorkScopeLoadService::new(
            backend.bash_handles(),
            crate::api::resource_monitor::ResourceMonitor::new(),
            backend.terminals.clone(),
        ),
        &fixture.db,
    );
    let tool = registered_tools
        .into_iter()
        .find(|tool| tool.name() == "close_conversation")
        .expect("the coordinator registration must include close_conversation");

    let (event_tx, event_rx) = mpsc::channel(32);
    let mut actor = ConversationRuntime::new(
        ConvContext::coordinator(&fixture.source_transcript_id, "test-model", 200_000, 16_384),
        fixture.state.clone(),
        Arc::new(crate::runtime::traits::DatabaseStorage::new(
            fixture.db.clone(),
        )),
        Arc::new(MockLlmClient::new("test-model")),
        Arc::new(MockToolExecutor::new()),
        backend.browser_sessions().clone(),
        backend.bash_handles().clone(),
        backend.tmux_registry().clone(),
        Arc::new(ModelRegistry::new_empty()),
        backend.terminals.clone(),
        event_rx,
        event_tx.clone(),
        crate::runtime::SseBroadcaster::new(128, 0),
    )
    .with_fatal_local_authority_fence(Arc::clone(&backend.fatal_local_authority_fence))
    .with_native_task_admission_database(fixture.db.clone());

    let (sender, receiver) = mpsc::channel(4);
    let (watch_tx, state_rx) = tokio::sync::watch::channel(fixture.state.clone());
    let (startup, started) = oneshot::channel();
    actor = actor
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver)
        .with_startup_llm_recovery_ack(startup);
    backend.runtimes.write().await.insert(
        fixture.source_transcript_id.clone(),
        crate::runtime::ConversationHandle {
            event_tx: event_tx.clone(),
            acknowledged_event_tx: sender,
            handoff_review_tx: None,
            turn_trigger: crate::runtime::TurnTriggerSlot::default(),
            broadcast_tx: crate::runtime::SseBroadcaster::new(128, 0),
            identity: Arc::new(()),
            state_rx,
            startup_llm_recovery_ack: Arc::new(Mutex::new(None)),
        },
    );
    let running = tokio::spawn(actor.run());
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .expect("source actor must finish startup")
        .unwrap()
        .unwrap();

    let context = crate::tools::ToolContext::new(
        CancellationToken::new(),
        fixture.source_transcript_id.clone(),
        PathBuf::from("/tmp"),
        backend.browser_sessions().clone(),
        backend.bash_handles().clone(),
        Arc::new(ModelRegistry::new_empty()),
        backend.terminals.clone(),
        backend.tmux_registry().clone(),
        None,
        phoenix_core::work_scope::WorkScopeId::parse("close-source-test").unwrap(),
    )
    .with_tool_use_id(fixture.source.tool_use_id.clone())
    .with_svg_assistant_message_id(fixture.source.message_id.clone());
    let input = serde_json::json!({
        "action": "initial",
        "product_conversation_id": fixture.target.as_str()
    });

    let output = tokio::time::timeout(std::time::Duration::from_secs(30), tool.run(input, context))
        .await
        .expect("registered Close must finish its actor-owned lifecycle");
    assert!(
        output.is_success(),
        "registered Close failed: {}",
        output.output()
    );
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);

    let attempt_id: String = sqlx::query_scalar(
        "SELECT attempt_id FROM native_close_initial_receipts WHERE request_id=?1",
    )
    .bind(fixture.command.request_id().to_string())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let obligation = fixture.db.get_close_obligation(&attempt_id).await.unwrap();
    assert_eq!(
        obligation.phase(),
        phoenix_core::domain::close::ClosePhase::Completed,
        "registered Close must pass inventory capture and empty-scope retirement"
    );
    assert!(fixture
        .db
        .list_close_attempt_scopes(&attempt_id)
        .await
        .unwrap()
        .iter()
        .all(|scope| scope.captured_worktree.is_none()));

    event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("source actor must shut down after Close completes")
        .unwrap();
}
