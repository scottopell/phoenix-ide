#![allow(clippy::large_futures)]

use super::*;
use crate::runtime::deny_gate::CheckedToolCall;
use crate::runtime::testing::MockLlmClient;
use crate::runtime::traits::{DatabaseStorage, ToolRegistryExecutor};
use crate::runtime::FatalLocalAuthorityFence;
use crate::state_machine::AssistantMessage;
use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion;
use phoenix_core::domain::sm_state::ToolInput;
use phoenix_llm::ToolDefinition;
use phoenix_tools::{ToolOutput, ToolRegistry};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use tokio::sync::{broadcast, Notify};

struct ObserveUncertainSettingsAdapter {
    inner: ToolRegistryExecutor,
    fence: Arc<FatalLocalAuthorityFence>,
    pending: Arc<Notify>,
    dropped: Arc<Notify>,
    ordinary_outputs: AtomicUsize,
}

struct SettingsAdapterDrop(Arc<Notify>);

impl Drop for SettingsAdapterDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[async_trait::async_trait]
impl ToolExecutor for ObserveUncertainSettingsAdapter {
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput> {
        let _drop = SettingsAdapterDrop(self.dropped.clone());
        assert!(!self.fence.is_closed());
        let mut native = Box::pin(self.inner.execute(call, ctx));
        std::future::poll_fn(|cx| match native.as_mut().poll(cx) {
            Poll::Ready(output) => {
                self.ordinary_outputs.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(output)
            }
            Poll::Pending => {
                self.pending.notify_one();
                Poll::Pending
            }
        })
        .await
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        self.inner.definitions().await
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn uncertain_registered_setting_closes_authority_and_stops_https_supervisor() {
    let (manager, db, source) = crate::conversation_action_tools::tests::native_runtime().await;
    let source_aggregate = db
        .get_conversation(&source)
        .await
        .unwrap()
        .product_conversation_id;
    assert!(!manager.local_authority_is_closed());
    assert_eq!(manager.fatal_local_authority_fence.owner_count(), 0);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&source_aggregate)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled,
    );
    let registry = ToolRegistry::coordinator(
        crate::conversation_action_tools::tools(manager.clone(), &source)
            .await
            .unwrap(),
        None,
    );
    let name = "set_automatic_continuation";
    assert!(registry.definitions().iter().any(|tool| tool.name == name));
    let executor = Arc::new(ObserveUncertainSettingsAdapter {
        inner: ToolRegistryExecutor::builtin_only(registry, Arc::from([])),
        fence: manager.fatal_local_authority_fence.clone(),
        pending: Arc::new(Notify::new()),
        dropped: Arc::new(Notify::new()),
        ordinary_outputs: AtomicUsize::new(0),
    });
    let input = serde_json::json!({
        "target": { "kind": "coordinator" },
        "auto_continue_on_context_exhaustion": true,
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
    let (acknowledged_event_tx, _acknowledged_event_rx) = mpsc::channel(2);
    let (_state_tx, state_rx) = tokio::sync::watch::channel(state.clone());
    manager.runtimes.write().await.insert(
        source.clone(),
        crate::runtime::ConversationHandle {
            event_tx: event_tx.clone(),
            acknowledged_event_tx,
            handoff_review_tx: None,
            turn_trigger: Arc::new(std::sync::Mutex::new(None)),
            broadcast_tx: broadcaster.clone(),
            identity: Arc::new(()),
            state_rx,
            startup_llm_recovery_ack: Arc::new(tokio::sync::Mutex::new(None)),
        },
    );
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
    let (committed, release_ack) = db.pause_next_native_settings_commit_ack_for_test();
    let supervisor_barrier = Arc::new(tokio::sync::Barrier::new(2));
    *manager.fatal_runtime_map_barrier.lock().await = Some(supervisor_barrier.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tls_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(rustls::server::ResolvesServerCertUsingSni::new()));
    let supervisor = crate::tls::serve_https(
        listener,
        axum::Router::new(),
        tls_config,
        false,
        manager.fatal_local_authority_receiver(),
        &manager,
        &manager.bash_handles,
    );
    assert!(runtime
        .execute_effect(Effect::ExecuteTool { tool })
        .await
        .unwrap()
        .is_none());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let drive = async {
            committed.await.expect("registered setter never committed");
            assert!(!manager.local_authority_is_closed());
            executor.pending.notified().await;
            assert_eq!(manager.fatal_local_authority_fence.owner_count(), 4);
            assert_eq!(
                db.auto_continue_on_context_exhaustion(&source_aggregate)
                    .await
                    .unwrap(),
                AutoContinueOnContextExhaustion::Enabled,
            );
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
            release_ack
                .send(())
                .expect("owned setter abandoned DB acknowledgement");
            executor.dropped.notified().await;
            assert!(manager.local_authority_is_closed());
            assert_eq!(
                *manager.fatal_local_authority_receiver().borrow(),
                Some("native_automatic_continuation_setting_unclassified"),
            );
            assert_eq!(
                manager.fatal_local_authority_fence.owners_at_first_close(),
                Some(4),
            );
            manager.fatal_local_authority_fence.wait_for_owners().await;
            assert_eq!(manager.fatal_local_authority_fence.owner_count(), 0);
            supervisor_barrier.wait().await;
            assert!(manager.runtimes.read().await.contains_key(&source));
            assert_eq!(
                runtime.run().await,
                RuntimeExitDisposition::FatalLocalAuthorityLoss,
            );
            assert_eq!(executor.ordinary_outputs.load(Ordering::SeqCst), 0);
            assert!(llm.recorded_requests().is_empty());
            supervisor_barrier.wait().await;
        };
        let (result, ()) = tokio::join!(biased; supervisor, drive);
        assert!(result.is_err_and(|error| error.is::<crate::FatalLocalAuthorityExit>()));
        assert!(manager.runtimes.read().await.is_empty());
        let released_listener = tokio::net::TcpListener::bind(address)
            .await
            .expect("HTTPS supervisor retained its listening socket");
        drop(released_listener);
    })
    .await
    .expect("uncertain settings fatal path did not dispose executor and supervisor");
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&source_aggregate)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled,
    );
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
                panic!("uncertain settings fixture lost {skipped} observable events");
            }
        }
    }
}
