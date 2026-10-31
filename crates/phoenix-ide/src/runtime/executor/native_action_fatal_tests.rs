#![allow(clippy::large_futures)]

use super::*;
use crate::runtime::deny_gate::CheckedToolCall;
use crate::runtime::testing::MockLlmClient;
use crate::runtime::traits::{DatabaseStorage, ToolRegistryExecutor};
use crate::state_machine::AssistantMessage;
use phoenix_core::domain::tool_input::ToolInput;
use phoenix_llm::ToolDefinition;
use phoenix_tools::{ToolOutput, ToolRegistry};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use tokio::sync::{broadcast, Notify};

struct ObserveNativeFatalSink {
    inner: ToolRegistryExecutor,
    fence: Arc<FatalLocalAuthorityFence>,
    pending: Arc<Notify>,
    dropped: Arc<Notify>,
    ordinary_outputs: AtomicUsize,
}

struct NotifyAdapterDrop(Arc<Notify>);

impl Drop for NotifyAdapterDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[async_trait::async_trait]
impl ToolExecutor for ObserveNativeFatalSink {
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput> {
        let _drop = NotifyAdapterDrop(self.dropped.clone());
        self.fence.close("native_adapter_fatal_sink_regression");
        let mut native = Box::pin(self.inner.execute(call, ctx));
        std::future::poll_fn(|cx| match native.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("closed native adapter returned an ordinary outcome"),
        })
        .await;
        self.pending.notify_one();
        let output = native.await;
        self.ordinary_outputs.fetch_add(1, Ordering::SeqCst);
        output
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions().await
    }
}

#[allow(clippy::too_many_lines)]
async fn assert_native_fatal_sink_is_disposed_without_result(name: &str, input: serde_json::Value) {
    let (manager, db, source) = crate::conversation_action_tools::tests::native_runtime().await;
    let registry = ToolRegistry::coordinator(
        crate::conversation_action_tools::tools(manager.clone(), &source)
            .await
            .unwrap(),
        None,
    );
    assert!(registry.tool_definitions().any(|tool| tool.name == name));
    let executor = Arc::new(ObserveNativeFatalSink {
        inner: ToolRegistryExecutor::new(registry),
        fence: manager.fatal_local_authority_fence.clone(),
        pending: Arc::new(Notify::new()),
        dropped: Arc::new(Notify::new()),
        ordinary_outputs: AtomicUsize::new(0),
    });
    let tool = ToolCall::new(
        "native-fatal-call",
        ToolInput::Unknown {
            name: name.into(),
            input,
        },
    );
    let assistant = AssistantMessage::new(
        "native-fatal-assistant".into(),
        vec![ContentBlock::ToolUse {
            id: tool.id.clone(),
            name: name.into(),
            input: tool.input.to_value(),
        }],
        None,
        None,
    );
    let expected_tool_use_id = tool.id.clone();
    let expected_assistant_message_id = assistant.message_id.clone();
    let state = ConvState::ToolExecuting {
        current_tool: tool.clone(),
        remaining_tools: Vec::new(),
        completed_results: Vec::new(),
        pending_sub_agents: Vec::new(),
        assistant_message: assistant,
    };
    db.update_conversation_state(&source, &state).await.unwrap();
    let definitions = executor.definitions().await;
    let callable_names = definitions.iter().map(|tool| tool.name.clone()).collect();
    db.prepare_tool_availability(
        &source,
        "native-fatal-test",
        None,
        &definitions,
        &callable_names,
        &[],
        &[],
    )
    .await
    .unwrap();
    let before_messages = db.get_messages(&source).await.unwrap();
    let before_state = serde_json::to_value(&state).unwrap();
    let before_updated_at = db.get_conversation(&source).await.unwrap().state_updated_at;
    let storage = Arc::new(DatabaseStorage::new(db.clone()));
    let llm = Arc::new(MockLlmClient::new("test-model"));
    let broadcaster = SseBroadcaster::new(32, 0)
        .with_fatal_local_authority_fence(manager.fatal_local_authority_fence.clone());
    let mut events = broadcaster.subscribe();
    let (event_tx, event_rx) = mpsc::channel(2);
    let mut runtime = ConversationRuntime::new(
        ConvContext::coordinator(&source, "test-model", 100_000, 8_192),
        state,
        storage,
        llm.clone(),
        executor.clone(),
        manager.browser_sessions.clone(),
        manager.bash_handles.clone(),
        manager.tmux_registry.clone(),
        manager.llm_registry.clone(),
        manager.terminals.clone(),
        event_rx,
        event_tx,
        broadcaster,
    )
    .with_fatal_local_authority_fence(manager.fatal_local_authority_fence.clone());
    assert!(runtime
        .execute_effect(Effect::ExecuteTool { tool })
        .await
        .unwrap()
        .is_none());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        executor.pending.notified().await;
        let Ok(SseEvent::MessageUpdated {
            opening: None,
            sequence_id: _,
            message_id,
            transcript_generation: None,
            display_data: Some(display_data),
            content: None,
            duration_ms: None,
        }) = events.try_recv()
        else {
            panic!("expected the single pre-close tool-start metadata event");
        };
        assert_eq!(message_id, expected_assistant_message_id);
        let started_at = display_data["tool_starts"][expected_tool_use_id.as_str()]
            .as_i64()
            .expect("tool-start metadata must contain an integer timestamp");
        assert!(started_at > 0);
        assert_eq!(
            display_data,
            serde_json::json!({
                "tool_starts": { (expected_tool_use_id.as_str()): started_at },
            }),
        );
        assert_eq!(
            runtime.run().await,
            RuntimeExitDisposition::FatalLocalAuthorityLoss,
        );
        executor.dropped.notified().await;
        manager.fatal_local_authority_fence.wait_for_owners().await;
    })
    .await
    .expect("native fatal sink must be observed and disposed by the executor");
    assert_eq!(executor.ordinary_outputs.load(Ordering::SeqCst), 0);
    assert!(llm.recorded_requests().is_empty());
    assert_eq!(
        serde_json::to_value(db.get_messages(&source).await.unwrap()).unwrap(),
        serde_json::to_value(before_messages).unwrap(),
    );
    let after = db.get_conversation(&source).await.unwrap();
    assert_eq!(serde_json::to_value(after.state).unwrap(), before_state);
    assert_eq!(after.state_updated_at, before_updated_at);
    loop {
        match events.try_recv() {
            Ok(event) => assert!(!matches!(
                event,
                SseEvent::Message { .. } | SseEvent::MessageUpdated { .. }
            )),
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break;
            }
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                panic!("SSE observation lost {skipped} events");
            }
        }
    }
}

#[tokio::test]
async fn native_continue_fatal_sink_exits_executor_without_persisting_a_result() {
    assert_native_fatal_sink_is_disposed_without_result(
        "continue_conversation",
        serde_json::json!({ "product_conversation_id": "unreached-target" }),
    )
    .await;
}

#[tokio::test]
async fn native_settings_read_fatal_sink_exits_executor_without_persisting_a_result() {
    assert_native_fatal_sink_is_disposed_without_result(
        "read_automatic_continuation",
        serde_json::json!({ "target": { "kind": "coordinator" } }),
    )
    .await;
}

#[tokio::test]
async fn native_settings_set_fatal_sink_exits_executor_without_persisting_a_result() {
    assert_native_fatal_sink_is_disposed_without_result(
        "set_automatic_continuation",
        serde_json::json!({
            "target": { "kind": "coordinator" },
            "auto_continue_on_context_exhaustion": true,
        }),
    )
    .await;
}
