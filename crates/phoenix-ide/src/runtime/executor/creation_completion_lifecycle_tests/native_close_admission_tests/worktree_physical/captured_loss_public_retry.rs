use super::*;
use crate::coordinator_tools;
use crate::runtime::executor::native_close_retry::test_hooks::{Hooks, Pause, Point};
use crate::send_chat_service::SendChatApplicationService;
use tokio_util::sync::CancellationToken;

pub(super) async fn acknowledge_public_initial_result(
    fixture: &CloseFixture,
    run: &CloseRunRef,
    output: &str,
) {
    use phoenix_core::domain::db_schema::{
        FinalizedToolResult, InputOrigin, Message, MessageContent, ToolResult,
    };

    let source = fixture
        .db
        .get_conversation(&fixture.source_transcript_id)
        .await
        .unwrap();
    assert_eq!(
        source.state, fixture.state,
        "initial result must precede source shutdown"
    );
    let ConvState::ToolExecuting {
        assistant_message,
        current_tool,
        ..
    } = &source.state
    else {
        panic!("initial Close source must still be executing");
    };
    assert_eq!(assistant_message.message_id, fixture.source.message_id);
    assert_eq!(current_tool.id, fixture.source.tool_use_id);
    let sequence_id = fixture
        .db
        .get_messages(&fixture.source_transcript_id)
        .await
        .unwrap()
        .iter()
        .map(|message| message.sequence_id)
        .max()
        .unwrap_or(0)
        + 1;
    let content = MessageContent::agent(assistant_message.content.clone());
    let assistant = Message {
        origin: InputOrigin::SystemGenerated,
        message_id: assistant_message.message_id.clone(),
        conversation_id: fixture.source_transcript_id.clone(),
        sequence_id,
        message_type: content.message_type(),
        content,
        display_data: assistant_message.display_data.clone(),
        usage_data: assistant_message.usage.clone(),
        created_at: assistant_message.created_at,
    };
    let now = chrono::Utc::now();
    let result = FinalizedToolResult::new(
        &assistant.message_id,
        &assistant.conversation_id,
        sequence_id + 1,
        now,
        &ToolResult::success(fixture.source.tool_use_id.clone(), output.to_owned()),
    );
    fixture
        .db
        .persist_tool_round_and_state(
            &fixture.source_transcript_id,
            &assistant,
            std::slice::from_ref(&result),
            &source.state,
            now,
        )
        .await
        .unwrap();
    let acknowledgement: (String, String, i64) = sqlx::query_as(
        "SELECT ack.assistant_message_id, ack.result_message_id, allowance.suppressed
         FROM native_close_initial_result_acknowledgements ack
         JOIN close_completion_retry_allowances allowance ON allowance.attempt_id = ack.attempt_id
         WHERE ack.request_id = ?1 AND ack.attempt_id = ?2 AND ack.run_ordinal = 1",
    )
    .bind(fixture.command.request_id().to_string())
    .bind(run.attempt_id.as_str())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(
        acknowledgement,
        (assistant.message_id, result.row().message_id.clone(), 0)
    );
    fixture
        .db
        .read_native_close_captured_loss_retry_context(&fixture.target, run)
        .await
        .expect("canonical initial result must retain captured-loss retry eligibility");
}

