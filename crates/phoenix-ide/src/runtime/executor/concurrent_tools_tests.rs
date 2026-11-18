use super::*;
use crate::runtime::testing::{InMemoryStorage, MockLlmClient};
use crate::runtime::traits::ToolExecutor;
use crate::tools::{ToolContext, ToolOutput};
use phoenix_llm::{LlmResponse, ToolDefinition, Usage};
use tokio::sync::Semaphore;

struct ControlledOrdinaryTools {
    started: mpsc::UnboundedSender<String>,
    release_first: Arc<Semaphore>,
    fail_fast: bool,
}

#[async_trait::async_trait]
impl ToolExecutor for ControlledOrdinaryTools {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        context: ToolContext,
    ) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        assert_eq!(name, "bash");
        let command = input["cmd"].as_str().unwrap();
        self.started.send(command.into()).unwrap();
        if command == "slow-command" {
            tokio::select! {
                () = context.cancel.cancelled() => return Some(ToolOutput::error("cancelled")),
                permit = self.release_first.acquire() => permit.unwrap().forget(),
            }
        }
        Some(if self.fail_fast && command == "fast-command" {
            ToolOutput::error("independent fast failure")
        } else {
            ToolOutput::success(format!("completed {command}"))
        })
    }

    async fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "bash".into(),
            description: "Controlled ordinary call".into(),
            input_schema: serde_json::json!({"type":"object"}),
            defer_loading: false,
        }]
    }
}

type BatchRuntime =
    ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<ControlledOrdinaryTools>>;

fn runtime(
    directory: &std::path::Path,
    fail_fast: bool,
) -> (
    BatchRuntime,
    mpsc::Sender<Event>,
    mpsc::UnboundedReceiver<String>,
    Arc<Semaphore>,
) {
    let (events, receiver) = mpsc::channel(16);
    let (started, observations) = mpsc::unbounded_channel();
    let release_first = Arc::new(Semaphore::new(0));
    let tools = Arc::new(ControlledOrdinaryTools {
        started,
        release_first: Arc::clone(&release_first),
        fail_fast,
    });
    let llm = Arc::new(MockLlmClient::new("test-model"));
    for (content, end_turn) in [
        (
            vec![
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
            ],
            false,
        ),
        (vec![ContentBlock::text("complete batch observed")], true),
    ] {
        llm.queue_response(LlmResponse {
            provider_replay: None,
            content,
            end_turn,
            usage: Usage::default(),
            usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
            stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
        });
    }
    let runtime = ConversationRuntime::new(
        ConvContext::new(
            "ordinary-batch",
            directory.to_path_buf(),
            "test-model",
            200_000,
            16_384,
        ),
        ConvState::Idle,
        Arc::new(InMemoryStorage::new()),
        llm,
        tools,
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        receiver,
        events.clone(),
        SseBroadcaster::new(128, 0),
    );
    (runtime, events, observations, release_first)
}

async fn held_first_actor_journey(fail_fast: bool) {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, events, mut starts, release_first) = runtime(directory.path(), fail_fast);
    let llm = Arc::clone(&runtime.llm_client);
    let storage = Arc::clone(&runtime.storage);
    let (state_tx, mut states) = watch::channel(runtime.state.clone());
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    events
        .send(Event::UserMessage {
            text: "run these independent ordinary calls".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: "batch-input".into(),
            user_agent: None,
            skill_invocation: None,
        })
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(60), starts.recv())
            .await
            .unwrap()
            .as_deref(),
        Some("slow-command")
    );
    tokio::time::pause();
    let sibling_accepted_while_first_held = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| {
            matches!(state, ConvState::ToolExecuting { completed_results, .. }
            if completed_results.iter().any(|result| result.tool_use_id() == "fast"))
        }),
    )
    .await
    .is_ok();
    let requests_before_barrier = llm.recorded_requests().len();
    tokio::time::resume();
    release_first.add_permits(1);
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
    assert!(
        sibling_accepted_while_first_held,
        "a held first call must not prevent a fast sibling's owning commit"
    );
    assert_eq!(requests_before_barrier, 1);
    assert_eq!(llm.recorded_requests().len(), 2);
    let results: Vec<_> = storage
        .get_all_messages("ordinary-batch")
        .into_iter()
        .filter_map(|message| match message.content {
            MessageContent::Tool(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(
        results
            .iter()
            .map(|result| result.tool_use_id.as_str())
            .collect::<Vec<_>>(),
        ["slow", "fast"]
    );
    assert!(!results[0].is_error);
    assert_eq!(results[1].is_error, fail_fast);
}

#[tokio::test]
async fn fast_sibling_commits_while_first_foreground_call_is_held() {
    held_first_actor_journey(false).await;
}

#[tokio::test]
async fn ordinary_sibling_error_preserves_peer_and_the_model_barrier() {
    held_first_actor_journey(true).await;
}
