use super::*;
use crate::runtime::executor::RuntimeExitDisposition;
use crate::runtime::testing::{MockLlmClient, MockToolExecutor};
use crate::runtime::traits::{DatabaseStorage, StateStore};
use crate::runtime::{RuntimeManager, SseBroadcaster, SubAgentSpawnRequest};
use crate::state_machine::ConvContext;
use crate::tools::BrowserSessionManager;
use phoenix_core::domain::sm_state::{
    AssistantMessage, ConversationExecutionEnvironment, SpawnAgentsInput, SubAgentMode,
    SubAgentOutcome, SubAgentTask, ToolInput,
};
use phoenix_llm::{ContentBlock, ModelRegistry};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};

type TestRuntime = ConversationRuntime<DatabaseStorage, Arc<MockLlmClient>, Arc<MockToolExecutor>>;

async fn pending_native_spawn() -> (Arc<RuntimeManager>, TestRuntime, SubAgentSpawnRequest) {
    let manager = Arc::new(crate::runtime::scope_liveness_tests::test_manager().await);
    let parent = manager
        .db()
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let call = ToolCall::new(
        "spawn",
        ToolInput::SpawnAgents(SpawnAgentsInput {
            tasks: (0..2)
                .map(|index| SubAgentTask {
                    task: format!("task {index}"),
                    cwd: None,
                    mode: Some(SubAgentMode::Explore),
                    execution: None,
                    max_turns: Some(1),
                    agent_type: None,
                })
                .collect(),
        }),
    );
    let state = ConvState::ToolExecuting {
        current_tool: call.clone(),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: AssistantMessage::new(
            "assistant".into(),
            vec![ContentBlock::tool_use(
                "spawn",
                "spawn_agents",
                call.input.to_value(),
            )],
            None,
            None,
        ),
    };
    manager
        .db()
        .update_conversation_state(&parent.id, &state)
        .await
        .unwrap();
    let mut context = ConvContext::new(&parent.id, "/tmp".into(), "gpt-5.6-sol", 200_000, 16_384);
    context.is_coordinator = true;
    context.execution_environment = ConversationExecutionEnvironment::NoFilesystem;
    context.resource_scope = phoenix_core::work_scope::ResourceScopeKey::Coordinator;
    let (event_tx, event_rx) = mpsc::channel(32);
    let (spawn_tx, mut spawn_rx) = mpsc::channel(1);
    let (cancel_tx, _) = mpsc::channel(1);
    let mut runtime = ConversationRuntime::new(
        context,
        state,
        DatabaseStorage::new(manager.db().clone()),
        Arc::new(MockLlmClient::new("gpt-5.6-sol")),
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new(&phoenix_llm::LlmConfig {
            openai_api_key: Some("test-key".into()),
            ..Default::default()
        })),
        crate::terminal::ActiveTerminals::new(),
        event_rx,
        event_tx,
        SseBroadcaster::new(128, 0),
    )
    .with_spawn_channels(spawn_tx, cancel_tx)
    .with_fatal_local_authority_fence(Arc::clone(&manager.fatal_local_authority_fence));
    assert!(runtime.start_spawn_agents_tool(&call).unwrap().is_none());
    let request = spawn_rx
        .try_recv()
        .expect("native admission request was submitted");
    (manager, runtime, request)
}

async fn cancel(runtime: &mut TestRuntime) {
    runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
}

async fn issue_native_spawn(
    manager: &Arc<RuntimeManager>,
    request: SubAgentSpawnRequest,
) -> tokio::task::JoinHandle<()> {
    let mut admitted = request.control.subscribe_batch_admitted();
    let manager = Arc::clone(manager);
    let task = tokio::spawn(async move { manager.handle_spawn_request(request).await });
    tokio::time::timeout(
        Duration::from_secs(60),
        admitted.wait_for(|admitted| *admitted),
    )
    .await
    .expect("the real native batch admission committed")
    .unwrap();
    task
}

