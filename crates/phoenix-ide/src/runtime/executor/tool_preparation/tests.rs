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

fn selected(runtime: &mut TestRuntime, assistant: &str, tool: &ToolCall) {
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
    let active_turn = runtime.active_direct_turn.as_deref().cloned();
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
    if let Some(turn) = active_turn {
        let obligations = storage.recorded_terminal_obligations();
        let terminal = obligations
            .first()
            .expect("terminal cancellation fact must be acknowledged");
        assert!(
            obligations.iter().all(|receipt| receipt == terminal),
            "each idempotent acknowledgement must describe the same exact terminal fact"
        );
        assert_eq!(obligations[0].turn_id, turn.turn_id);
        assert_eq!(obligations[0].expected_generation, turn.generation);
        assert!(matches!(obligations[0].projection.state, ConvState::Idle));
        assert_eq!(
            obligations[0].terminal,
            phoenix_workflow::TurnTerminal::Cancelled
        );
    } else {
        assert!(matches!(
            storage.get_current_state("conv-tool-gen"),
            Some(ConvState::Idle)
        ));
    }
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

#[derive(Clone, Copy)]
enum RawProviderRead {
    Policy,
    Scope,
    NativeCatalog,
}

async fn raw_provider_read_cancellation(read: RawProviderRead) {
    let (name, input) = match read {
        RawProviderRead::Policy | RawProviderRead::Scope => {
            ("bash", serde_json::json!({"op": "run", "cmd": "pwd"}))
        }
        RawProviderRead::NativeCatalog => (
            "spawn_agents",
            serde_json::json!({"tasks": [{"task": "read"}]}),
        ),
    };
    let response = phoenix_llm::LlmResponse {
        provider_replay: None,
        content: vec![ContentBlock::tool_use(
            "raw-provider-call",
            name,
            input.clone(),
        )],
        end_turn: false,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    };
    let calls: Vec<_> = response
        .tool_uses()
        .into_iter()
        .map(|(id, name, input)| {
            ToolCall::new(id, ToolInput::from_name_and_value(name, input.clone()))
        })
        .collect();
    assert_eq!(calls.len(), 1);
    let call = calls.into_iter().next().unwrap();
    assert_ne!(call.input.to_value(), input);
    let mut runtime = runtime_for(call.clone()).await;
    let ConvState::ToolExecuting {
        assistant_message,
        remaining_tools,
        ..
    } = &mut runtime.state
    else {
        unreachable!()
    };
    assert_eq!(remaining_tools, &[call]);
    let selected = assistant_message
        .content
        .iter_mut()
        .find(|block| {
            matches!(
                block, ContentBlock::ToolUse { id, .. } if id == "raw-provider-call"
            )
        })
        .unwrap();
    *selected = response.content.into_iter().next().unwrap();
    if matches!(read, RawProviderRead::Scope) {
        runtime.context.resource_scope = phoenix_core::work_scope::ResourceScopeKey::Work(
            phoenix_core::work_scope::WorkScopeId::new(),
        );
    }
    let storage = Arc::clone(&runtime.storage);
    let gate = match read {
        RawProviderRead::Policy => storage.hold_next_tool_policy_read(),
        RawProviderRead::Scope => storage.hold_next_tool_scope_read(),
        RawProviderRead::NativeCatalog => runtime.tool_executor.hold_next_definitions(),
    };
    cancellation_while_read_held(runtime, gate).await;
    assert!(!storage.recorded_messages().iter().any(|message| matches!(
        &message.content,
        phoenix_core::domain::db_schema::MessageContent::System(content)
            if content.text.starts_with("Cancellation completed.")
    )));
}

#[tokio::test]
async fn raw_provider_bash_policy_read_cancels_without_canonicalizing_origin() {
    raw_provider_read_cancellation(RawProviderRead::Policy).await;
}

#[tokio::test]
async fn raw_provider_bash_scope_read_cancels_without_canonicalizing_origin() {
    raw_provider_read_cancellation(RawProviderRead::Scope).await;
}

#[tokio::test]
async fn raw_provider_spawn_catalog_read_cancels_without_canonicalizing_origin() {
    raw_provider_read_cancellation(RawProviderRead::NativeCatalog).await;
}

#[tokio::test]
async fn same_assistant_and_call_ids_cannot_adopt_changed_source() {
    let call = bash("reused", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "same-assistant", &call);
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call })
        .await
        .unwrap();
    let observation = runtime
        .serial_dispatch_mut()
        .preparation
        .as_mut()
        .unwrap()
        .observe()
        .await;
    let ConvState::ToolExecuting {
        assistant_message, ..
    } = &mut runtime.state
    else {
        unreachable!()
    };
    assistant_message.content = vec![ContentBlock::tool_use(
        "reused",
        "bash",
        serde_json::json!({"op": "run", "cmd": "git status --short"}),
    )];
    assert!(runtime
        .accept_tool_preparation(observation)
        .unwrap()
        .is_none());
    assert_eq!(runtime.serial_dispatch().execution.generation(), 0);
    assert!(runtime.tool_executor.recorded_executions().is_empty());
    assert!(runtime.serial_dispatch_mut().preparation.is_none());
    runtime.abort_external_effects_in_memory();
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
        selected(&mut runtime, "original", &original);
        let (entered, release) = runtime.storage.hold_next_tool_policy_read();
        if disposition == "error" {
            let owner = runtime.fatal_local_authority_fence.try_acquire().unwrap();
            runtime.required_settlement_authority =
                Some(super::super::RequiredSettlementScope::new(
                    owner,
                    super::super::RequiredSettlementBoundary::Tool,
                ));
        }
        runtime
            .execute_effect(Effect::ExecuteTool { tool: original })
            .await
            .unwrap();
        runtime.required_settlement_authority = None;
        let mut pending = runtime.serial_dispatch_mut().preparation.take().unwrap();
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
        selected(&mut runtime, "successor", &successor);
        runtime
            .execute_effect(Effect::ExecuteTool { tool: successor })
            .await
            .unwrap();
        let state = runtime.state.clone();
        let generation = runtime.serial_dispatch().execution.generation();
        assert!(
            runtime
                .accept_tool_preparation(observation)
                .unwrap()
                .is_none(),
            "{disposition}"
        );
        assert_eq!(runtime.state, state);
        assert!(runtime.serial_dispatch_mut().preparation.is_some());
        assert_eq!(runtime.serial_dispatch().execution.generation(), generation);
        assert!(runtime.tool_executor.recorded_executions().is_empty());
        assert!(runtime.storage.recorded_messages().is_empty());
        assert!(!runtime.fatal_local_authority_fence.is_closed());
        runtime.abort_external_effects_in_memory();
    }
}

