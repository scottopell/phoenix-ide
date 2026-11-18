use super::*;
use crate::runtime::traits::MessageStore;
use crate::runtime::{
    AddressedConversationEventDispatcher, ConversationEventDispatcher, ConversationHandle,
    RuntimeManager,
};

#[derive(Clone, Copy)]
enum Route {
    Acknowledged,
    Addressed,
}

async fn register_actor(
    mut runtime: BatchRuntime,
) -> (
    Arc<RuntimeManager>,
    ConversationHandle,
    tokio::task::JoinHandle<RuntimeExitDisposition>,
    watch::Receiver<ConvState>,
) {
    let manager = Arc::new(crate::runtime::scope_liveness_tests::test_manager().await);
    manager
        .db()
        .create_conversation("ordinary-batch", "ordinary-batch", "/tmp", true, None, None)
        .await
        .unwrap();
    let (acknowledged_event_tx, acknowledged_event_rx) = mpsc::channel(32);
    let (event_tx, acknowledged_event_tx, cancellation_input_rx) =
        crate::runtime::cancellation_inbox::channel(
            runtime.event_tx.clone(),
            acknowledged_event_tx,
        );
    runtime = runtime.with_cancellation_input_receiver(cancellation_input_rx);
    runtime = runtime
        .with_acknowledged_event_receiver(acknowledged_event_rx)
        .with_fatal_local_authority_fence(manager.fatal_local_authority_fence.clone());
    let (state_tx, state_rx) = watch::channel(runtime.state.clone());
    let handle = ConversationHandle {
        event_tx,
        acknowledged_event_tx,
        handoff_review_tx: None,
        turn_trigger: runtime.turn_trigger_slot(),
        broadcast_tx: runtime.broadcast_tx.clone(),
        identity: Arc::new(()),
        state_rx: state_rx.clone(),
        startup_llm_recovery_ack: Arc::new(tokio::sync::Mutex::new(None)),
    };
    manager
        .runtimes
        .write()
        .await
        .insert("ordinary-batch".into(), handle.clone());
    let actor = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    (manager, handle, actor, state_rx)
}

async fn deliver_cancel(manager: Arc<RuntimeManager>, route: Route) -> Result<(), String> {
    match route {
        Route::Acknowledged => manager.send_event("ordinary-batch", user_cancel()).await,
        Route::Addressed => {
            AddressedConversationEventDispatcher::new(manager)
                .dispatch("ordinary-batch", user_cancel())
                .await
        }
    }
}

async fn cancellation_interrupts_held_sibling_commit(route: Route) {
    let mut content = calls(6, "stubborn-command");
    content[1] = ContentBlock::tool_use(
        "call-1",
        "bash",
        serde_json::json!({"op":"run","cmd":"fast-command"}),
    );
    let (runtime, mut starts, _directory) = prepared_batch(content).await;
    let storage = Arc::clone(&runtime.storage);
    let tools = Arc::clone(&runtime.tool_executor);
    let llm = Arc::clone(&runtime.llm_client);
    let release_fast = Arc::new(Semaphore::new(0));
    *tools.fast_release.lock().unwrap() = Some(release_fast.clone());
    let (write_entered, release_write) = storage.gate_state_update();
    let (manager, handle, actor, mut states) = register_actor(runtime).await;
    for _ in 0..4 {
        tokio::time::timeout(Duration::from_secs(60), starts.recv())
            .await
            .unwrap()
            .unwrap();
    }
    release_fast.add_permits(1);
    tokio::time::timeout(Duration::from_secs(60), write_entered)
        .await
        .unwrap()
        .unwrap();
    let tokens: Vec<_> = tools
        .tokens
        .lock()
        .unwrap()
        .iter()
        .filter(|(command, _)| command == "stubborn-command")
        .map(|(_, token)| token.clone())
        .collect();
    assert_eq!(tokens.len(), 3);
    let cancellation = match route {
        Route::Addressed => {
            deliver_cancel(manager.clone(), route).await.unwrap();
            None
        }
        Route::Acknowledged => {
            let (acknowledgement, accepted) = oneshot::channel();
            let (retirement, retired) = oneshot::channel();
            handle
                .acknowledged_event_tx
                .send(AcknowledgedEventRequest {
                    event: user_cancel(),
                    acknowledgement,
                    retirement: retired,
                })
                .await
                .unwrap();
            Some(tokio::spawn(async move {
                let _retirement = retirement;
                accepted
                    .await
                    .map_err(|error| error.to_string())?
                    .map(|_| ())
            }))
        }
    };
    tokio::time::pause();
    let interrupted_before_write_receipt = tokio::time::timeout(
        Duration::from_millis(100),
        futures::future::join_all(tokens.iter().map(CancellationToken::cancelled)),
    )
    .await
    .is_ok();
    let acknowledged_before_receipt = cancellation
        .as_ref()
        .is_some_and(tokio::task::JoinHandle::is_finished);
    let launched_while_held = starts.try_recv().is_ok();
    let requests_while_held = llm.recorded_requests().len();
    tokio::time::advance(CANCELLATION_DEADLINE + Duration::from_millis(1)).await;
    let retired_before_write_receipt = tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_finished(&tools, "stubborn-command", 3),
    )
    .await
    .is_ok();
    release_write.send(()).unwrap();
    let settled_without_renewed_grace = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    if !settled_without_renewed_grace {
        tokio::time::advance(CANCELLATION_DEADLINE).await;
        tokio::time::timeout(
            Duration::from_secs(60),
            states.wait_for(|state| matches!(state, ConvState::Idle)),
        )
        .await
        .unwrap()
        .unwrap();
    }
    tokio::time::resume();
    if let Some(cancellation) = cancellation {
        tokio::time::timeout(Duration::from_secs(60), cancellation)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    handle.event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        actor.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    manager.runtimes.write().await.remove("ordinary-batch");
    assert!(interrupted_before_write_receipt, "actual Cancel must signal every running sibling within 100ms while its owning commit remains held");
    assert!(!acknowledged_before_receipt, "resource interruption cannot acknowledge semantic cancellation before write classification");
    assert!(retired_before_write_receipt, "uncooperative tasks must retire at the original cutoff while the issued write remains held");
    assert!(!launched_while_held);
    assert_eq!(requests_while_held, 0);
    assert!(
        settled_without_renewed_grace,
        "classifying a slow write cannot renew the original cancellation drain cutoff"
    );
    assert_eq!(tools.tokens.lock().unwrap().len(), 4);
    assert!(llm.recorded_requests().is_empty());
}