#[tokio::test]
async fn native_parent_authority_rejection_prevents_child_admission() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let control = Arc::clone(&request.control);
    sqlx::query(
        "CREATE TRIGGER reject_native_parent_action BEFORE INSERT ON startup_parent_actions
        BEGIN SELECT RAISE(ABORT, 'native parent authority rejected'); END",
    )
    .execute(manager.db().pool())
    .await
    .unwrap();
    manager.handle_spawn_request(request).await;
    let admission = runtime.observe_native_spawn().await;
    let event = runtime
        .native_spawn_observed(admission)
        .await
        .unwrap()
        .unwrap();
    let Event::ToolComplete {
        tool_use_id,
        result,
    } = event
    else {
        panic!("known parent-write failure must return its paired rejection")
    };
    assert_eq!(tool_use_id, "spawn");
    assert!(result.is_error() && result.output().contains("native parent authority rejected"));
    let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    let actions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM startup_parent_actions")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    assert_eq!((children, actions), (0, 0));
    assert!(!manager.fatal_local_authority_fence.is_closed());
    assert_eq!(control.publication_count(), 1);
}

#[tokio::test]
async fn native_full_submission_stays_owned_and_cancels_in_the_real_loop() {
    let (manager, mut runtime, filler) = pending_native_spawn().await;
    runtime
        .native_spawn_handoff
        .take()
        .unwrap()
        .request_cancel();
    if let ConvState::ToolExecuting {
        assistant_message, ..
    } = &mut runtime.state
    {
        assistant_message.message_id = "full-submission-round".into();
    }
    manager
        .db()
        .update_conversation_state(&runtime.context.conversation_id, &runtime.state)
        .await
        .unwrap();
    let (sender, mut requests) = mpsc::channel(1);
    sender.try_send(filler).unwrap();
    runtime.spawn_tx = Some(sender);
    let ConvState::ToolExecuting { current_tool, .. } = &runtime.state else {
        unreachable!()
    };
    let current_tool = current_tool.clone();
    assert!(runtime
        .start_spawn_agents_tool(&current_tool)
        .unwrap()
        .is_none());
    let handoff = runtime.native_spawn_handoff.as_mut().unwrap();
    let control = Arc::clone(&handoff.control);
    let NativeSpawnPhase::Submission { send, .. } = &mut handoff.phase else {
        panic!("the full channel must retain submission")
    };
    let future = send.get_mut().unwrap();
    let original = std::mem::replace(future, Box::pin(std::future::pending()));
    let (entered, observed) = tokio::sync::oneshot::channel();
    *future = Box::pin(async move {
        let _ = entered.send(());
        original.await
    });
    let event_tx = runtime.event_tx.clone();
    let (state_tx, mut states) = watch::channel(runtime.state.clone());
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    tokio::time::timeout(Duration::from_secs(60), observed)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    event_tx
        .send(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    let interrupted = tokio::time::timeout(Duration::from_millis(100), control.cancelled())
        .await
        .is_ok();
    tokio::time::resume();
    let settled_before_capacity = tokio::time::timeout(
        Duration::from_secs(60),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    let filler = requests.try_recv().unwrap();
    let no_late_request = matches!(requests.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        task.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    manager.handle_spawn_request(filler).await;
    assert!(interrupted, "native interruption must occur within 100ms");
    assert!(
        settled_before_capacity,
        "cancellation must not await bounded send capacity"
    );
    assert!(no_late_request);
    assert!(control.is_closed());
    assert_eq!(control.publication_count(), 1);
    assert!(!manager.fatal_local_authority_fence.is_closed());
    let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    let actions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM startup_parent_actions")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    assert_eq!((children, actions), (0, 0));
}

#[tokio::test]
async fn native_parent_write_owner_loss_or_cutoff_cannot_claim_unissued_batch() {
    for cutoff in [false, true] {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        *manager.native_admission_prewrite_barrier.lock().await = Some(Arc::clone(&barrier));
        let mut issued = request.control.subscribe_issue_attempt();
        let task = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.handle_spawn_request(request).await }
        });
        barrier.wait().await;
        let held_connection = manager.db().pool().acquire().await.unwrap();
        barrier.wait().await;
        tokio::time::timeout(
            Duration::from_secs(60),
            issued.wait_for(|issued| *issued == Some(true)),
        )
        .await
        .unwrap()
        .unwrap();
        let next = crate::state_machine::transition(
            &runtime.state,
            &runtime.context,
            Event::UserCancel {
                reason: None,
                cause: crate::state_machine::event::CancelCause::UserRequested,
            },
        )
        .unwrap();
        runtime.state = next.new_state;
        runtime
            .native_spawn_handoff
            .as_mut()
            .unwrap()
            .request_cancel();
        if cutoff {
            assert!(matches!(
                runtime.classify_native_spawn_cutoff("spawn").await,
                NativeSpawnCutoff::Stop
            ));
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(manager
            .fatal_local_authority_fence
            .fatal_authority_loss_is_latched());
        assert!(runtime.native_spawn_handoff.is_some());
        assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
        drop(held_connection);
        let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs")
            .fetch_one(manager.db().pool())
            .await
            .unwrap();
        assert_eq!(children, 0);
        assert!(runtime.llm_client.recorded_requests().is_empty());
    }
}

async fn install_ordinary_tool_policy(runtime: &mut TestRuntime) -> Arc<MockToolExecutor> {
    let tools = Arc::new(
        MockToolExecutor::new()
            .with_tool("bash", crate::tools::ToolOutput::success("must not run")),
    );
    runtime.tool_executor = Arc::new(Arc::clone(&tools));
    let definitions = tools.definitions().await;
    runtime
        .storage
        .prepare_tool_availability(
            &runtime.context.conversation_id,
            "native-cancel-failure",
            None,
            &definitions,
            &std::collections::BTreeSet::from(["bash".into()]),
            &[],
            &[],
        )
        .await
        .unwrap();
    tools
}

#[tokio::test]
async fn native_cancel_persistence_failure_retains_closing_origin_and_stops_queued_work() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let tools = install_ordinary_tool_policy(&mut runtime).await;
    let later = ToolCall::new(
        "later",
        ToolInput::from(crate::tools::BashToolInput::run("git status --short")),
    );
    if let ConvState::ToolExecuting {
        remaining_tools,
        assistant_message,
        ..
    } = &mut runtime.state
    {
        remaining_tools.push(later.clone());
        assistant_message.content.push(ContentBlock::tool_use(
            &later.id,
            later.name(),
            later.input.to_value(),
        ));
    }
    manager
        .db()
        .update_conversation_state(&runtime.context.conversation_id, &runtime.state)
        .await
        .unwrap();
    let control = Arc::clone(&request.control);
    let task = issue_native_spawn(&manager, request).await;
    let admitted = runtime.observe_native_spawn().await;
    assert!(matches!(
        &admitted,
        NativeSpawnObservation::Admission(Ok(SubAgentAdmissionReceipt {
            outcome: SubAgentAdmissionResponse::Admitted,
            ..
        }))
    ));
    sqlx::query("CREATE TRIGGER reject_native_cancel_checkpoint BEFORE INSERT ON execution_cancel_observations
        BEGIN SELECT RAISE(ABORT, 'native cancellation persistence failed'); END")
        .execute(manager.db().pool()).await.unwrap();
    let error = runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap_err();
    assert!(error.contains("native cancellation persistence failed"));
    let signalled_before_failed_write = control.is_cancelled();
    if !signalled_before_failed_write {
        runtime
            .native_spawn_handoff
            .as_mut()
            .unwrap()
            .request_cancel();
    }
    assert!(runtime
        .native_spawn_observed(admitted)
        .await
        .unwrap()
        .is_none());
    task.await.unwrap();
    let closure = runtime.observe_native_spawn().await;
    if let Some(event) = runtime.native_spawn_observed(closure).await.unwrap() {
        runtime.process_event(event).await.unwrap();
    }
    let ordinary_generation_before_teardown = runtime.serial_dispatch().execution.generation();
    runtime.abort_external_effects_in_memory();
    assert!(
        signalled_before_failed_write,
        "native interruption must precede the failing cancellation evidence write"
    );
    assert!(matches!(
        &runtime.state,
        ConvState::Error {
            error_kind: crate::db::ErrorKind::SubAgentError,
            message, ..
        } if message.contains("native cancellation persistence failed")
    ));
    assert_eq!(ordinary_generation_before_teardown, 0);
    assert!(tools.recorded_executions().is_empty());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    assert!(!manager.fatal_local_authority_fence.is_closed());
    let owed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NULL AND terminal_at_unix_micros IS NOT NULL AND initial_dispatch_claimed_at_unix_micros IS NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(owed, 2);
    let messages = manager
        .db()
        .get_messages(&runtime.context.conversation_id)
        .await
        .unwrap();
    assert!(!messages
        .iter()
        .any(|message| serde_json::to_string(&message.content)
            .unwrap()
            .contains("Cancellation completed")));
}

#[tokio::test]
async fn native_admitted_manager_loss_before_closure_write_is_known_failure() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let control = Arc::clone(&request.control);
    let manager_task = issue_native_spawn(&manager, request).await;
    let admission = runtime.observe_native_spawn().await;
    assert!(matches!(
        runtime.native_spawn_observed(admission).await.unwrap(),
        Some(Event::SpawnAgentsComplete { .. })
    ));
    manager_task.abort();
    assert!(manager_task.await.unwrap_err().is_cancelled());
    cancel(&mut runtime).await;
    let receipt = runtime
        .native_spawn_handoff
        .as_mut()
        .unwrap()
        .closure
        .try_recv()
        .expect("manager lifetime publishes its known pre-closure-write failure");
    assert!(matches!(
        receipt.evidence,
        NativeSpawnTerminalEvidence::Failed(_)
    ));
    assert!(runtime
        .native_spawn_observed(NativeSpawnObservation::Closure(Ok(receipt)))
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        runtime.state,
        ConvState::Error {
            error_kind: crate::db::ErrorKind::SubAgentError,
            ..
        }
    ));
    assert!(!manager.fatal_local_authority_fence.is_closed());
    assert_eq!(control.publication_count(), 1);
    let owed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NULL AND terminal_at_unix_micros IS NULL AND initial_dispatch_claimed_at_unix_micros IS NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(owed, 2);
    assert!(runtime.tool_executor.recorded_executions().is_empty());
    assert!(runtime.llm_client.recorded_requests().is_empty());
}