// Preserve the ordered public capture/retry journey: Stop cuts, physical effects,
// durable classification, and canonical result provenance must remain causally linked.
#[allow(clippy::too_many_lines)]
pub(super) async fn retry_after_public_capture(
    initial_fixture: &CloseFixture,
    git: &DisposableGit,
    backend: Arc<crate::runtime::RuntimeManager>,
    scope: &WorkScopeId,
    initial_run: &CloseRunRef,
    stop_at: Option<Point>,
) {
    install_physical_close_diagnostics();
    let source = SourceToolCall {
        message_id: "captured-loss-retry-message".into(),
        tool_use_id: "captured-loss-retry-call".into(),
    };
    let intent = NativeCloseIntent::ExplicitRetry {
        target: initial_fixture.target.clone(),
        attempt_id: initial_run.attempt_id.clone(),
        failed_run_ordinal: initial_run.ordinal,
    };
    let fixture = CloseFixture {
        db: initial_fixture.db.clone(),
        source_transcript_id: initial_fixture.source_transcript_id.clone(),
        target: initial_fixture.target.clone(),
        command: NativeCloseCommand::new(
            initial_fixture.source_transcript_id.clone(),
            source.clone(),
            intent.clone(),
        )
        .unwrap(),
        state: executing_state(&source, &intent),
        source,
    };
    assert_ne!(
        fixture.command.request_id(),
        initial_fixture.command.request_id()
    );
    fixture
        .db
        .update_conversation_state(&fixture.source_transcript_id, &fixture.state)
        .await
        .unwrap();
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
    .with_actor_database(fixture.db.clone());
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
            acknowledged_event_tx: sender.clone(),
            handoff_review_tx: None,
            turn_trigger: crate::runtime::TurnTriggerSlot::default(),
            broadcast_tx: crate::runtime::SseBroadcaster::new(128, 0),
            identity: Arc::new(()),
            state_rx,
            startup_llm_recovery_ack: Arc::new(Mutex::new(None)),
        },
    );
    let hooks = Hooks::default();
    let actor_hooks = hooks.clone();
    let running = tokio::spawn(async move { actor_hooks.scope(actor.run()).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .expect("source actor must finish startup")
        .unwrap()
        .unwrap();

    let input = intent.source_input();
    let context = crate::tools::ToolContext::new(
        CancellationToken::new(),
        fixture.source_transcript_id.clone(),
        git.root.clone(),
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
    let captured = fixture
        .db
        .read_native_close_captured_loss_retry_context(&fixture.target, initial_run)
        .await
        .unwrap();
    let historical = captured_history(&fixture, initial_run).await;
    let before = public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await;
    let stale_context = context
        .clone()
        .with_tool_use_id(initial_fixture.source.tool_use_id.clone());
    let stale = tool.run(input.clone(), stale_context).await;
    assert!(!stale.is_success());
    let stale: Value = serde_json::from_str(stale.output()).unwrap();
    assert_eq!(stale["code"], "source_refused");
    assert_eq!(
        public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
        before
    );

    let still_dirty = tool.run(input.clone(), context.clone()).await;
    assert!(!still_dirty.is_success());
    let still_dirty: Value = serde_json::from_str(still_dirty.output()).unwrap();
    assert_eq!(still_dirty["code"], "storage_rejected", "{still_dirty}");
    assert_eq!(
        public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
        before
    );
    assert_eq!(captured_history(&fixture, initial_run).await, historical);
    assert!(git.worktree.exists());
    assert!(git.admin.join("gitdir").is_file());
    assert_eq!(
        std::fs::read(git.worktree.join("untracked.txt")).unwrap(),
        b"unpreserved new file\n"
    );
    assert_eq!(
        std::fs::read(git.worktree.join("build/a.bin")).unwrap(),
        b"retained build a"
    );
    assert!(fixture
        .db
        .read_native_close_explicit_retry_receipt(&fixture.command)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .db
        .read_stopped_close_original_retirement_proof(initial_run)
        .await
        .is_err());

    // These filesystem changes are independent of Close and grant it no authority.
    git.run(&git.worktree, &["restore", "--", "tracked.txt"]);
    std::fs::remove_file(git.worktree.join("untracked.txt")).unwrap();
    std::fs::remove_dir_all(git.worktree.join("build")).unwrap();
    assert_eq!(
        public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
        before
    );
    assert_eq!(captured_history(&fixture, initial_run).await, historical);
    assert_eq!(
        fixture
            .db
            .read_native_close_captured_loss_retry_context(&fixture.target, initial_run)
            .await
            .unwrap(),
        captured
    );
    assert!(git.worktree.exists());
    assert!(git.admin.join("gitdir").is_file());
    assert!(
        !fixture
            .db
            .get_conversation(TARGET_ID)
            .await
            .unwrap()
            .archived
    );

    if stop_at == Some(Point::CapturedLossAssessmentReady) {
        let counts_before = close_row_counts(&fixture).await;
        let target_before = durable_target_rows(&fixture).await;
        let worktree_before = physical_snapshot(&git.worktree);
        let admin_before = physical_snapshot(&git.admin);
        let sibling_before = physical_snapshot(&git.sibling);
        let sibling_admin_before = physical_snapshot(&git.sibling_admin);
        let registrations_before = git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]);
        let scope_before = fixture
            .db
            .get_conversation_work_scope_context(TARGET_ID)
            .await
            .unwrap();
        let messages_before =
            serde_json::to_value(fixture.db.get_messages(TARGET_ID).await.unwrap()).unwrap();

        let mut pause = hooks.arm(Point::CapturedLossAssessmentReady);
        let retry_tool = Arc::clone(&tool);
        let retry_input = input.clone();
        let retry_context = context.clone();
        let diagnostics_before_retry = physical_close_diagnostics();
        let mut retry =
            tokio::spawn(async move { retry_tool.run(retry_input, retry_context).await });
        require_retry_barrier(
            &fixture,
            initial_run,
            &mut pause,
            &mut retry,
            &diagnostics_before_retry,
        )
        .await;
        assert!(
            !retry.is_finished(),
            "assessment remains owned by the held worker"
        );
        assert_eq!(
            public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
            before
        );

        // ACK must arrive while assessment cannot finish, not after releasing it.
        stop_actor(&sender).await;
        assert!(
            !retry.is_finished(),
            "Stop ACK must not depend on joining assessment"
        );
        assert_ne!(
            fixture
                .db
                .get_conversation(&fixture.source_transcript_id)
                .await
                .unwrap()
                .state,
            fixture.state,
            "Stop ACK must have durably invalidated the exact source"
        );
        assert_eq!(
            public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
            before
        );
        assert_eq!(physical_snapshot(&git.worktree), worktree_before);
        assert_eq!(physical_snapshot(&git.admin), admin_before);
        pause.release();
        let refused = tokio::time::timeout(Duration::from_secs(10), retry)
            .await
            .expect("held assessment must join after Stop")
            .unwrap();
        assert!(!refused.is_success(), "{}", refused.output());
        let refused: Value = serde_json::from_str(refused.output()).unwrap();
        assert_eq!(refused["code"], "source_refused");
        for _ in 0..2 {
            let refused = tokio::time::timeout(
                Duration::from_secs(10),
                tool.run(input.clone(), context.clone()),
            )
            .await
            .expect("stopped public retry must return without admission");
            assert!(!refused.is_success(), "{}", refused.output());
            let refused: Value = serde_json::from_str(refused.output()).unwrap();
            assert_eq!(refused["code"], "source_refused");
            assert_eq!(
                public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
                before
            );
            // Stop revoked the source, so the source-gated replay reader must refuse.
            assert!(fixture
                .db
                .read_native_close_explicit_retry_receipt(&fixture.command)
                .await
                .is_err());
            assert!(retained_public_retry_receipt(&fixture).await.is_none());
        }
        assert_eq!(close_row_counts(&fixture).await, counts_before);
        assert_eq!(durable_target_rows(&fixture).await, target_before);
        assert_eq!(captured_history(&fixture, initial_run).await, historical);
        assert_eq!(
            serde_json::to_value(fixture.db.get_messages(TARGET_ID).await.unwrap()).unwrap(),
            messages_before
        );
        assert_eq!(
            fixture
                .db
                .get_conversation_work_scope_context(TARGET_ID)
                .await
                .unwrap(),
            scope_before
        );
        assert!(
            !fixture
                .db
                .get_conversation(TARGET_ID)
                .await
                .unwrap()
                .archived
        );
        assert_eq!(physical_snapshot(&git.worktree), worktree_before);
        assert_eq!(physical_snapshot(&git.admin), admin_before);
        assert_eq!(physical_snapshot(&git.sibling), sibling_before);
        assert_eq!(physical_snapshot(&git.sibling_admin), sibling_admin_before);
        assert_eq!(
            git.run(&git.repo, &["worktree", "list", "--porcelain", "-z"]),
            registrations_before
        );
        git.assert_untargeted_state_preserved();
        event_tx.send(Event::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .expect("stopped retry source actor must shut down")
            .unwrap();
        return;
    }

    if let Some(point) = stop_at {
        let worktree_before = physical_snapshot(&git.worktree);
        let admin_before = physical_snapshot(&git.admin);
        let sibling_before = physical_snapshot(&git.sibling);
        let sibling_admin_before = physical_snapshot(&git.sibling_admin);
        let mut pause = hooks.arm(point);
        let retry_tool = Arc::clone(&tool);
        let retry_input = input.clone();
        let retry_context = context.clone();
        let diagnostics_before_retry = physical_close_diagnostics();
        let mut retry =
            tokio::spawn(async move { retry_tool.run(retry_input, retry_context).await });
        require_retry_barrier(
            &fixture,
            initial_run,
            &mut pause,
            &mut retry,
            &diagnostics_before_retry,
        )
        .await;
        let receipt = fixture
            .db
            .read_native_close_explicit_retry_receipt(&fixture.command)
            .await
            .unwrap()
            .expect("barrier must belong to a durably admitted successor");
        assert_eq!(receipt.attempt_id, initial_run.attempt_id);
        assert_eq!(receipt.run_ordinal.get(), initial_run.ordinal.get() + 1);
        let successor = CloseRunRef {
            attempt_id: receipt.attempt_id.clone(),
            ordinal: receipt.run_ordinal,
        };
        assert_eq!(
            fixture.db.get_close_run(&successor).await.unwrap().status,
            CloseRunStatus::Running
        );
        assert!(fixture
            .db
            .close_safe_retry_progress(&successor)
            .await
            .unwrap()
            .successful
            .is_empty());
        assert!(!retry.is_finished());
        if matches!(
            point,
            Point::WorktreeDispatchBeforeWriter | Point::WorktreePreparationReady
        ) {
            assert_eq!(physical_snapshot(&git.worktree), worktree_before);
            assert_eq!(physical_snapshot(&git.admin), admin_before);
        } else {
            assert_eq!(point, Point::PhysicalBeforeObservation);
            assert_eq!(
                std::fs::symlink_metadata(&git.worktree).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
            assert_eq!(
                std::fs::symlink_metadata(&git.admin).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
        }

        if point == Point::WorktreeDispatchBeforeWriter {
            assert_no_retry_dispatch_or_cleanup_plan(&fixture, initial_run.attempt_id.as_str())
                .await;
        }

        // The worker cannot publish its result until after this ACK is observed.
        stop_actor(&sender).await;
        assert!(
            !retry.is_finished(),
            "Stop ACK must not wait for the held worker"
        );
        assert_eq!(
            fixture.db.get_close_run(&successor).await.unwrap().status,
            CloseRunStatus::Stopped
        );
        assert_eq!(captured_history(&fixture, initial_run).await, historical);
        assert!(
            !fixture
                .db
                .get_conversation(TARGET_ID)
                .await
                .unwrap()
                .archived
        );
        assert_eq!(
            fixture
                .db
                .get_conversation_work_scope_context(TARGET_ID)
                .await
                .unwrap()
                .1,
            WorkScopeLifecycle::Active
        );
        let stopped_rows = public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await;
        let stopped: Value = serde_json::from_str(&stopped_rows).unwrap();
        assert_eq!(
            stopped[4],
            serde_json::json!([]),
            "no retry effect was acknowledged as successful"
        );
        if point == Point::WorktreeDispatchBeforeWriter {
            assert_no_retry_dispatch_or_cleanup_plan(&fixture, initial_run.attempt_id.as_str())
                .await;
        }
        let stopped_target = durable_target_rows(&fixture).await;
        let stopped_physical = physical_snapshot(&git.root);
        let failures = fixture
            .db
            .list_close_cleanup_failures(successor.attempt_id.as_str())
            .await
            .unwrap();
        assert!(
            failures.iter().any(|failure| {
                failure.run_ordinal == successor.ordinal
                    && failure.occurrence.failure_occurrence_id == successor.failure_occurrence_id()
            }),
            "Stop ACK must establish the exact successor failure"
        );

        pause.release();
        let refused = tokio::time::timeout(Duration::from_secs(10), retry)
            .await
            .expect("stopped successor must join its original worker")
            .unwrap();
        assert!(!refused.is_success(), "{}", refused.output());
        let refused: Value = serde_json::from_str(refused.output()).unwrap();
        if point == Point::WorktreeDispatchBeforeWriter {
            assert_no_retry_dispatch_or_cleanup_plan(&fixture, initial_run.attempt_id.as_str())
                .await;
        }
        assert_eq!(refused["code"], "storage_rejected");
        assert_eq!(
            public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
            stopped_rows
        );
        assert_eq!(durable_target_rows(&fixture).await, stopped_target);
        assert_eq!(captured_history(&fixture, initial_run).await, historical);
        assert_eq!(physical_snapshot(&git.root), stopped_physical);
        assert_eq!(physical_snapshot(&git.sibling), sibling_before);
        assert_eq!(physical_snapshot(&git.sibling_admin), sibling_admin_before);
        git.assert_untargeted_state_preserved();

        let replay = tokio::time::timeout(Duration::from_secs(10), tool.run(input, context))
            .await
            .expect("stopped successor exact-source receipt replay must be bounded");
        assert!(!replay.is_success(), "{}", replay.output());
        let replay: Value = serde_json::from_str(replay.output()).unwrap();
        if point == Point::WorktreeDispatchBeforeWriter {
            assert_no_retry_dispatch_or_cleanup_plan(&fixture, initial_run.attempt_id.as_str())
                .await;
        }
        assert_eq!(replay["code"], "source_refused");
        // Historical storage survives Stop, but the stopped source cannot replay it.
        assert!(fixture
            .db
            .read_native_close_explicit_retry_receipt(&fixture.command)
            .await
            .is_err());
        assert_eq!(
            retained_public_retry_receipt(&fixture).await,
            Some((
                receipt.attempt_id.as_str().to_owned(),
                receipt.run_ordinal.get(),
            )),
            "source refusal must retain the admitted successor receipt"
        );
        assert_eq!(
            public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
            stopped_rows
        );
        assert_eq!(durable_target_rows(&fixture).await, stopped_target);
        assert_eq!(physical_snapshot(&git.root), stopped_physical);
        event_tx.send(Event::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .expect("stopped successor actor must shut down")
            .unwrap();
        return;
    }

    let diagnostics_before_clean_retry = physical_close_diagnostics();
    let completed = tokio::time::timeout(
        Duration::from_secs(10),
        tool.run(input.clone(), context.clone()),
    )
    .await
    .expect("public captured-loss retry must finish within its terminal fence");
    if !completed.is_success() {
        let diagnostics = physical_close_diagnostics();
        eprintln!(
            "clean captured-loss retry was refused: {}\ndiagnostics before clean retry:\n{diagnostics_before_clean_retry}\ndiagnostics after clean retry (bounded tail):\n{diagnostics}",
            completed.output()
        );
        let evidence = tokio::time::timeout(
            Duration::from_secs(2),
            clean_retry_failure_evidence(&fixture, initial_run),
        )
        .await;
        panic!("clean captured-loss retry failure; durable evidence: {evidence:?}");
    }
    let admitted: Value = serde_json::from_str(completed.output()).unwrap();
    assert_eq!(admitted["action"], "explicit_retry");
    assert_eq!(admitted["status"], "newly_admitted");
    assert_eq!(
        admitted["admitted"]["attempt_id"],
        initial_run.attempt_id.as_str()
    );
    assert_eq!(
        admitted["admitted"]["run_ordinal"],
        initial_run.ordinal.get() + 1
    );
    assert_eq!(
        admitted["request_id"],
        fixture.command.request_id().to_string()
    );
    let receipt = fixture
        .db
        .read_native_close_explicit_retry_receipt(&fixture.command)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.attempt_id, initial_run.attempt_id);
    assert_eq!(receipt.run_ordinal.get(), initial_run.ordinal.get() + 1);
    let successor = CloseRunRef {
        attempt_id: receipt.attempt_id.clone(),
        ordinal: receipt.run_ordinal,
    };
    assert_eq!(
        fixture.db.get_close_run(&successor).await.unwrap().status,
        CloseRunStatus::Completed
    );
    let effects = fixture
        .db
        .list_close_safe_retry_effects(&successor)
        .await
        .unwrap();
    assert_eq!(effects.len(), 2);
    for kind in [
        RetiredResourceKind::Worktree,
        RetiredResourceKind::WorkScope,
    ] {
        assert!(effects
            .iter()
            .any(|effect| { &effect.scope == scope && effect.resource.kind() == kind }));
    }
    let success_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_run_retry_successes WHERE attempt_id = ?1 AND run_ordinal = ?2",
    )
    .bind(successor.attempt_id.as_str())
    .bind(successor.ordinal.get())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(success_count, 2);
    let (generation, fingerprint): (String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint FROM native_close_retry_plans
         WHERE attempt_id = ?1 AND run_ordinal = ?2",
    )
    .bind(successor.attempt_id.as_str())
    .bind(successor.ordinal.get())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let successor_snapshot = CloseRetirementSnapshot::parse(generation, fingerprint).unwrap();
    assert_ne!(
        successor_snapshot.generation(),
        captured.snapshot().generation()
    );
    assert_ne!(
        successor_snapshot.fingerprint(),
        captured.snapshot().fingerprint()
    );
    assert!(!git.worktree.exists());
    assert!(!git.admin.join("gitdir").is_file());
    assert_eq!(
        fixture
            .db
            .get_conversation_work_scope_context(TARGET_ID)
            .await
            .unwrap()
            .1,
        WorkScopeLifecycle::Retired
    );
    assert!(
        fixture
            .db
            .get_conversation(TARGET_ID)
            .await
            .unwrap()
            .archived
    );
    assert_eq!(
        captured_history(&fixture, initial_run).await,
        historical,
        "admission must not rewrite the failed run, captured losses or failure message"
    );
    assert!(fixture
        .db
        .list_close_retirement_evidence(initial_run.attempt_id.as_str())
        .await
        .unwrap()
        .iter()
        .all(|evidence| evidence.snapshot != *captured.snapshot()));
    assert_eq!(
        fixture
            .db
            .read_ordinary_product_conversation_metadata(TARGET_ID)
            .await
            .unwrap()
            .lifecycle,
        OrdinaryProductConversationLifecycle::History
    );
    let messages = fixture.db.get_messages(TARGET_ID).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].message_id,
        format!("close-outcome:{}", initial_run.attempt_id)
    );
    assert_eq!(
        messages[1].message_id,
        format!(
            "close-run-outcome:{}:{}",
            successor.ordinal.get(),
            successor.attempt_id
        )
    );
    assert_eq!(
        messages[1].content.to_json(),
        phoenix_core::domain::db_schema::MessageContent::system(format!(
            "Close retry run {} for attempt {} completed; cleanup retired and conversation archived in History.",
            successor.ordinal.get(), successor.attempt_id
        )).to_json()
    );
    let messages = serde_json::to_value(messages).unwrap();

    let terminal = public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await;
    let replay = tokio::time::timeout(Duration::from_secs(10), tool.run(input, context))
        .await
        .expect("exact-source captured-loss retry replay must be bounded");
    assert!(replay.is_success(), "{}", replay.output());
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    let mut expected = admitted;
    expected["status"] = serde_json::json!("replay");
    assert_eq!(replay, expected);
    assert_eq!(
        public_retry_rows(&fixture, initial_run.attempt_id.as_str()).await,
        terminal
    );
    assert_eq!(
        serde_json::to_value(fixture.db.get_messages(TARGET_ID).await.unwrap()).unwrap(),
        messages
    );
    assert!(!git.worktree.exists());
    assert!(!git.admin.join("gitdir").is_file());
    assert!(git.sibling.exists());
    git.assert_untargeted_state_preserved();
    event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("retry source actor must shut down")
        .unwrap();
}