#[tokio::test]
async fn acknowledged_cancel_interrupts_running_siblings_during_held_commit() {
    cancellation_interrupts_held_sibling_commit(Route::Acknowledged).await;
}

#[tokio::test]
async fn addressed_cancel_interrupts_running_siblings_during_held_commit() {
    cancellation_interrupts_held_sibling_commit(Route::Addressed).await;
}

async fn final_checkpoint_cancellation(queued_steer: bool) {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, events, mut starts, _) =
        runtime_with_calls(directory.path(), false, calls(2, "fast-command"));
    let runtime = runtime.with_steering_projection_gate(Arc::new(tokio::sync::Mutex::new(())));
    let fast_release = Arc::new(Semaphore::new(0));
    *runtime.tool_executor.fast_release.lock().unwrap() = Some(fast_release.clone());
    let llm = Arc::clone(&runtime.llm_client);
    let storage = Arc::clone(&runtime.storage);
    let (write_entered, release_write) = storage.gate_tool_round_persist();
    let (manager, handle, actor, mut states) = register_actor(runtime).await;
    send_user(&events).await;
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(60), starts.recv())
            .await
            .unwrap()
            .unwrap();
    }
    if queued_steer {
        let entry = phoenix_core::domain::sm_event::SteerEntry {
            origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
            text: "keep this instruction".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: "queued-instruction".into(),
            user_agent: None,
            skill_invocation: None,
        };
        storage.set_steering_queue("ordinary-batch", vec![entry.clone()]);
        manager
            .send_acknowledged_event(
                "ordinary-batch",
                Event::SteerMessage {
                    origin: entry.origin,
                    text: entry.text,
                    llm_text: entry.llm_text,
                    images: entry.images,
                    files: entry.files,
                    message_id: entry.message_id,
                    user_agent: entry.user_agent,
                    skill_invocation: entry.skill_invocation,
                },
            )
            .await
            .unwrap();
    }
    fast_release.add_permits(2);
    tokio::time::timeout(Duration::from_secs(60), write_entered)
        .await
        .unwrap()
        .unwrap();
    deliver_cancel(manager.clone(), Route::Addressed)
        .await
        .unwrap();
    assert_eq!(llm.recorded_requests().len(), 1);
    assert_eq!(storage.capacity_plans.lock().unwrap().len(), 1);
    let cancellation_commit = queued_steer.then(|| storage.gate_state_update());
    assert!(storage
        .load_active_direct_turn("ordinary-batch")
        .await
        .unwrap()
        .is_none());
    release_write.send(()).unwrap();
    let before_cancellation_commit = if let Some((entered, release)) = cancellation_commit {
        let entered = tokio::time::timeout(Duration::from_secs(60), entered).await;
        let plans = storage.capacity_plans.lock().unwrap().len();
        let _ = release.send(());
        Some((matches!(entered, Ok(Ok(()))), plans))
    } else {
        None
    };
    let idle = tokio::time::timeout(
        Duration::from_secs(60),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .map(|result| result.map(|_| ()));
    let _ = handle.event_tx.send(Event::Shutdown).await;
    let exit = actor.await.unwrap();
    manager.runtimes.write().await.remove("ordinary-batch");
    assert!(
        matches!(idle, Ok(Ok(()))),
        "actor did not settle cancellation: {idle:?}; {exit:?}"
    );
    assert_eq!(exit, RuntimeExitDisposition::CoordinatedShutdown);
    if queued_steer {
        assert!(
            storage
                .get_all_messages("ordinary-batch")
                .iter()
                .any(|message| message.message_id == "queued-instruction"),
            "required steering commit must preserve the instruction in history"
        );
    }
    if let Some((entered, plans)) = before_cancellation_commit {
        assert!(
            entered,
            "the legacy Cancel owning state write must be observed"
        );
        assert_eq!(
            plans, 1,
            "no old-round model tail may be admitted before the pending Cancel commits"
        );
    }
    let expected_requests = if queued_steer { 2 } else { 1 };
    assert_eq!(
        llm.recorded_requests().len(),
        expected_requests,
        "only the legitimate post-Cancel queued instruction may request another response"
    );
    assert_eq!(
        storage.capacity_plans.lock().unwrap().len(),
        expected_requests
    );
}

