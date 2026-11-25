#![cfg(target_os = "linux")]

use super::*;

use crate::runtime::close_retirement::native_initial::bash::{
    BashCustodySlot, BashStage, BashStage2TestStats,
};
use std::sync::atomic::Ordering;

// Owns only this test's retained custody. A failed assertion after the real
// Confirmed writer release must not leave the stopped process behind.
struct ConfirmedBashFixtureTeardown {
    custody: BashCustodySlot,
    stats: Arc<BashStage2TestStats>,
    completion: oneshot::Receiver<()>,
    worker_done: bool,
    restored: bool,
}

impl ConfirmedBashFixtureTeardown {
    async fn wait_for_worker(&mut self) {
        let completion =
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut self.completion).await;
        self.worker_done = matches!(&completion, Ok(Ok(())));
        completion
            .expect("retained Bash task must classify the injected release")
            .unwrap();
    }

    async fn restore(&mut self) {
        assert!(self.worker_done && self.stats.actual_confirmed_release.load(Ordering::SeqCst));
        let mut state = self.custody.lock().await;
        assert_eq!(state.stage, BashStage::Quarantined);
        state
            .owner
            .as_mut()
            .expect("original owner retained")
            .cleanup_after_writer_release()
            .expect("confirmed writer allows test-only stop cleanup");
        drop(state.owner.take());
        self.restored = true;
    }
}

impl Drop for ConfirmedBashFixtureTeardown {
    fn drop(&mut self) {
        if self.restored
            || !self.worker_done
            || !self.stats.actual_confirmed_release.load(Ordering::SeqCst)
        {
            return;
        }
        let custody = Arc::clone(&self.custody);
        // Join a dedicated supervisor: tokio::test drops its runtime after a
        // panic, so a cleanup task on that runtime could be cancelled instead.
        let cleanup = std::thread::spawn(move || {
            let mut state = custody.blocking_lock();
            if state.stage != BashStage::Quarantined {
                return;
            }
            if let Some(owner) = state.owner.as_mut() {
                if let Err(error) = owner.cleanup_after_writer_release() {
                    eprintln!("confirmed-release test teardown could not resume Bash: {error}");
                    return;
                }
                drop(state.owner.take());
            }
        });
        if cleanup.join().is_err() {
            eprintln!("confirmed-release Bash test teardown supervisor panicked");
        }
    }
}

#[tokio::test]
async fn bash_second_writer_injected_unknown_release_quarantines_original_owner() {
    assert_unknown_release_custody(false).await;
}

#[tokio::test]
async fn bash_second_writer_unknown_release_retains_owner_after_public_close_waiter_drop() {
    assert_unknown_release_custody(true).await;
}

