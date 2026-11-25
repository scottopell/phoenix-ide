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

async fn enqueue_cancel(
    manager: &Arc<RuntimeManager>,
    handle: &ConversationHandle,
    route: Route,
) -> Option<tokio::task::JoinHandle<Result<(), String>>> {
    match route {
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
    }
}

async fn cancellation_interrupts_held_sibling_commit(route: Route) {
    let mut content = calls(10, "stubborn-command");
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
    for _ in 0..8 {
        next_started(&mut starts).await;
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
    assert_eq!(tokens.len(), 7);
    let cancellation = enqueue_cancel(&manager, &handle, route).await;
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
        wait_for_finished(&tools, "stubborn-command", 7),
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
    assert_eq!(storage.get_all_messages("ordinary-batch").iter().filter(|message| {
        matches!(&message.content, MessageContent::System(content) if content.text.starts_with("Cancellation completed."))
    }).count(), 1, "actual resource retirement must retain its one degraded-cancellation notice through deferred settlement");
    assert_eq!(tools.tokens.lock().unwrap().len(), 8);
    assert!(llm.recorded_requests().is_empty());
}

#[tokio::test]
async fn acknowledged_cancel_interrupts_running_siblings_during_held_commit() {
    cancellation_interrupts_held_sibling_commit(Route::Acknowledged).await;
}

#[tokio::test]
async fn stop_keeps_batch_commit_and_cutoff_live_while_close_holds_source_custody() {
    use native_close::source_entry::{ActorEntryResult, SourceEntryController, WorkerEntryResult};
    use phoenix_core::domain::{db_schema::SourceToolCall, product_conversation::ProductConversationId};
    use phoenix_db::native_close::{NativeCloseCommand, NativeCloseIntent};

    let mut content = calls(2, "stubborn-command");
    content[1] = ContentBlock::tool_use(
        "call-1",
        "bash",
        serde_json::json!({"op":"run","cmd":"fast-command"}),
    );
    let (mut runtime, mut starts, _directory) = prepared_batch(content).await;
    let tools = Arc::clone(&runtime.tool_executor);
    let release_fast = Arc::new(Semaphore::new(0));
    *tools.fast_release.lock().unwrap() = Some(release_fast.clone());
    let (write_entered, release_write) = runtime.storage.gate_state_update();

    // Retain an entered Close leaf at the custody seam, independently of the
    // foreground batch state. The real lease, not an artificial wait in Stop,
    // must exclude semantic cancellation until released.
    let command = NativeCloseCommand::new(
        "ordinary-batch".into(),
        SourceToolCall {
            message_id: "close-source".into(),
            tool_use_id: "close-call".into(),
        },
        NativeCloseIntent::Initial { target: ProductConversationId::parse("target").unwrap() },
    )
    .unwrap();
    let input = command.intent().source_input();
    let close_state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            "close-call",
            ToolInput::Unknown { name: "close_conversation".into(), input: input.clone() },
        ),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: AssistantMessage::new(
            "close-source",
            vec![ContentBlock::tool_use("close-call", "close_conversation", input)],
            None,
            None,
        ),
    };
    let mut controller = SourceEntryController::new(
        command.clone(),
        tokio::time::Instant::now() + Duration::from_secs(60),
    );
    assert_eq!(controller.revalidate(&close_state, &command), ActorEntryResult::Updated);
    let worker = controller.worker();
    let WorkerEntryResult::Ran(lease) = worker.try_acquire_first_effect(&command) else {
        panic!("Close leaf must hold source custody before Stop");
    };
    runtime.native_close_source_entry = Some(controller);

    let (_manager, handle, actor, mut states) = register_actor(runtime).await;
    for _ in 0..2 {
        next_started(&mut starts).await;
    }
    release_fast.add_permits(1);
    let limit = Duration::from_secs(10);
    tokio::time::timeout(limit, write_entered).await.unwrap().unwrap();
    let sibling_token = tools.tokens.lock().unwrap().iter()
        .find(|(command, _)| command == "stubborn-command").unwrap().1.clone();
    let (acknowledgement, mut stop_ack) = oneshot::channel();
    let (_retirement, retirement) = oneshot::channel();
    tokio::time::pause();
    handle.acknowledged_event_tx.send(AcknowledgedEventRequest {
        event: user_cancel(), acknowledgement, retirement,
    }).await.unwrap();
    tokio::time::timeout(limit, sibling_token.cancelled()).await.unwrap();
    assert!(worker.is_revoked_for_test(), "receipt closes Close admission even while Busy");

    // The sibling cancellation token is the causal witness that Stop has been
    // applied and its original resource cutoff armed. Keep time virtual so a
    // late/rearmed cutoff cannot be hidden by waiting on wall-clock time.
    tokio::time::advance(CANCELLATION_DEADLINE + Duration::from_millis(1)).await;
    tokio::time::timeout(Duration::from_millis(100), wait_for_finished(&tools, "stubborn-command", 1))
        .await
        .expect("original resource cutoff must fire with both custody barriers held");
    tokio::time::resume();
    assert_eq!(stop_ack.try_recv(), Err(oneshot::error::TryRecvError::Empty));

    release_write.send(()).unwrap();
    tokio::time::timeout(limit, states.wait_for(|state| {
        matches!(state, ConvState::ToolExecuting { completed_results, .. }
            if completed_results.iter().any(|result| result.tool_use_id() == "call-1"))
    })).await.unwrap().unwrap();
    assert_eq!(stop_ack.try_recv(), Err(oneshot::error::TryRecvError::Empty));

    drop(lease);
    assert_eq!(
        tokio::time::timeout(limit, stop_ack).await.unwrap().unwrap().unwrap(),
        AcknowledgedEventOutcome::Settled,
    );
    assert!(matches!(*states.borrow(), ConvState::Idle));
    handle.event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(actor.await.unwrap(), RuntimeExitDisposition::CoordinatedShutdown);
}

