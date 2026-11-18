use super::*;
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
    let capacity = match route {
        Route::Acknowledged => handle.acknowledged_event_tx.capacity(),
        Route::Addressed => handle.event_tx.capacity(),
    };
    let cancellation = tokio::spawn(deliver_cancel(manager.clone(), route));
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let remaining = match route {
                Route::Acknowledged => handle.acknowledged_event_tx.capacity(),
                Route::Addressed => handle.event_tx.capacity(),
            };
            if remaining < capacity || tokens.iter().all(CancellationToken::is_cancelled) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual route enqueues its request or already interrupts the active controls");
    tokio::time::pause();
    let interrupted_before_write_receipt = tokio::time::timeout(
        Duration::from_millis(100),
        futures::future::join_all(tokens.iter().map(CancellationToken::cancelled)),
    )
    .await
    .is_ok();
    let acknowledged_before_receipt =
        matches!(route, Route::Acknowledged) && cancellation.is_finished();
    let launched_while_held = starts.try_recv().is_ok();
    let requests_while_held = llm.recorded_requests().len();
    tokio::time::advance(CANCELLATION_DEADLINE + Duration::from_millis(1)).await;
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
    tokio::time::timeout(Duration::from_secs(60), cancellation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    handle.event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        actor.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    manager.runtimes.write().await.remove("ordinary-batch");
    assert!(interrupted_before_write_receipt, "actual Cancel must signal every running sibling within 100ms while its owning commit remains held");
    assert!(!acknowledged_before_receipt, "resource interruption cannot acknowledge semantic cancellation before write classification");
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

#[tokio::test]
async fn cancellation_during_final_atomic_checkpoint_prevents_model_tail() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, events, mut starts, _) =
        runtime_with_calls(directory.path(), false, calls(2, "fast-command"));
    let llm = Arc::clone(&runtime.llm_client);
    let storage = Arc::clone(&runtime.storage);
    let (write_entered, release_write) = storage.gate_tool_round_persist();
    let (manager, handle, actor, mut states) = register_actor(runtime).await;
    send_user(&events).await;
    tokio::time::timeout(Duration::from_secs(60), write_entered)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..2 {
        assert!(
            starts.try_recv().is_ok(),
            "both calls executed before their atomic checkpoint"
        );
    }
    deliver_cancel(manager.clone(), Route::Addressed)
        .await
        .unwrap();
    assert_eq!(llm.recorded_requests().len(), 1);
    assert_eq!(storage.capacity_plans.lock().unwrap().len(), 1);
    release_write.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(60),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .unwrap()
    .unwrap();
    handle.event_tx.send(Event::Shutdown).await.unwrap();
    actor.await.unwrap();
    manager.runtimes.write().await.remove("ordinary-batch");
    assert_eq!(llm.recorded_requests().len(), 1, "observed cancellation must precede the next provider request after the actual atomic checkpoint");
    assert_eq!(storage.capacity_plans.lock().unwrap().len(), 1, "no second model request may be admitted, even if its task would be aborted before its first poll");
}