#[tokio::test]
async fn changed_origin_cannot_adopt_its_own_ready_clear_or_denial() {
    for command in ["git status --short", "git add -A"] {
        let call = bash("reused", command);
        let mut runtime = runtime_for(call.clone()).await;
        selected(&mut runtime, "original", &call);
        runtime
            .execute_effect(Effect::ExecuteTool { tool: call.clone() })
            .await
            .unwrap();
        let observation = runtime
            .serial_dispatch_mut()
            .preparation
            .as_mut()
            .unwrap()
            .observe()
            .await;
        selected(&mut runtime, "successor", &call);
        assert!(runtime
            .accept_tool_preparation(observation)
            .unwrap()
            .is_none());
        assert_eq!(runtime.serial_dispatch().execution.generation(), 0);
        assert!(runtime.tool_executor.recorded_executions().is_empty());
        assert!(runtime.serial_dispatch_mut().preparation.is_none());
    }
}

#[tokio::test]
async fn genuine_shared_authority_loss_precedes_stale_read_disposition() {
    let call = bash("reused", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", &call);
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call.clone() })
        .await
        .unwrap();
    let observation = runtime
        .serial_dispatch_mut()
        .preparation
        .as_mut()
        .unwrap()
        .observe()
        .await;
    selected(&mut runtime, "successor", &call);
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
    assert_eq!(runtime.serial_dispatch().execution.generation(), 0);
    assert!(runtime.tool_executor.recorded_executions().is_empty());
}

