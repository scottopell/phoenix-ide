use super::super::{tool_generation_guard_tests::runtime_tool_executing, RuntimeExitDisposition};
use super::*;
use crate::runtime::{
    testing::{InMemoryStorage, MockLlmClient, MockToolExecutor},
    traits::ActiveDirectTurn,
};
use crate::state_machine::{state::ToolInput, Effect};
use phoenix_core::domain::tool_availability::ToolAvailability;
use phoenix_llm::ContentBlock;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

type TestRuntime =
    ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>;
type ReadGate = (oneshot::Receiver<()>, oneshot::Sender<Result<(), String>>);

async fn runtime_for(tool: ToolCall) -> TestRuntime {
    let mut runtime = runtime_tool_executing("before");
    let executor = Arc::new(
        MockToolExecutor::new().with_tool(tool.name(), crate::tools::ToolOutput::success("unused")),
    );
    runtime.storage.seed_tool_admission_policy(
        "conv-tool-gen",
        ToolAvailability::all(executor.definitions().await),
    );
    runtime.tool_executor = Arc::new(executor);
    if let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        assistant_message,
        ..
    } = &mut runtime.state
    {
        *remaining_tools = vec![tool.clone()];
        assistant_message.content = [current_tool, &tool]
            .into_iter()
            .map(|call| ContentBlock::tool_use(&call.id, call.name(), call.input.to_value()))
            .collect();
    }
    runtime
}

fn selected(runtime: &mut TestRuntime, assistant: &str, tool: ToolCall) {
    let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        assistant_message,
        ..
    } = &mut runtime.state
    else {
        panic!("executing fixture")
    };
    *current_tool = tool.clone();
    remaining_tools.clear();
    assistant_message.message_id = assistant.into();
    assistant_message.content = vec![ContentBlock::tool_use(
        &tool.id,
        tool.name(),
        tool.input.to_value(),
    )];
}

fn bash(id: &str, command: &str) -> ToolCall {
    ToolCall::new(
        id,
        ToolInput::from(crate::tools::BashToolInput::run(command)),
    )
}

