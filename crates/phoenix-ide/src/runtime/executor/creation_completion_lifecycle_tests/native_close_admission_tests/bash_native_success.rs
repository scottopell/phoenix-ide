#![cfg(target_os = "linux")]

use super::*;

#[tokio::test]
async fn native_close_retires_one_registered_real_bash_group() {
    run_native_bash_success(1).await;
}

#[tokio::test]
async fn native_close_retires_two_registered_real_bash_groups_under_one_scope_capture() {
    run_native_bash_success(2).await;
}

async fn run_native_bash_success(group_count: usize) {
    use crate::coordinator_tools;
    use crate::runtime::close_retirement::native_initial::bash::BashStage;
    use crate::send_chat_service::SendChatApplicationService;
    use phoenix_core::domain::close::{
        CloseAttemptId, CloseCompletionOutcome, ClosePhase, CloseRunRef, RetiredResourceKind,
        RetirementOutcome,
    };
    use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
    use phoenix_db::{CloseProcessResourceKind, CloseProcessStepOutcome};
    use phoenix_tools::bash::BashTool;
    use phoenix_tools::Tool;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use tokio_util::sync::CancellationToken;

    assert!(matches!(group_count, 1 | 2));
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
    let cwd = std::env::current_dir().expect("test process has a current directory");
    let shell_quote = |path: &std::path::Path| {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    };
    let mut settlements = Vec::with_capacity(group_count);
    let mut pgids = BTreeSet::new();
    let mut original_identities = BTreeSet::new();
    for ordinal in 0..group_count {
        let output = bash
            .run(
                serde_json::json!({
                    "op":"run",
                    "cmd":format!("cd -- {} && while :; do sleep 1; done", shell_quote(&cwd)),
                    "wait_seconds":0,
                    "label":format!("native-close-success-{ordinal}")
                }),
                crate::tools::ToolContext::new(
                    CancellationToken::new(),
                    TARGET_ID.to_owned(),
                    cwd.clone(),
                    backend.browser_sessions().clone(),
                    backend.bash_handles().clone(),
                    Arc::new(ModelRegistry::new_empty()),
                    backend.terminals.clone(),
                    backend.tmux_registry().clone(),
                    None,
                    scope.clone(),
                ),
            )
            .await;
        assert!(
            output.is_success(),
            "real Bash fixture {ordinal} failed to launch: {}",
            output.output()
        );
        let launched: serde_json::Value = serde_json::from_str(&output.output())
            .expect("background Bash run returns its structured handle response");
        let handle_id = launched["handle"]
            .as_str()
            .expect("spawn response contains handle id")
            .to_owned();
        let registered = backend
            .bash_handles()
            .get_by_id(&phoenix_tools::bash::HandleId::new(handle_id.clone()))
            .await
            .expect("the public launch registers its actual handle");
        settlements.push(registered.handle.subscribe_waiter_settlement());
        let pgid = registered
            .handle
            .live_pgid()
            .await
            .expect("the launched Bash process group has a live pgid");
        assert!(
            pgids.insert(pgid),
            "each real launch has a distinct process group"
        );
        assert_eq!(registered.owner, work_key);
        assert!(original_identities.insert(registered.handle.stable_resource_identity()));
    }
    let live_pgids = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        backend.bash_handles().snapshot_live_pgids(),
    )
    .await
    .expect("initial Bash registry snapshot completes within its deadline");
    assert_eq!(
        live_pgids.into_iter().collect::<BTreeSet<_>>(),
        pgids,
        "only the exact test-owned process groups are live before Close"
    );
    let registered = backend
        .bash_handles()
        .get_existing(&work_key)
        .await
        .expect("the original target Work scope has a Bash handle table");
    assert_eq!(
        registered.read().await.live_count().await,
        group_count,
        "all original handles are registered in the same Work scope"
    );

    let stats = Arc::new(
        crate::runtime::close_retirement::native_initial::bash::BashStage2TestStats::default(),
    );
    let mut hook_witness = None;
    if group_count == 1 {
        let pgid = *pgids.iter().next().unwrap();
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
        hook_witness = Some((reached_rx, resume_tx, callback_rx, completed_rx));
    }

    let retriever = Arc::new(fixture.db.fts_retriever());
    let read_service = crate::api::global_read::GlobalReadService::new(
        fixture.db.clone(),
        retriever,
    );
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
        Arc::new(crate::runtime::traits::DatabaseStorage::new(fixture.db.clone())),
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
        cwd,
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
        "action":"initial",
        "product_conversation_id":fixture.target.as_str()
    });
    let close = tokio::spawn(async move { close_tool.run(input, context).await });

    let custody = if let Some((reached, resume, callback, completed)) = hook_witness {
        let (eligible, custody) = tokio::time::timeout(std::time::Duration::from_secs(60), reached)
            .await
            .expect("real Bash retirement reaches the stop-observation barrier")
            .expect("stage-two hook remains owned by the retained task");
        assert!(eligible, "the original source is eligible before group signaling");
        resume.send(()).expect("the retained owner is waiting at the hook");
        let callback_result = tokio::time::timeout(std::time::Duration::from_secs(10), callback)
            .await
            .expect("the production second writer callback is reached")
            .expect("the second writer callback reports its result");
        assert_eq!(
            callback_result,
            (false, true, false),
            "the original live source admits the single group signal"
        );
        tokio::time::timeout(std::time::Duration::from_secs(60), completed)
            .await
            .expect("Bash owner completion hook fires after production retirement")
            .expect("completion witness remains available");
        assert_eq!(stats.stop_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(stats.group_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(stats.cleanup_attempts.load(Ordering::SeqCst), 1);
        {
            let state = custody.lock().await;
            assert_eq!(state.stage, BashStage::Verified);
            assert!(
                state.owner.is_none(),
                "successful Verified retirement releases the original scope registry writer"
            );
        }
        Some(custody)
    } else {
        None
    };
    let _custody_witness = custody;
    let output = tokio::time::timeout(std::time::Duration::from_secs(90), close)
        .await
        .expect("registered Close tool returns after completion")
        .expect("registered Close tool task joins");
    assert!(
        output.is_success(),
        "registered Close admission failed: {}",
        output.output()
    );
    let admission: serde_json::Value = serde_json::from_str(&output.output())
        .expect("registered Close returns a structured admission receipt");
    let attempt_id = admission["admitted"]["attempt_id"]
        .as_str()
        .expect("admission receipt identifies its admitted attempt")
        .to_owned();
    assert_eq!(admission["status"], "newly_admitted");

    let captured_scopes = fixture.db.list_close_attempt_scopes(&attempt_id).await.unwrap();
    assert_eq!(captured_scopes.len(), 1);
    assert_eq!(captured_scopes[0].scope, scope);
    assert!(
        captured_scopes[0].captured_worktree.is_none(),
        "this disposable registered Work scope must have no captured Worktree"
    );
    let obligation = fixture.db.get_close_obligation(&attempt_id).await.unwrap();
    assert_eq!(obligation.phase(), ClosePhase::Completed);
    assert_eq!(
        obligation.close_outcome(),
        Some(CloseCompletionOutcome::Archived)
    );
    let evidence = fixture.db.list_close_retirement_evidence(&attempt_id).await.unwrap();
    let expected = fixture.db.list_close_expected_retirement_resources(&attempt_id).await.unwrap();
    let original_scope_resources: Vec<_> = expected
        .iter()
        .filter(|item| item.resource.kind() == RetiredResourceKind::WorkScope)
        .collect();
    assert_eq!(original_scope_resources.len(), 1);
    let original_scope_resource = original_scope_resources[0];
    assert_eq!(original_scope_resource.scope, scope);
    assert!(
        evidence.iter().any(|item| {
            item.scope == scope
                && item.resource == original_scope_resource.resource
                && item.outcome == RetirementOutcome::Retired
        }),
        "the original WorkScope has its own durable Retired evidence"
    );
    let expected_identities: BTreeSet<String> = expected
        .iter()
        .filter(|item| {
            item.scope == scope && item.resource.kind() == RetiredResourceKind::BashProcessGroup
        })
        .map(|item| item.resource.identity().value())
        .collect();
    let successful_identities: BTreeSet<String> = evidence
        .iter()
        .filter(|item| {
            item.scope == scope
                && item.resource.kind() == RetiredResourceKind::BashProcessGroup
                && item.outcome == RetirementOutcome::Retired
        })
        .map(|item| item.resource.identity().value())
        .collect();
    assert_eq!(expected_identities, original_identities);
    assert_eq!(
        expected_identities, successful_identities,
        "captured Bash identities equal persisted successful identities"
    );
    assert_eq!(expected_identities.len(), group_count);
    let attempt = CloseAttemptId::parse(attempt_id.clone()).unwrap();
    let process_steps = fixture
        .db
        .list_close_process_step_successes(&CloseRunRef::initial(attempt))
        .await
        .unwrap();
    let bash_process_steps: Vec<_> = process_steps
        .iter()
        .filter(|step| step.resource_kind == CloseProcessResourceKind::BashProcessGroup)
        .collect();
    assert_eq!(bash_process_steps.len(), group_count);
    assert!(
        bash_process_steps.iter().all(|step| {
            step.scope == scope && step.outcome == CloseProcessStepOutcome::Retired
        }),
        "every Bash process receipt belongs to the original scope and records Retired"
    );
    let successful_process_steps: BTreeSet<String> = bash_process_steps
        .iter()
        .map(|step| step.identity.as_str().to_owned())
        .collect();
    assert_eq!(
        successful_process_steps,
        original_identities,
        "initial-run process receipts match the original scope and launch identities"
    );
    let remaining_pgids = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        backend.bash_handles().snapshot_live_pgids(),
    )
    .await
    .expect("post-completion Bash registry snapshot completes within its deadline");
    assert!(
        remaining_pgids.is_empty(),
        "public Bash registry is empty after Close completes"
    );

    for settlement in settlements {
        assert!(
            matches!(
                *settlement.borrow(),
                phoenix_tools::bash::handle::WaiterSettlement::Settled
            ),
            "the exact original Bash waiter is settled when Close completes"
        );
    }

    event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("source actor shuts down after Close completes")
        .unwrap();
}