async fn require_retry_barrier(
    fixture: &CloseFixture,
    run: &CloseRunRef,
    pause: &mut Pause,
    retry: &mut tokio::task::JoinHandle<phoenix_tools::ToolOutput>,
    diagnostics_before_retry: &str,
) {
    let (failure, joined) = tokio::select! {
        arrival = pause.reached() => match arrival {
            Ok(()) => return,
            Err(error) => (error.to_owned(), false),
        },
        result = &mut *retry => (
            format!("public retry completed before its required barrier: {result:?}"),
            true,
        ),
    };
    let diagnostics = physical_close_diagnostics();
    eprintln!(
        "{failure}; public task finished: {}\ndiagnostics before held retry:\n{diagnostics_before_retry}\ndiagnostics after held retry (bounded tail):\n{diagnostics}",
        retry.is_finished()
    );
    if !joined {
        // Failure-only custody: do not detach the public waiter. Dropping the
        // caller's Pause during unwind releases any held actor-owned worker.
        retry.abort();
        let result = tokio::time::timeout(Duration::from_secs(2), &mut *retry).await;
        eprintln!("public retry join after barrier failure: {result:?}");
    }
    let evidence = tokio::time::timeout(
        Duration::from_secs(2),
        clean_retry_failure_evidence(fixture, run),
    )
    .await;
    panic!("{failure}; durable evidence: {evidence:?}");
}