#[tokio::test]
async fn ordinary_read_error_text_is_not_sqlite_authority_and_keeps_positive_cancel_proof() {
    let call = bash("read-error", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", &call);
    let (entered, release) = runtime.storage.hold_next_tool_policy_read();
    let diagnostic = "FATAL_LOCAL_AUTHORITY_UNCLASSIFIED: ordinary read diagnostic";
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call })
        .await
        .unwrap();
    let observation = runtime
        .serial_dispatch_mut()
        .preparation
        .as_mut()
        .unwrap()
        .observe();
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
    assert!(matches!(
        runtime
            .serial_dispatch_mut()
            .preparation
            .as_ref()
            .unwrap()
            .phase,
        PreparationPhase::NoRead
    ));
    runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    let interrupted = runtime
        .observe_tool_preparation_for_test()
        .await
        .unwrap()
        .expect("positive no-effects cancellation");
    assert!(matches!(interrupted, Event::ToolAborted { .. }));
    runtime.process_event(interrupted).await.unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert_eq!(runtime.serial_dispatch().execution.generation(), 0);
    assert!(runtime.tool_executor.recorded_executions().is_empty());
}

#[tokio::test]
async fn rejected_cancel_checkpoint_revokes_read_without_false_completion_or_late_launch() {
    let call = bash("cancel-persist", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", &call);
    let (entered, release) = runtime.storage.hold_next_tool_policy_read();
    runtime
        .execute_effect(Effect::ExecuteTool { tool: call })
        .await
        .unwrap();
    let mut pending = runtime.serial_dispatch_mut().preparation.take().unwrap();
    tokio::select! {
        _ = pending.observe() => panic!("read must remain held"),
        entered = entered => entered.unwrap(),
    }
    runtime.serial_dispatch_mut().preparation = Some(pending);
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
    assert!(
        release.send(Ok(())).is_err(),
        "read was revoked despite the rejected checkpoint"
    );
    assert!(
        runtime
            .observe_tool_preparation_for_test()
            .await
            .unwrap()
            .is_none(),
        "failed cancellation checkpoint cannot publish ToolAborted"
    );
    assert!(matches!(
        runtime
            .serial_dispatch_mut()
            .preparation
            .as_ref()
            .unwrap()
            .phase,
        PreparationPhase::NoRead
    ));
    assert!(runtime.storage.recorded_messages().is_empty());
    assert!(!runtime.fatal_local_authority_fence.is_closed());
    assert_eq!(runtime.serial_dispatch().execution.generation(), 0);
    runtime.storage.set_fail_state_update(false);
    runtime
        .process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    let interrupted = runtime
        .observe_tool_preparation_for_test()
        .await
        .unwrap()
        .expect("retry retains exact no-effects proof");
    assert!(matches!(interrupted, Event::ToolAborted { .. }));
    runtime.process_event(interrupted).await.unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(runtime.tool_executor.recorded_executions().is_empty());
    assert!(runtime.llm_client.recorded_requests().is_empty());
}

#[tokio::test]
async fn ordinary_live_catalog_wait_cannot_launch_after_cancellation() {
    let call = bash("live-catalog", "git status --short");
    let mut runtime = runtime_for(call.clone()).await;
    selected(&mut runtime, "original", &call);
    let tools = Arc::clone(&runtime.tool_executor);
    let (entered, release) = tools.hold_next_definitions();
    assert!(runtime
        .dispatch_tool_for_test(call)
        .await
        .unwrap()
        .is_none());
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .unwrap()
        .unwrap();
    let (events, receiver) = mpsc::channel(4);
    runtime.event_tx = events.clone();
    runtime.event_rx = receiver;
    let (state_tx, mut states) = watch::channel(runtime.state.clone());
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    tokio::time::pause();
    events
        .send(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .unwrap();
    let cancelled_before_release = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    let read_revoked = release.send(Ok(())).is_err();
    tokio::time::resume();
    tokio::time::timeout(
        Duration::from_secs(60),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .unwrap()
    .unwrap();
    events.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        task.await.unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    assert_eq!(
        (
            cancelled_before_release,
            read_revoked,
            tools.recorded_executions().len()
        ),
        (true, true, 0),
        "cancellation must settle before the read releases and prevent any late tool execution"
    );
}

#[derive(Clone, Copy, Debug)]
enum RequiredRead {
    Policy,
    Scope,
}

async fn required_read_failure_stops_the_owning_actor(read: RequiredRead) {
    let failing = bash("failing-read", "git status --short");
    let mut runtime = runtime_for(failing).await;
    let never = bash("never-launched", "git diff --stat");
    if let ConvState::ToolExecuting {
        remaining_tools,
        assistant_message,
        ..
    } = &mut runtime.state
    {
        remaining_tools.push(never.clone());
        assistant_message.content.push(ContentBlock::tool_use(
            &never.id,
            never.name(),
            never.input.to_value(),
        ));
    }
    let gate = match read {
        RequiredRead::Policy => runtime.storage.hold_next_tool_policy_read(),
        RequiredRead::Scope => {
            runtime.context.resource_scope = phoenix_core::work_scope::ResourceScopeKey::Work(
                phoenix_core::work_scope::WorkScopeId::new(),
            );
            runtime.storage.hold_next_tool_scope_read()
        }
    };
    let (entered, release) = gate;
    let fence = Arc::clone(&runtime.fatal_local_authority_fence);
    let tools = Arc::clone(&runtime.tool_executor);
    let storage = Arc::clone(&runtime.storage);
    let llm = Arc::clone(&runtime.llm_client);
    let (events, receiver) = mpsc::channel(4);
    runtime.event_tx = events.clone();
    runtime.event_rx = receiver;
    let mut task = tokio::spawn(runtime.run());
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
    release
        .send(Err(format!("{read:?} lookup failed")))
        .unwrap();
    let stopped = tokio::time::timeout(Duration::from_millis(100), &mut task)
        .await
        .ok()
        .map(Result::unwrap);
    tokio::time::resume();
    if stopped.is_none() {
        events.send(Event::Shutdown).await.unwrap();
        assert_eq!(
            task.await.unwrap(),
            RuntimeExitDisposition::CoordinatedShutdown
        );
    }
    assert_eq!(
        stopped,
        Some(RuntimeExitDisposition::FatalLocalAuthorityLoss)
    );
    assert!(fence.fatal_authority_loss_is_latched());
    assert_eq!(*fence.tx.borrow(), Some("ordinary_tool_outcome_settlement"));
    assert!(
        matches!(storage.get_current_state("conv-tool-gen"), Some(ConvState::ToolExecuting {
        current_tool, remaining_tools, completed_results, ..
    }) if current_tool.id == "failing-read"
        && remaining_tools.len() == 1 && remaining_tools[0].id == "never-launched"
        && completed_results.len() == 1
        && matches!(&completed_results[0], phoenix_core::domain::conversation_goal::ToolRoundResult::Finalized(result) if result.tool_use_id == "before" && !result.is_error()))
    );
    assert!(tools.recorded_executions().is_empty());
    assert!(llm.recorded_requests().is_empty());
    assert!(storage.recorded_messages().is_empty());
}

#[tokio::test]
async fn later_policy_read_failure_preserves_required_completion_fail_stop() {
    required_read_failure_stops_the_owning_actor(RequiredRead::Policy).await;
}

#[tokio::test]
async fn later_scope_read_failure_preserves_required_completion_fail_stop() {
    required_read_failure_stops_the_owning_actor(RequiredRead::Scope).await;
}