// Keep the real callback, injected unknown release, fail-stop, and custody proofs ordered.
#[allow(clippy::too_many_lines)]
async fn assert_unknown_release_custody(drop_close_before_second_writer: bool) {
    use crate::coordinator_tools;
    use crate::runtime::close_retirement::native_initial::bash::BashStage2TestHook;
    use crate::send_chat_service::SendChatApplicationService;
    use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
    use phoenix_tools::bash::BashTool;
    use phoenix_tools::Tool;
    use std::path::PathBuf;
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
        .expect("the target owns the disposable Work scope");
    let work_key = ResourceScopeKey::Work(scope.clone());
    let bash = BashTool;
    let context = crate::tools::ToolContext::new(
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
                "op":"run", "cmd":"exec sleep 120", "wait_seconds":0,
                "label":"native-close-unknown-release-test"
            }),
            context,
        )
        .await;
    assert!(
        spawned.is_success(),
        "real Bash fixture failed to launch: {}",
        spawned.output()
    );
    let spawned_json: serde_json::Value = serde_json::from_str(spawned.output()).unwrap();
    let handle_id = spawned_json["handle"].as_str().unwrap().to_owned();
    let original = backend
        .bash_handles()
        .get_by_id(&phoenix_tools::bash::HandleId::new(handle_id.clone()))
        .await
        .expect("public Bash launch registers the original handle");
    let mut settlement = original.handle.subscribe_waiter_settlement();
    let pgids = backend.bash_handles().snapshot_live_pgids().await;
    assert_eq!(
        pgids.len(),
        1,
        "fixture owns exactly one live process group"
    );
    let pgid = pgids[0];
    let registered = backend
        .bash_handles()
        .get_existing(&work_key)
        .await
        .unwrap();
    assert_eq!(registered.read().await.live_count().await, 1);

    let stats = Arc::new(BashStage2TestStats::default());
    let (reached_tx, reached_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let (callback_tx, callback_rx) = oneshot::channel();
    let (completed_tx, completed_rx) = oneshot::channel();
    backend
        .set_native_bash_stage2_test_hook(BashStage2TestHook {
            scope: scope.clone(),
            pgid,
            reached: reached_tx,
            resume: resume_rx,
            callback: callback_tx,
            inject_unknown_release: true,
            completed: completed_tx,
            stats: Arc::clone(&stats),
        })
        .await;

    let read_service = crate::api::global_read::GlobalReadService::new(
        fixture.db.clone(),
        Arc::new(fixture.db.fts_retriever()),
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
    .with_actor_database(fixture.db.clone());
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
            event_tx: crate::runtime::ConversationEventSender::for_test_receiver(event_tx.clone()),
            acknowledged_event_tx:
                crate::runtime::cancellation_inbox::AcknowledgedEventSender::for_test_receiver(
                    sender,
                ),
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
    let mut close = Some(tokio::spawn(async move {
        close_tool.run(input, close_ctx).await
    }));

    let (eligible, custody) = tokio::time::timeout(std::time::Duration::from_secs(60), reached_rx)
        .await
        .expect("real Bash retirement reaches the stop-observed barrier")
        .unwrap();
    let mut teardown = ConfirmedBashFixtureTeardown {
        custody: Arc::clone(&custody),
        stats: Arc::clone(&stats),
        completion: completed_rx,
        worker_done: false,
        restored: false,
    };
    let observer_drop_observations = if drop_close_before_second_writer {
        // Drop only the public response observer while the retained worker is
        // paused before the second writer. Keep the actor and independent
        // callback/completion/custody probes alive through classification.
        // Record now, but assert only after resuming and joining retained work:
        // assertion unwinding must not abandon the physically stopped fixture.
        let observer_cancelled = if let Some(observer) = close.take() {
            observer.abort();
            matches!(observer.await, Err(error) if error.is_cancelled())
        } else {
            false
        };
        Some((
            observer_cancelled,
            stats.actual_confirmed_release.load(Ordering::SeqCst),
            stats.group_attempts.load(Ordering::SeqCst),
            running.is_finished(),
        ))
    } else {
        None
    };
    let resumed = resume_tx.send(());
    let callback = tokio::time::timeout(std::time::Duration::from_secs(10), callback_rx).await;
    teardown.wait_for_worker().await;
    if let Some((cancelled, released, attempts, actor_finished)) = observer_drop_observations {
        assert!(
            cancelled,
            "public observer was cancelled before resuming stage two"
        );
        assert!(!released, "second writer had not returned at observer drop");
        assert_eq!(attempts, 0, "second callback had not run at observer drop");
        assert!(
            !actor_finished,
            "source actor remained alive at the stage-two barrier"
        );
    }
    assert!(
        stats.actual_confirmed_release.load(Ordering::SeqCst),
        "only an actual Confirmed second writer return permits injected uncertainty"
    );
    assert!(
        eligible,
        "the original source is live before the second writer"
    );
    resumed.expect("retained owner waits at the barrier");
    let (revoked, deadline_unexpired, refused) = callback
        .expect("real second physical callback must run")
        .unwrap();
    assert!(
        !revoked && deadline_unexpired && !refused,
        "second writer uses the unrevoked original source and unexpired original deadline"
    );
    assert_eq!(stats.stop_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(stats.group_attempts.load(Ordering::SeqCst), 1);
    assert!(
        stats.group_signal_succeeded.load(Ordering::SeqCst),
        "the actual second callback must successfully signal the original group"
    );
    assert_eq!(stats.cleanup_attempts.load(Ordering::SeqCst), 0);
    assert!(
        backend.local_authority_is_closed(),
        "unknown release closes process-wide admission"
    );
    assert!(
        backend.acquire_local_authority_pass().is_err(),
        "a fresh scope cannot mutate after fatal close"
    );
    assert_eq!(
        backend.observed_local_authority_failure_boundary(),
        Some("native_close_initial_bash_group_entry")
    );

    // The hook replaces a Confirmed return AFTER the real callback, not an actual
    // unknown SQLite result. This owner and its original waiter must stay pinned
    // until the assertions above have proved the production fail-stop path.
    {
        let custody_guard = custody.lock().await;
        assert_eq!(custody_guard.stage, BashStage::Quarantined);
        assert!(
            custody_guard.owner.is_some(),
            "entire original owner remains in custody"
        );
    }
    assert!(
        backend
            .bash_handles()
            .get_by_id(&phoenix_tools::bash::HandleId::new(handle_id.clone()))
            .await
            .is_none(),
        "unknown release must not hand back ordinary handle membership"
    );
    assert!(
        matches!(
            *settlement.borrow_and_update(),
            phoenix_tools::bash::handle::WaiterSettlement::Pending
        ),
        "the exact original waiter has not falsely completed cleanup"
    );
    // The retained owner's scope writer is still held: a registry snapshot
    // would block here, and is not a valid witness for this fatal boundary.
    let receipts = count(&fixture.db, "native_close_initial_receipts").await;
    let runs = count(&fixture.db, "close_runs").await;
    assert_eq!(receipts, 1);
    assert_eq!(runs, 1, "no retry or successor run is admitted");

    // The actor joins retained native work before fatal exit. Do not send Shutdown:
    // fatal loss already ends it, and a Close observer need not receive a result.
    let disposition = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("fatal source actor must exit after the retained task settles")
        .unwrap();
    assert_eq!(disposition, RuntimeExitDisposition::FatalLocalAuthorityLoss);
    assert_eq!(
        count(&fixture.db, "native_close_initial_receipts").await,
        receipts
    );
    assert_eq!(count(&fixture.db, "close_runs").await, runs);
    if let Some(observer) = close {
        observer.abort();
        let _ = observer.await;
    }

    // INJECTION-ONLY TEST FIXTURE RESTORE: the real writer returned Confirmed;
    // only this test's hook substituted Unconfirmed after it returned. Never
    // perform this cleanup for a production unknown release. The retained task
    // and actor are finished before touching this original Bash owner.
    teardown.restore().await;
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
                other @ phoenix_tools::bash::handle::WaiterSettlement::Unverified(_) => {
                    panic!("original waiter did not settle: {other:?}");
                }
            }
        }
    })
    .await
    .expect("test-only restore must reap the exact original waiter");
    assert!(backend
        .bash_handles()
        .snapshot_live_pgids()
        .await
        .is_empty());
    assert_eq!(
        stats.cleanup_attempts.load(Ordering::SeqCst),
        0,
        "only explicit test teardown, never production unknown cleanup, resumed the owner"
    );
}