async fn clean_retry_failure_evidence(fixture: &CloseFixture, run: &CloseRunRef) -> String {
    let source = fixture
        .db
        .get_conversation(&fixture.source_transcript_id)
        .await;
    let target = fixture.db.get_conversation(TARGET_ID).await;
    let retry_rows = sqlx::query_scalar::<_, String>(
        "SELECT json_object('run_ordinal', run_ordinal, 'status', status,
                            'ended_at_us', ended_at_us)
         FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal",
    )
    .bind(run.attempt_id.as_str())
    .fetch_all(fixture.db.pool())
    .await;
    let receipt = fixture
        .db
        .read_native_close_explicit_retry_receipt(&fixture.command)
        .await;
    let cleanup_failures = fixture
        .db
        .list_close_cleanup_failures(run.attempt_id.as_str())
        .await;
    let retirement = worktree_close_failure_evidence(fixture, run).await;
    format!(
        "source: {source:?}; target: {target:?}; retry rows: {retry_rows:?}; retry receipt: {receipt:?}; cleanup failures: {cleanup_failures:?}; {retirement}"
    )
}

#[derive(Debug, PartialEq)]
struct CapturedHistory {
    run: phoenix_core::domain::close::CloseRun,
    inspections: Vec<phoenix_core::domain::close::CloseInspection>,
    losses: Vec<phoenix_core::domain::close::CloseInspectionLoss>,
    failures: Vec<phoenix_db::CloseCleanupFailure>,
    outcome: Value,
}