async fn cancellation_while_read_held(mut runtime: TestRuntime, (entered, release): ReadGate) {
    let executor = Arc::clone(&runtime.tool_executor);
    let storage = Arc::clone(&runtime.storage);
    let llm = Arc::clone(&runtime.llm_client);
    let (events, receiver) = mpsc::channel(8);
    runtime.event_tx = events.clone();
    runtime.event_rx = receiver;
    let (state_tx, mut states) = watch::channel(runtime.state.clone());
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    events
        .send(Event::ToolComplete {
            tool_use_id: "before".into(),
            result: ToolResult::success("before".into(), "done".into()),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    events
        .send(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    let revoked = release.send(Ok(())).is_err();
    tokio::time::resume();
    events.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        task.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    assert!(cancelled, "read must not queue cancellation beyond 100ms");
    assert!(
        revoked,
        "the cancelled read continuation must be dropped before cancellation completes"
    );
    assert!(matches!(
        storage.get_current_state("conv-tool-gen"),
        Some(ConvState::Idle)
    ));
    assert!(executor.recorded_executions().is_empty());
    assert!(llm.recorded_requests().is_empty());
}

#[tokio::test]
async fn work_scope_preparation_is_cancellable_in_the_owned_loop() {
    let mut runtime = runtime_for(bash("scope", "git status --short")).await;
    runtime.context.resource_scope = phoenix_core::work_scope::ResourceScopeKey::Work(
        phoenix_core::work_scope::WorkScopeId::new(),
    );
    let gate = runtime.storage.hold_next_tool_scope_read();
    cancellation_while_read_held(runtime, gate).await;
}

#[tokio::test]
async fn native_catalog_preparation_is_cancellable_before_any_native_owner() {
    let call = ToolCall::new(
        "catalog",
        ToolInput::from_name_and_value(
            "spawn_agents",
            serde_json::json!({"tasks": [{"task": "read"}]}),
        ),
    );
    let runtime = runtime_for(call).await;
    let gate = runtime.tool_executor.hold_next_definitions();
    cancellation_while_read_held(runtime, gate).await;
}

#[tokio::test]
async fn typed_self_goal_preparation_is_cancellable_before_checkpoint() {
    let call = ToolCall::new(
        "goal",
        ToolInput::from_name_and_value("create_goal", serde_json::json!({"objective": "finish"})),
    );
    let mut runtime = runtime_for(call).await;
    runtime.active_direct_turn = Some(Box::new(ActiveDirectTurn {
        turn_id: phoenix_workflow::TurnAuthorityId(1),
        generation: 1,
    }));
    let gate = runtime.storage.hold_next_self_goal_read();
    cancellation_while_read_held(runtime, gate).await;
}

#[tokio::test]
async fn stale_clear_deny_and_read_error_cannot_touch_same_id_successor() {
    for disposition in ["clear", "deny", "error"] {
        let command = if disposition == "deny" {
            "git add -A"
        } else {
            "git status --short"
        };
        let original = bash("reused", command);
        let mut runtime = runtime_for(original.clone()).await;
        selected(&mut runtime, "original", original.clone());
        let (entered, release) = runtime.storage.hold_next_tool_policy_read();
        runtime
            .execute_effect(Effect::ExecuteTool { tool: original })
            .await
            .unwrap();
        let mut pending = runtime.tool_preparation.take().unwrap();
        let observation = async {
            let observation = pending.observe();
            let unblock = async {
                entered.await.unwrap();
                release
                    .send(if disposition == "error" {
                        Err("policy read failed".into())
                    } else {
                        Ok(())
                    })
                    .unwrap();
            };
            tokio::join!(observation, unblock).0
        }
        .await;
        let successor = bash("reused", "git diff --stat");
        selected(&mut runtime, "successor", successor.clone());
        runtime
            .execute_effect(Effect::ExecuteTool { tool: successor })
            .await
            .unwrap();
        let state = runtime.state.clone();
        let generation = runtime.tool_execution.generation();
        assert!(
            runtime
                .accept_tool_preparation(observation)
                .unwrap()
                .is_none(),
            "{disposition}"
        );
        assert_eq!(runtime.state, state);
        assert!(runtime.tool_preparation.is_some());
        assert_eq!(runtime.tool_execution.generation(), generation);
        assert!(runtime.tool_executor.recorded_executions().is_empty());
        assert!(runtime.storage.recorded_messages().is_empty());
        runtime.abort_external_effects_in_memory();
    }
}

#[tokio::test]
async fn changed_origin_cannot_adopt_its_own_ready_clear_or_denial() {
    for command in ["git status --short", "git add -A"] {
        let call = bash("reused", command);
        let mut runtime = runtime_for(call.clone()).await;
        selected(&mut runtime, "original", call.clone());
        runtime
            .execute_effect(Effect::ExecuteTool { tool: call.clone() })
            .await
            .unwrap();
        let observation = runtime.tool_preparation.as_mut().unwrap().observe().await;
        selected(&mut runtime, "successor", call);
        assert!(runtime
            .accept_tool_preparation(observation)
            .unwrap()
            .is_none());
        assert_eq!(runtime.tool_execution.generation(), 0);
        assert!(runtime.tool_executor.recorded_executions().is_empty());
        assert!(runtime.tool_preparation.is_none());
    }
}

#[tokio::test]
async fn genuine_shared_authority_loss_precedes_stale_read_disposition() {
    let call = bash("reused", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", call.clone());
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call.clone() })
        .await
        .unwrap();
    let observation = runtime.tool_preparation.as_mut().unwrap().observe().await;
    selected(&mut runtime, "successor", call);
    runtime
        .fatal_local_authority_fence
        .close("test_unknown_local_write");
    assert_eq!(
        runtime.accept_tool_preparation(observation).unwrap_err(),
        "runtime processing closed after fatal local authority loss"
    );
    assert!(runtime
        .fatal_local_authority_fence
        .fatal_authority_loss_is_latched());
    assert_eq!(runtime.tool_execution.generation(), 0);
    assert!(runtime.tool_executor.recorded_executions().is_empty());
}

#[tokio::test]
async fn ordinary_read_error_text_is_not_sqlite_authority_and_keeps_positive_cancel_proof() {
    let call = bash("read-error", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", call.clone());
    let (entered, release) = runtime.storage.hold_next_tool_policy_read();
    let diagnostic = "FATAL_LOCAL_AUTHORITY_UNCLASSIFIED: ordinary read diagnostic";
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call })
        .await
        .unwrap();
    let observation = runtime.tool_preparation.as_mut().unwrap().observe();
    let unblock = async {
        entered.await.unwrap();
        release.send(Err(diagnostic.into())).unwrap();
    };
    let observation = tokio::join!(observation, unblock).0;
    assert_eq!(
        runtime.accept_tool_preparation(observation).unwrap_err(),
        diagnostic
    );
    assert!(!runtime.fatal_local_authority_fence.is_closed());
    assert!(runtime.tool_preparation.as_ref().unwrap().read.is_none());
    runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert_eq!(runtime.tool_execution.generation(), 0);
    assert!(runtime.tool_executor.recorded_executions().is_empty());
}

#[tokio::test]
async fn rejected_cancel_checkpoint_revokes_read_without_false_completion_or_late_launch() {
    let call = bash("cancel-persist", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", call.clone());
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call })
        .await
        .unwrap();
    runtime.storage.set_fail_state_update(true);
    let error = runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap_err();
    assert!(error.contains("injected state update failure"));
    assert!(matches!(runtime.state, ConvState::ToolExecuting { .. }));
    assert!(runtime.tool_preparation.as_ref().unwrap().read.is_none());
    assert!(runtime.storage.recorded_messages().is_empty());
    assert!(!runtime.fatal_local_authority_fence.is_closed());
    assert_eq!(runtime.tool_execution.generation(), 0);
    runtime.storage.set_fail_state_update(false);
    runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(runtime.tool_executor.recorded_executions().is_empty());
    assert!(runtime.llm_client.recorded_requests().is_empty());
}