#[tokio::test]
async fn native_prewrite_refusal_is_known_without_unknown_authority_or_false_completion() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    manager
        .fatal_local_authority_fence
        .close_for_process_shutdown();
    manager.handle_spawn_request(request).await;
    let response = runtime.observe_native_spawn().await;
    assert!(matches!(
        &response,
        NativeSpawnObservation::Admission(Ok(SubAgentAdmissionReceipt {
            outcome: SubAgentAdmissionResponse::Rejected(_),
            ..
        }))
    ));
    assert!(runtime.native_spawn_observed(response).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(!manager
        .fatal_local_authority_fence
        .fatal_authority_loss_is_latched());

    let (manager, runtime, request) = pending_native_spawn().await;
    let mut lifecycle =
        crate::runtime::native_spawn::NativeSpawnLifecycleOwner::new(Arc::clone(&request.control));
    manager
        .admit_sub_agent_batch(
            &request.invocation.batch_id,
            &request.specs,
            &request.invocation.parent_conversation_id,
            request.parent_scope.clone(),
            request.parallel_work_qualified,
            &mut lifecycle,
        )
        .await
        .unwrap();
    let batch = phoenix_db::SubAgentBatchAdmission {
        batch_id: request.invocation.batch_id.clone(),
        parent_conversation_id: request.invocation.parent_conversation_id.clone(),
        parallel_work_qualified: request.parallel_work_qualified,
        runs: request
            .specs
            .iter()
            .map(|spec| phoenix_db::SubAgentRunAdmission {
                child_conversation_id: spec.agent_id.clone(),
                execution_authority: phoenix_db::SubAgentExecutionAuthority::ReadOnly,
            })
            .collect(),
    };
    manager
        .fatal_local_authority_fence
        .close_for_process_shutdown();
    assert!(matches!(
        manager
            .close_native_spawn_batch(&batch, &mut lifecycle)
            .await,
        NativeSpawnTerminalEvidence::Failed(_)
    ));
    let owed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NULL AND terminal_at_unix_micros IS NULL AND initial_dispatch_claimed_at_unix_micros IS NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(owed, 2);
    assert!(!manager
        .fatal_local_authority_fence
        .fatal_authority_loss_is_latched());
    assert!(matches!(runtime.state, ConvState::ToolExecuting { .. }));
    drop(request);
}