async fn captured_history(fixture: &CloseFixture, run: &CloseRunRef) -> CapturedHistory {
    let outcome_id = format!("close-outcome:{}", run.attempt_id);
    let outcome = fixture
        .db
        .get_messages(TARGET_ID)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.message_id == outcome_id)
        .expect("initial failed Close must retain its outcome");
    CapturedHistory {
        run: fixture.db.get_close_run(run).await.unwrap(),
        inspections: fixture
            .db
            .list_close_retirement_inspections(run.attempt_id.as_str())
            .await
            .unwrap(),
        losses: fixture
            .db
            .list_close_retirement_losses(run.attempt_id.as_str())
            .await
            .unwrap(),
        failures: fixture
            .db
            .list_close_cleanup_failures(run.attempt_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .filter(|failure| failure.run_ordinal == run.ordinal)
            .collect(),
        outcome: serde_json::to_value(outcome).unwrap(),
    }
}

async fn assert_no_retry_dispatch_or_cleanup_plan(fixture: &CloseFixture, attempt: &str) {
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1),
                (SELECT COUNT(*) FROM close_worktree_cleanup_plans WHERE attempt_id = ?1)",
    )
    .bind(attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(
        counts,
        (0, 0),
        "pre-writer Stop must not authorize dispatch or cleanup metadata"
    );
}