#[tokio::test]
async fn cancellation_during_final_atomic_checkpoint_prevents_model_tail() {
    final_checkpoint_cancellation(false).await;
}

#[tokio::test]
async fn cancellation_during_final_checkpoint_with_queued_steer_prevents_model_tail() {
    final_checkpoint_cancellation(true).await;
}

async fn wait_for_finished(tools: &ControlledOrdinaryTools, command: &str, count: usize) {
    loop {
        let finished = tools.execution_finished.notified();
        if tools
            .finished
            .lock()
            .unwrap()
            .iter()
            .filter(|name| name.as_str() == command)
            .count()
            >= count
        {
            return;
        }
        finished.await;
    }
}

#[tokio::test]
async fn expired_wait_retires_only_its_task_while_a_sibling_commit_is_held() {
    let content = vec![
        ContentBlock::tool_use(
            "slow",
            "bash",
            serde_json::json!({"op":"run","cmd":"slow-command"}),
        ),
        ContentBlock::tool_use(
            "fast",
            "bash",
            serde_json::json!({"op":"run","cmd":"fast-command"}),
        ),
        ContentBlock::tool_use(
            "wait",
            "bash",
            serde_json::json!({"op":"wait","handle":"independent-process","wait_seconds":0}),
        ),
    ];
    let (runtime, mut starts, _directory) = prepared_batch(content).await;
    let storage = runtime.storage.clone();
    let tools = runtime.tool_executor.clone();
    let release_fast = Arc::new(Semaphore::new(0));
    *tools.fast_release.lock().unwrap() = Some(release_fast.clone());
    let (entered, release_write) = storage.gate_state_update();
    let (manager, handle, actor, mut states) = register_actor(runtime).await;
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(60), starts.recv())
            .await
            .unwrap()
            .unwrap();
    }
    release_fast.add_permits(1);
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(BASH_WAIT_EXECUTION_GRACE + Duration::from_millis(1)).await;
    let wait_retired = tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_finished(&tools, "held-wait", 1),
    )
    .await
    .is_ok();
    let no_tokens_cancelled = tools
        .tokens
        .lock()
        .unwrap()
        .iter()
        .all(|(_, token)| !token.is_cancelled());
    let no_premature_result = matches!(&*states.borrow(), ConvState::ToolExecuting { completed_results, .. } if completed_results.is_empty());
    release_write.send(()).unwrap();
    deliver_cancel(manager.clone(), Route::Addressed)
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(60),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::resume();
    handle.event_tx.send(Event::Shutdown).await.unwrap();
    actor.await.unwrap();
    manager.runtimes.write().await.remove("ordinary-batch");
    assert!(
        wait_retired,
        "expired Wait task must retire while the other invocation's exact write remains pinned"
    );
    assert!(
        no_tokens_cancelled,
        "wait fallback cannot cancel its process token or an independent call"
    );
    assert!(
        no_premature_result,
        "resource-only retirement cannot publish an uncommitted result"
    );
}