#[tokio::test]
async fn native_run_loop_accepts_cancellation_while_admission_response_is_outstanding() {
    let (manager, runtime, request) = pending_native_spawn().await;
    let origin = request.invocation.clone();
    let control = Arc::clone(&request.control);
    let manager_task = issue_native_spawn(&manager, request).await;
    let event_tx = runtime.event_tx.clone();
    let (state_tx, mut state_rx) = watch::channel(runtime.state.clone());
    event_tx
        .send(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    tokio::time::timeout(Duration::from_secs(60), control.cancelled())
        .await
        .expect("native cancellation disposition was signalled");
    tokio::time::timeout(
        Duration::from_secs(60),
        state_rx.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .expect("native cancellation reaches real parent fan-in")
    .unwrap();
    assert!(manager
        .db()
        .owed_sub_agent_terminals(&origin.parent_conversation_id)
        .await
        .unwrap()
        .is_empty());
    let accepted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NOT NULL AND initial_dispatch_claimed_at_unix_micros IS NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(accepted, 2);
    manager_task.await.unwrap();
    event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        task.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
}

#[tokio::test]
async fn native_cancel_and_late_admission_join_one_closure_then_atomic_fan_in() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let origin = request.invocation.clone();
    let children: Vec<_> = request
        .specs
        .iter()
        .map(|spec| spec.agent_id.clone())
        .collect();
    sqlx::query("CREATE TABLE closure_counts(child TEXT PRIMARY KEY, n INTEGER NOT NULL)")
        .execute(manager.db().pool())
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER count_live_closure AFTER UPDATE OF cancellation_requested_at_unix_micros ON sub_agent_runs BEGIN INSERT INTO closure_counts VALUES (NEW.child_conversation_id, 1) ON CONFLICT(child) DO UPDATE SET n = n + 1; END").execute(manager.db().pool()).await.unwrap();
    let control = Arc::clone(&request.control);
    let manager_task = issue_native_spawn(&manager, request).await;
    cancel(&mut runtime).await;
    let cancellation_entered_at = runtime.state_updated_at;
    let cancellation_deadline = runtime.deadline;
    runtime
        .native_spawn_handoff
        .as_mut()
        .unwrap()
        .request_cancel();
    manager_task.await.unwrap();
    assert_eq!(control.publication_count(), 1);
    let observation = runtime.observe_native_spawn().await;
    let event = runtime
        .native_spawn_observed(observation)
        .await
        .unwrap()
        .expect("one reconciled cancellation receipt");
    assert_eq!(runtime.state_updated_at, cancellation_entered_at);
    assert_eq!(runtime.deadline, cancellation_deadline);
    assert!(
        matches!(&event, Event::ToolAborted { tool_use_id } if *tool_use_id == origin.tool_use_id)
    );
    let persisted = manager
        .db()
        .get_conversation(&origin.parent_conversation_id)
        .await
        .unwrap()
        .state;
    assert!(
        matches!(persisted, ConvState::CancellingTool { ref pending_sub_agents, .. } if pending_sub_agents.iter().map(|member| &member.agent_id).eq(children.iter()))
    );
    for child in &children {
        assert!(!manager
            .db()
            .sub_agent_terminal_is_accepted(child)
            .await
            .unwrap());
        let claims: Option<i64> = sqlx::query_scalar("SELECT initial_dispatch_claimed_at_unix_micros FROM sub_agent_runs WHERE child_conversation_id = ?1").bind(child).fetch_one(manager.db().pool()).await.unwrap();
        assert_eq!(claims, None);
        let count: i64 = sqlx::query_scalar("SELECT n FROM closure_counts WHERE child = ?1")
            .bind(child)
            .fetch_one(manager.db().pool())
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "cancellation and dropped late response joined one transaction"
        );
    }
    runtime.process_event(event).await.unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(runtime.native_spawn_handoff.is_none());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    for child in children {
        assert!(manager
            .db()
            .sub_agent_terminal_is_accepted(&child)
            .await
            .unwrap());
        let before = manager
            .db()
            .get_messages(&origin.parent_conversation_id)
            .await
            .unwrap();
        runtime
            .process_event(Event::SubAgentResult {
                agent_id: child,
                outcome: SubAgentOutcome::Failure {
                    error: "duplicate".into(),
                    error_kind: crate::db::ErrorKind::Cancelled,
                },
            })
            .await
            .unwrap();
        assert_eq!(
            manager
                .db()
                .get_messages(&origin.parent_conversation_id)
                .await
                .unwrap()
                .len(),
            before.len()
        );
    }
}