#[tokio::test]
async fn addressed_cancel_interrupts_running_siblings_during_held_commit() {
    cancellation_interrupts_held_sibling_commit(Route::Addressed).await;
}

async fn queue_instruction(manager: &Arc<RuntimeManager>, storage: &InMemoryStorage) {
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
        next_started(&mut starts).await;
    }
    if queued_steer {
        queue_instruction(&manager, &storage).await;
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
        next_started(&mut starts).await;
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

#[tokio::test]
async fn rejected_cancel_retains_original_resource_cutoff_without_retry() {
    let (runtime, mut starts, _directory) = prepared_batch(calls(10, "stubborn-command")).await;
    let storage = Arc::clone(&runtime.storage);
    let tools = Arc::clone(&runtime.tool_executor);
    let llm = Arc::clone(&runtime.llm_client);
    let (entered, release) = storage.gate_state_update();
    storage.fail_gated_state_update_once();
    let (manager, handle, actor, mut states) = register_actor(runtime).await;
    for _ in 0..8 {
        next_started(&mut starts).await;
    }
    let tokens: Vec<_> = tools
        .tokens
        .lock()
        .unwrap()
        .iter()
        .map(|(_, token)| token.clone())
        .collect();
    tokio::time::pause();
    let rejected = enqueue_cancel(&manager, &handle, Route::Acknowledged)
        .await
        .unwrap();
    let signalled = tokio::time::timeout(
        Duration::from_millis(100),
        futures::future::join_all(tokens.iter().map(CancellationToken::cancelled)),
    )
    .await
    .is_ok();
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    let diagnostic = rejected.await.unwrap().unwrap_err();
    tokio::time::advance(CANCELLATION_DEADLINE + Duration::from_millis(1)).await;
    let retired_without_retry = tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_finished(&tools, "stubborn-command", 8),
    )
    .await
    .is_ok();
    let no_semantic_success = matches!(*states.borrow(), ConvState::ToolExecuting { .. })
        && storage.get_all_messages("ordinary-batch").is_empty();
    let fence_remained_open = !manager
        .fatal_local_authority_fence
        .fatal_authority_loss_is_latched();
    let no_refill = starts.try_recv().is_err() && tools.tokens.lock().unwrap().len() == 8;
    let retry = enqueue_cancel(&manager, &handle, Route::Acknowledged)
        .await
        .unwrap();
    let retry_kept_cutoff = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    if !retry_kept_cutoff {
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
    retry.await.unwrap().unwrap();
    handle.event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        actor.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    manager.runtimes.write().await.remove("ordinary-batch");
    assert!(
        diagnostic.contains("injected gated state update failure"),
        "{diagnostic}"
    );
    assert!(
        signalled,
        "a rejected cancel still interrupts every admitted resource immediately"
    );
    assert!(retired_without_retry, "the original resource cutoff must survive a known cancellation rejection without requiring another request");
    assert!(
        no_semantic_success,
        "resource retirement cannot fabricate a successful rejected cancellation"
    );
    assert!(
        fence_remained_open,
        "the observed rejection must not be reclassified as unknown local authority"
    );
    assert!(no_refill && llm.recorded_requests().is_empty());
    assert!(
        retry_kept_cutoff,
        "an explicit retry must join the same close and original cutoff"
    );
}