// Test-only durable observation; unlike replay admission, it needs no live source.
async fn retained_public_retry_receipt(fixture: &CloseFixture) -> Option<(String, i64)> {
    sqlx::query_as(
        "SELECT attempt_id, run_ordinal FROM native_close_initial_receipts
         WHERE request_id = ?1 AND source_transcript_id = ?2
           AND source_message_id = ?3 AND source_tool_use_id = ?4
           AND request_fingerprint = ?5 AND product_conversation_id = ?6
           AND intent = 'explicit_retry'",
    )
    .bind(fixture.command.request_id().to_string())
    .bind(fixture.command.source_transcript_id())
    .bind(&fixture.command.source().message_id)
    .bind(&fixture.command.source().tool_use_id)
    .bind(fixture.command.fingerprint())
    .bind(fixture.command.intent().target().as_str())
    .fetch_optional(fixture.db.pool())
    .await
    .unwrap()
}

async fn public_retry_rows(fixture: &CloseFixture, attempt: &str) -> String {
    sqlx::query_scalar(
        "SELECT json_array(
           (SELECT json_group_array(json_array(run_ordinal, status, retry_evidence_kind,
                    created_at_us, ended_at_us, retry_requested_by, retry_observed_at_us,
                    precondition_resolution, safety_evidence)) FROM
             (SELECT * FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal)),
           (SELECT json_group_array(json_array(request_id, source_message_id, source_tool_use_id,
                    request_fingerprint, run_ordinal, intent)) FROM
             (SELECT * FROM native_close_initial_receipts WHERE attempt_id = ?1 ORDER BY run_ordinal)),
           (SELECT json_group_array(json_array(run_ordinal, failed_run_ordinal,
                    inspection_generation, inspection_fingerprint, effect_count)) FROM
             (SELECT * FROM native_close_retry_plans WHERE attempt_id = ?1 ORDER BY run_ordinal)),
           (SELECT json_group_array(json_array(run_ordinal, ordinal, scope, resource_kind,
                    identity_kind, identity_codec, identity_value)) FROM
             (SELECT * FROM close_run_retry_effects WHERE attempt_id = ?1 ORDER BY run_ordinal, ordinal)),
           (SELECT json_group_array(json_array(run_ordinal, ordinal, detail, observed_at_us)) FROM
             (SELECT * FROM close_run_retry_successes WHERE attempt_id = ?1 ORDER BY run_ordinal, ordinal)))",
    )
    .bind(attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap()
}