#[tokio::test]
async fn native_lost_admission_response_reconciles_without_administrative_acceptance() {
    let (manager, mut runtime, mut request) = pending_native_spawn().await;
    let origin = request.invocation.clone();
    let (replacement, dropped) = oneshot::channel();
    drop(dropped);
    drop(std::mem::replace(&mut request.response_tx, replacement));
    manager.handle_spawn_request(request).await;
    let response_loss = runtime.observe_native_spawn().await;
    assert!(runtime
        .native_spawn_observed(response_loss)
        .await
        .unwrap()
        .is_none());
    let closure = runtime.observe_native_spawn().await;
    let completion = runtime
        .native_spawn_observed(closure)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(completion, Event::ToolComplete { ref result, .. } if result.is_error()));
    assert_eq!(
        manager
            .db()
            .owed_sub_agent_terminals(&origin.parent_conversation_id)
            .await
            .unwrap()
            .len(),
        2
    );
    runtime.process_event(completion).await.unwrap();
    assert!(manager
        .db()
        .owed_sub_agent_terminals(&origin.parent_conversation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn native_activation_loss_is_observed_by_retained_admitted_owner() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let invocation = request.invocation.clone();
    let manager_task = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.handle_spawn_request(request).await }
    });
    let response = runtime.observe_native_spawn().await;
    assert!(matches!(
        runtime.native_spawn_observed(response).await.unwrap(),
        Some(Event::SpawnAgentsComplete { .. })
    ));
    assert!(runtime
        .native_spawn_observed(NativeSpawnObservation::Admission(Ok(
            SubAgentAdmissionReceipt {
                invocation,
                outcome: SubAgentAdmissionResponse::Admitted
            }
        )))
        .await
        .unwrap()
        .is_none());
    assert!(runtime.native_spawn_activation_pending());
    runtime.native_spawn_handoff.as_mut().unwrap().phase = NativeSpawnPhase::Closing;
    manager_task.await.unwrap();
    let closure = runtime.observe_native_spawn().await;
    let completion = runtime
        .native_spawn_observed(closure)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(completion, Event::ToolComplete { ref result, .. } if result.is_error()));
    assert!(runtime.native_spawn_handoff.is_none());
    runtime.process_event(completion).await.unwrap();
    assert!(manager
        .db()
        .owed_sub_agent_terminals(&runtime.context.conversation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn native_definite_closure_failure_is_non_resumable_and_keeps_admission() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    sqlx::query("CREATE TRIGGER reject_live_closure BEFORE UPDATE OF terminal_cause ON sub_agent_runs BEGIN SELECT RAISE(ABORT, 'known live closure failure'); END").execute(manager.db().pool()).await.unwrap();
    let manager_task = issue_native_spawn(&manager, request).await;
    cancel(&mut runtime).await;
    manager_task.await.unwrap();
    let closure = runtime.observe_native_spawn().await;
    assert!(runtime
        .native_spawn_observed(closure)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        runtime.state,
        ConvState::Error {
            error_kind: crate::db::ErrorKind::SubAgentError,
            ..
        }
    ));
    assert!(!manager.fatal_local_authority_fence.is_closed());
    let admitted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NULL AND terminal_at_unix_micros IS NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(admitted, 2);
    assert!(manager
        .db()
        .get_messages(&runtime.context.conversation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn native_queued_behind_fork_command_cancellation_cutoff_is_not_sqlite_uncertainty() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let control = Arc::clone(&request.control);
    let mut issue_attempt = control.subscribe_issue_attempt();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    *manager.fork_command_barrier.lock().await = Some(Arc::clone(&barrier));
    manager.start_sub_agent_handler().await;
    let (reply, reply_rx) = oneshot::channel();
    manager
        .fork_cmd_tx
        .send(crate::runtime::fork_resolve::ForkCommand::Dismiss {
            proposal_id: "block-native-consumer".into(),
            reply,
        })
        .await
        .unwrap();
    barrier.wait().await;
    manager.spawn_tx.send(request).await.unwrap();
    cancel(&mut runtime).await;
    runtime.handle_cancelling_tool_timeout().await;
    let fenced_without_any_admission = manager
        .fatal_local_authority_fence
        .fatal_authority_loss_is_latched();
    barrier.wait().await;
    let _dismissal = reply_rx.await.unwrap();
    assert!(
        !fenced_without_any_admission,
        "an exactly owned unissued batch can be revoked without inventing unknown SQLite authority"
    );
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(
        control.is_closed(),
        "revocation permanently closes admission for this exact native command"
    );
    tokio::time::timeout(
        Duration::from_secs(60),
        issue_attempt.wait_for(|issued| *issued == Some(false)),
    )
    .await
    .expect("queued manager reaches revoked command gate")
    .unwrap();
    assert_eq!(control.publication_count(), 1);
    let admitted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    assert_eq!(admitted, 0);
}

#[tokio::test]
async fn native_issued_unresolved_backstop_cannot_acknowledge_or_replace_origin() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let origin = request.invocation.clone();
    assert!(request.control.try_issue());
    let _issued_write = crate::runtime::native_spawn::NativeSpawnWriteAuthority {
        owner: Some(manager.acquire_local_authority_pass().unwrap()),
    };
    cancel(&mut runtime).await;
    runtime.handle_cancelling_tool_timeout().await;
    assert!(runtime
        .native_spawn_handoff
        .as_ref()
        .is_some_and(|owner| owner.invocation == origin));
    assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
    assert!(manager.fatal_local_authority_fence.is_closed());
    assert!(manager
        .db()
        .get_messages(&origin.parent_conversation_id)
        .await
        .unwrap()
        .is_empty());
    drop(request);
}

#[tokio::test]
async fn native_membership_write_failure_cannot_publish_reconciled_or_accept_children() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let manager_task = issue_native_spawn(&manager, request).await;
    cancel(&mut runtime).await;
    manager_task.await.unwrap();
    sqlx::query("CREATE TRIGGER reject_native_membership BEFORE UPDATE OF state ON conversations WHEN json_array_length(json_extract(NEW.state, '$.pending_sub_agents')) > 0 BEGIN SELECT RAISE(ABORT, 'known native membership failure'); END").execute(manager.db().pool()).await.unwrap();
    let closure = runtime.observe_native_spawn().await;
    assert!(runtime
        .native_spawn_observed(closure)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        runtime.state,
        ConvState::Error {
            error_kind: crate::db::ErrorKind::SubAgentError,
            ..
        }
    ));
    assert!(!manager.fatal_local_authority_fence.is_closed());
    assert_eq!(
        manager
            .db()
            .owed_sub_agent_terminals(&runtime.context.conversation_id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(runtime.sub_agent_result_buffer.is_empty());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    assert!(manager
        .db()
        .get_messages(&runtime.context.conversation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn native_unclassified_receipt_closes_shared_fence_even_after_origin_replacement() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let origin = request.invocation.clone();
    runtime.state = ConvState::Idle;
    let result = runtime
        .native_spawn_observed(NativeSpawnObservation::Closure(Ok(
            NativeSpawnClosureReceipt {
                invocation: origin,
                evidence: NativeSpawnTerminalEvidence::Unclassified,
            },
        )))
        .await;
    assert!(result.is_err());
    assert!(manager.fatal_local_authority_fence.is_closed());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    drop(request);
}

#[tokio::test]
async fn native_missing_prewrite_owner_uses_affirmative_no_command_proof() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    drop(request);
    cancel(&mut runtime).await;
    let observation = runtime.observe_native_spawn().await;
    let event = runtime
        .native_spawn_observed(observation)
        .await
        .unwrap()
        .expect("exact unissued owner proves no admission");
    runtime.process_event(event).await.unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(!manager.fatal_local_authority_fence.is_closed());
    let admitted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs")
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
    assert_eq!(admitted, 0);
}

#[tokio::test]
async fn native_postcommit_activation_receiver_loss_stops_effect_tail_without_fencing() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let mut lifecycle =
        crate::runtime::native_spawn::NativeSpawnLifecycleOwner::new(Arc::clone(&request.control));
    manager
        .admit_sub_agent_batch(
            &request.invocation.batch_id,
            &request.specs,
            &request.invocation.parent_conversation_id,
            request.parent_scope,
            request.parallel_work_qualified,
            &mut lifecycle,
        )
        .await
        .unwrap();
    let next = ToolCall::new(
        "later",
        ToolInput::from(crate::tools::BashToolInput::run("true")),
    );
    if let ConvState::ToolExecuting {
        remaining_tools,
        assistant_message,
        ..
    } = &mut runtime.state
    {
        remaining_tools.push(next.clone());
        assistant_message.content.push(ContentBlock::tool_use(
            "later",
            next.name(),
            next.input.to_value(),
        ));
    }
    manager
        .db()
        .update_conversation_state(&runtime.context.conversation_id, &runtime.state)
        .await
        .unwrap();
    request
        .response_tx
        .send(SubAgentAdmissionReceipt {
            invocation: request.invocation.clone(),
            outcome: SubAgentAdmissionResponse::Admitted,
        })
        .unwrap();
    drop(request.activation_rx);
    let response = runtime.observe_native_spawn().await;
    let event = runtime
        .native_spawn_observed(response)
        .await
        .unwrap()
        .unwrap();
    runtime.process_event(event).await.unwrap();
    assert!(matches!(
        runtime.state,
        ConvState::Error {
            error_kind: crate::db::ErrorKind::SubAgentError,
            ..
        }
    ));
    assert!(!manager.fatal_local_authority_fence.is_closed());
    assert!(runtime.tool_executor.recorded_executions().is_empty());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    assert!(runtime
        .native_spawn_handoff
        .as_ref()
        .is_some_and(|handoff| handoff.invocation == request.invocation));
    let accepted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NOT NULL OR initial_dispatch_claimed_at_unix_micros IS NOT NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(accepted, 0);
    assert!(request.control.publish(NativeSpawnTerminalEvidence::Failed(
        "activation owner was lost after classified admission".into()
    )));
    let missing_receipt = runtime.observe_native_spawn().await;
    assert!(runtime
        .native_spawn_observed(missing_receipt)
        .await
        .unwrap()
        .is_none());
    assert!(!manager.fatal_local_authority_fence.is_closed());
}

#[tokio::test]
async fn native_replaced_admitted_origin_cannot_release_transferred_activation() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let manager_task = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.handle_spawn_request(request).await }
    });
    let response = runtime.observe_native_spawn().await;
    assert!(matches!(
        runtime.native_spawn_observed(response).await.unwrap(),
        Some(Event::SpawnAgentsComplete { .. })
    ));
    let members = runtime
        .native_spawn_handoff
        .as_ref()
        .unwrap()
        .members
        .clone();
    if let ConvState::ToolExecuting {
        assistant_message,
        pending_sub_agents,
        ..
    } = &mut runtime.state
    {
        assistant_message.message_id = "successor-assistant".into();
        *pending_sub_agents = members;
    }
    let successor = runtime.state.clone();
    let mut authority = runtime.admit_authoritative_effect().unwrap();
    runtime
        .persist_state_effect(false, &mut authority)
        .await
        .unwrap();
    assert_eq!(runtime.state, successor);
    manager_task.await.unwrap();
    assert_eq!(manager.db().owed_sub_agent_terminals(&runtime.context.conversation_id).await.unwrap().len(), 2, "replaced native origin must withhold activation even when a successor persists the same pending children");
    assert!(!manager.fatal_local_authority_fence.is_closed());
}

