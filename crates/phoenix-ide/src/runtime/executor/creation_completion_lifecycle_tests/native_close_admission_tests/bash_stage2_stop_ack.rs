#![cfg(target_os = "linux")]

use super::*;

#[tokio::test]
async fn bash_stage_two_stop_ack_reaches_real_second_writer_and_cleans_once() {
    use crate::coordinator_tools;
    use crate::send_chat_service::SendChatApplicationService;
    use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
    use phoenix_tools::bash::BashTool;
    use phoenix_tools::Tool;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use tokio_util::sync::CancellationToken;

    let Some(launch_exe) = std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE") else {
        assert!(
            std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_REQUIRED").is_none(),
            "hosted qualification must provide the real Phoenix launcher binary"
        );
        eprintln!(
            "explicitly skipping native Close Bash test: PHOENIX_BASH_LAUNCH_TEST_EXE is unset"
        );
        return;
    };
    let launch_exe = PathBuf::from(launch_exe);
    assert!(
        launch_exe.is_absolute() && launch_exe.is_file(),
        "PHOENIX_BASH_LAUNCH_TEST_EXE must name an existing absolute executable"
    );

    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let scope: WorkScopeId = fixture
        .db
        .get_conversation(TARGET_ID)
        .await
        .unwrap()
        .attached_work_scope_id
        .expect("the close target owns the disposable Work scope");
    let work_key = ResourceScopeKey::Work(scope.clone());
    let bash = BashTool;
    let bash_context = crate::tools::ToolContext::new(
        CancellationToken::new(),
        TARGET_ID.to_owned(),
        PathBuf::from("/tmp"),
        backend.browser_sessions().clone(),
        backend.bash_handles().clone(),
        Arc::new(ModelRegistry::new_empty()),
        backend.terminals.clone(),
        backend.tmux_registry().clone(),
        None,
        scope.clone(),
    );
    let spawned = bash
        .run(
            serde_json::json!({
                "op":"run", "cmd":"while :; do sleep 1; done", "wait_seconds":0,
                "label":"native-close-stage2-test"
            }),
            bash_context,
        )
        .await;
    assert!(
        spawned.is_success(),
        "real Bash fixture failed to launch: {}",
        spawned.output()
    );
    let spawned_json: serde_json::Value = serde_json::from_str(&spawned.output())
        .expect("background Bash run returns its structured handle response");
    let handle_id = spawned_json["handle"]
        .as_str()
        .expect("spawn response contains handle id")
        .to_owned();
    let original = backend
        .bash_handles()
        .get_by_id(&phoenix_tools::bash::HandleId::new(handle_id.clone()))
        .await
        .expect("the public launch registers its actual handle");
    let mut settlement = original.handle.subscribe_waiter_settlement();
    let pgids = backend.bash_handles().snapshot_live_pgids().await;
    assert_eq!(
        pgids.len(),
        1,
        "the test registry must contain only its real disposable process group"
    );
    let pgid = pgids[0];
    let registered = backend
        .bash_handles()
        .get_existing(&work_key)
        .await
        .expect("the original target Work scope has a Bash handle table");
    assert_eq!(
        registered.read().await.live_count().await,
        1,
        "the real handle must remain registered in its original Work scope"
    );

    let stats = Arc::new(
        crate::runtime::close_retirement::native_initial::bash::BashStage2TestStats::default(),
    );
    let (reached_tx, reached_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let (callback_tx, callback_rx) = oneshot::channel();
    let (completed_tx, completed_rx) = oneshot::channel();
    backend
        .set_native_bash_stage2_test_hook(
            crate::runtime::close_retirement::native_initial::bash::BashStage2TestHook {
                scope: scope.clone(),
                pgid,
                reached: reached_tx,
                resume: resume_rx,
                callback: callback_tx,
                inject_unknown_release: false,
                completed: completed_tx,
                stats: Arc::clone(&stats),
            },
        )
        .await;

    let retriever = Arc::new(fixture.db.fts_retriever());
    let read_service =
        crate::api::global_read::GlobalReadService::new(fixture.db.clone(), retriever);
    let send_chat = Arc::new(SendChatApplicationService::new(
        fixture.db.clone(),
        Arc::clone(&backend),
    ));
    let tools = coordinator_tools::tools(
        read_service,
        send_chat,
        crate::api::work_scope_load::WorkScopeLoadService::new(
            backend.bash_handles(),
            crate::api::resource_monitor::ResourceMonitor::new(),
            backend.terminals.clone(),
        ),
        &fixture.db,
    );
    let close_tool = tools
        .into_iter()
        .find(|tool| tool.name() == "close_conversation")
        .expect("coordinator registration includes close_conversation");

    let (event_tx, event_rx) = mpsc::channel(32);
    let actor = ConversationRuntime::new(
        ConvContext::coordinator(&fixture.source_transcript_id, "test-model", 200_000, 16_384),
        fixture.state.clone(),
        // Isolate the live-source fence: Stop is persisted in memory, while
        // SQLite retains the original ToolExecuting source for callback admission.
        Arc::new(InMemoryStorage::new()),
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
    let actor = actor
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver)
        .with_startup_llm_recovery_ack(startup);
    backend.runtimes.write().await.insert(
        fixture.source_transcript_id.clone(),
        crate::runtime::ConversationHandle {
            event_tx: event_tx.clone(),
            acknowledged_event_tx: sender.clone(),
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
        .unwrap()
        .unwrap()
        .unwrap();

    let close_ctx = crate::tools::ToolContext::new(
        CancellationToken::new(),
        fixture.source_transcript_id.clone(),
        PathBuf::from("/tmp"),
        backend.browser_sessions().clone(),
        backend.bash_handles().clone(),
        Arc::new(ModelRegistry::new_empty()),
        backend.terminals.clone(),
        backend.tmux_registry().clone(),
        None,
        WorkScopeId::parse("close-source-test").unwrap(),
    )
    .with_tool_use_id(fixture.source.tool_use_id.clone())
    .with_svg_assistant_message_id(fixture.source.message_id.clone());
    let input = serde_json::json!({
        "action":"initial", "product_conversation_id":fixture.target.as_str()
    });
    let close = tokio::spawn(async move { close_tool.run(input, close_ctx).await });

    let (eligible, custody) = tokio::time::timeout(std::time::Duration::from_secs(60), reached_rx)
        .await
        .expect("real Bash retirement reaches stop-observation barrier")
        .unwrap();
    assert!(eligible, "original source must be eligible before Stop ACK");
    assert!(
        stats.precleanup_handback_refused.load(Ordering::SeqCst),
        "the original owner cannot hand back membership before explicit cleanup"
    );
    stop_actor(&sender).await;
    let (revoked, deadline_unexpired, refused) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            resume_tx
                .send(())
                .expect("retained stage-two owner is waiting at the barrier");
            callback_rx.await.expect("second database callback witness")
        })
        .await
        .expect("second callback follows Stop ACK");
    assert!(revoked && deadline_unexpired && refused,
        "actual second writer callback must observe revoked source, original live deadline, and refused effect");
    tokio::time::timeout(std::time::Duration::from_secs(10), completed_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stats.stop_attempts.load(Ordering::SeqCst),
        1,
        "the original stop effect is admitted exactly once"
    );
    assert_eq!(
        stats.group_attempts.load(Ordering::SeqCst),
        0,
        "the revoked source is rejected before entering the admitted physical group-effect closure"
    );
    assert_eq!(
        stats.cleanup_attempts.load(Ordering::SeqCst),
        1,
        "the original owner compensates the observed stop exactly once after writer release"
    );
    // The completion witness establishes that the second writer has released
    // the barrier. Confirmed refusal plus successful compensation must hand
    // back the exact handle and release the native table writer without test
    // intervention. Uncertain cleanup paths must still retain their owners.
    {
        let state = custody.lock().await;
        assert!(
            state.owner.is_none(),
            "confirmed compensated refusal releases native custody after handback"
        );
    }

    let close_output = tokio::time::timeout(std::time::Duration::from_secs(30), close)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !close_output.is_success(),
        "Stop ACK revokes this Close source"
    );
    let close_error: serde_json::Value = serde_json::from_str(close_output.output())
        .expect("public Close refusal is structured JSON");
    assert_eq!(close_error["code"], "source_refused");

    let live_pgids = backend.bash_handles().snapshot_live_pgids().await;
    let original_state = original.handle.state().await;
    let original_settlement = settlement.borrow().clone();
    assert_eq!(
        live_pgids,
        vec![pgid],
        "the refused second-stage callback must preserve the test-owned group; original handle state: {original_state:?}; waiter settlement: {original_settlement:?}"
    );
    let returned = backend
        .bash_handles()
        .get_by_id(&phoenix_tools::bash::HandleId::new(handle_id.clone()))
        .await
        .expect("the same original handle is publicly addressable again");
    assert!(Arc::ptr_eq(&returned.handle, &original.handle));
    assert!(
        backend.bash_handles().reserve_spawn(&work_key).await.is_err(),
        "compensated handback must not reopen the sealed scope"
    );

    // Stop ACK may refuse the Close source, but the Bash child is test-owned.
    // Public kill exercises the normal handle API and waits for waiter settlement.
    let cleanup = bash
        .run(
            serde_json::json!({ "op":"kill", "handle":handle_id, "signal":"KILL" }),
            crate::tools::ToolContext::new(
                CancellationToken::new(),
                TARGET_ID.to_owned(),
                PathBuf::from("/tmp"),
                backend.browser_sessions().clone(),
                backend.bash_handles().clone(),
                Arc::new(ModelRegistry::new_empty()),
                backend.terminals.clone(),
                backend.tmux_registry().clone(),
                None,
                scope,
            ),
        )
        .await;
    assert!(
        cleanup.is_success(),
        "public Bash cleanup after definite release failed: {}",
        cleanup.output()
    );
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let observed = settlement.borrow_and_update().clone();
            match observed {
                phoenix_tools::bash::handle::WaiterSettlement::Settled => break,
                phoenix_tools::bash::handle::WaiterSettlement::Pending => {
                    settlement
                        .changed()
                        .await
                        .expect("the original waiter remains owned");
                }
                other => panic!("disposable Bash waiter did not settle: {other:?}"),
            }
        }
    })
    .await
    .expect("the exact original waiter reaps and releases readers and scratch");
    assert!(
        backend
            .bash_handles()
            .snapshot_live_pgids()
            .await
            .is_empty(),
        "public Bash kill must settle the original process group"
    );
    event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
}