#[tokio::test]
async fn native_malformed_current_payload_is_known_non_resumable_failure() {
    let (_, old_runtime, old_request) = pending_native_spawn().await;
    let old_invocation = old_request.invocation.clone();
    drop(old_request);
    drop(old_runtime);
    let (manager, mut runtime, mut request) = pending_native_spawn().await;
    assert_ne!(old_invocation, request.invocation);
    let old_receipt = NativeSpawnObservation::Admission(Ok(SubAgentAdmissionReceipt {
        invocation: old_invocation,
        outcome: SubAgentAdmissionResponse::Admitted,
    }));
    assert!(runtime
        .native_spawn_observed(old_receipt)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        runtime.state,
        ConvState::Error {
            error_kind: crate::db::ErrorKind::SubAgentError,
            ..
        }
    ));
    assert!(runtime.native_spawn_handoff.is_none());
    assert!(matches!(
        request.activation_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(!manager.fatal_local_authority_fence.is_closed());
}

#[tokio::test]
async fn native_abnormal_replaced_origin_withholds_activation_without_fencing() {
    let (manager, mut runtime, request) = pending_native_spawn().await;
    let manager_task = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.handle_spawn_request(request).await }
    });
    let response = runtime.observe_native_spawn().await;
    if let ConvState::ToolExecuting {
        assistant_message, ..
    } = &mut runtime.state
    {
        assistant_message.message_id = "successor-assistant".into();
    }
    let successor = runtime.state.clone();
    assert!(runtime
        .native_spawn_observed(response)
        .await
        .unwrap()
        .is_none());
    manager_task.await.unwrap();
    assert_eq!(runtime.state, successor);
    assert!(!manager.fatal_local_authority_fence.is_closed());
    let claims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE initial_dispatch_claimed_at_unix_micros IS NOT NULL OR parent_accepted_at_unix_micros IS NOT NULL").fetch_one(manager.db().pool()).await.unwrap();
    assert_eq!(claims, 0);
}
