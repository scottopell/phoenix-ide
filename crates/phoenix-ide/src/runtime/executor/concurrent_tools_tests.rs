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
    tokens: std::sync::Mutex<Vec<(String, CancellationToken)>>,
    progress: std::sync::Mutex<Vec<Arc<dyn BashProgressSink>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for ControlledOrdinaryTools {
    fn foreground_execution(&self, _: &str) -> crate::tools::ForegroundExecution {
        crate::tools::ForegroundExecution::Concurrent
    }

    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        context: ToolContext,
    ) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        assert_eq!(name, "bash");
        let command = input["cmd"].as_str().unwrap_or("held-wait");
        self.tokens
            .lock()
            .unwrap()
            .push((command.into(), context.cancel.clone()));
        self.progress
            .lock()
            .unwrap()
            .push(context.bash_progress_sink().unwrap());
        self.started.send(command.into()).unwrap();
        if command == "stubborn-command" {
            self.release_first.acquire().await.unwrap().forget();
        }
        if matches!(command, "slow-command" | "held-wait") {
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
    runtime_with_calls(
        directory,
        fail_fast,
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
    )
}

fn runtime_with_calls(
    directory: &std::path::Path,
    fail_fast: bool,
    calls: Vec<ContentBlock>,
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
        tokens: std::sync::Mutex::default(),
        progress: std::sync::Mutex::default(),
    });
    let llm = Arc::new(MockLlmClient::new("test-model"));
    for (content, end_turn) in [
        (calls, false),
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

#[tokio::test]
async fn completed_special_member_keeps_the_whole_source_round_serial() {
    let directory = tempfile::tempdir().unwrap();
    let (mut runtime, _, _, _) = runtime(directory.path(), false);
    let calls: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|id| {
            ToolCall::new(
                id,
                ToolInput::from_name_and_value("bash", serde_json::json!({"op":"run", "cmd":id})),
            )
        })
        .collect();
    let source = crate::state_machine::AssistantMessage {
        message_id: "mixed-source".into(),
        content: vec![
            ContentBlock::tool_use("native", "spawn_agents", serde_json::json!({"tasks":[]})),
            ContentBlock::tool_use(
                "first",
                "bash",
                serde_json::json!({"op":"run", "cmd":"first"}),
            ),
            ContentBlock::tool_use(
                "second",
                "bash",
                serde_json::json!({"op":"run", "cmd":"second"}),
            ),
        ],
        ..Default::default()
    };
    runtime.state = ConvState::ToolExecuting {
        current_tool: calls[0].clone(),
        remaining_tools: vec![calls[1].clone()],
        completed_results: vec![ToolResult::success("native".into(), "no children".into()).into()],
        assistant_message: source,
        pending_sub_agents: vec![],
    };
    runtime.begin_tool_preparation(calls[0].clone()).unwrap();
    assert!(
        matches!(
            runtime.foreground_dispatch,
            tool_preparation::ForegroundDispatch::Serial(_)
        ),
        "a completed special call still determines the entire original response's serial policy"
    );
}

fn calls(count: usize, command: &str) -> Vec<ContentBlock> {
    (0..count)
        .map(|ordinal| {
            ContentBlock::tool_use(
                format!("call-{ordinal}"),
                "bash",
                serde_json::json!({"op":"run","cmd":command}),
            )
        })
        .collect()
}

async fn send_user(events: &mpsc::Sender<Event>) {
    events
        .send(Event::UserMessage {
            text: "independent foreground batch".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: "batch-input".into(),
            user_agent: None,
            skill_invocation: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn cap_four_refills_only_after_a_sibling_commit() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, events, mut starts, release) =
        runtime_with_calls(directory.path(), false, calls(6, "slow-command"));
    let llm = Arc::clone(&runtime.llm_client);
    let storage = Arc::clone(&runtime.storage);
    let (state_tx, mut states) = watch::channel(ConvState::Idle);
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    send_user(&events).await;
    for _ in 0..4 {
        tokio::time::timeout(Duration::from_secs(60), starts.recv())
            .await
            .unwrap()
            .unwrap();
    }
    tokio::time::pause();
    let extra = tokio::time::timeout(Duration::from_millis(100), starts.recv()).await;
    tokio::time::resume();
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(60), starts.recv())
        .await
        .unwrap()
        .unwrap();
    let before_release = llm.recorded_requests().len();
    release.add_permits(5);
    tokio::time::timeout(
        Duration::from_secs(60),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .unwrap()
    .unwrap();
    events.send(Event::Shutdown).await.unwrap();
    task.await.unwrap();
    assert!(
        extra.is_err(),
        "unissued siblings cannot exceed four retained preparation/execution owners"
    );
    assert_eq!(before_release, 1);
    assert_eq!(llm.recorded_requests().len(), 2);
    let ids: Vec<_> = storage
        .get_all_messages("ordinary-batch")
        .into_iter()
        .filter_map(|message| match message.content {
            MessageContent::Tool(result) => Some(result.tool_use_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        (0..6)
            .map(|ordinal| format!("call-{ordinal}"))
            .collect::<Vec<_>>()
    );
}

async fn prepared_batch(
    content: Vec<ContentBlock>,
) -> (
    BatchRuntime,
    mpsc::UnboundedReceiver<String>,
    tempfile::TempDir,
) {
    prepared_batch_for_scope(content, false).await
}

async fn prepared_batch_for_scope(
    content: Vec<ContentBlock>,
    work_scope: bool,
) -> (
    BatchRuntime,
    mpsc::UnboundedReceiver<String>,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap();
    let (mut runtime, _, starts, _) = runtime_with_calls(directory.path(), false, content.clone());
    if work_scope {
        runtime.context.resource_scope = phoenix_core::work_scope::ResourceScopeKey::Work(
            phoenix_core::work_scope::WorkScopeId::new(),
        );
    }
    runtime.storage.seed_tool_admission_policy(
        "ordinary-batch",
        phoenix_core::domain::tool_availability::ToolAvailability::all(
            runtime.tool_executor.definitions().await,
        ),
    );
    let mut calls = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(ToolCall::new(
                id,
                ToolInput::from_name_and_value(name, input.clone()),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let current_tool = calls.remove(0);
    runtime.state = ConvState::ToolExecuting {
        current_tool: current_tool.clone(),
        remaining_tools: calls,
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: crate::state_machine::AssistantMessage::new(
            "direct-batch-source".into(),
            content,
            None,
            None,
        ),
    };
    runtime.begin_tool_preparation(current_tool).unwrap();
    (runtime, starts, directory)
}

async fn launch_preparations(runtime: &mut BatchRuntime, count: usize) {
    for _ in 0..count {
        let observation = runtime.foreground_dispatch.observe().await;
        assert!(runtime
            .accept_foreground_observation(observation)
            .await
            .unwrap()
            .is_none());
    }
}

async fn drain_cancelled_batch(runtime: &mut BatchRuntime) {
    while !matches!(runtime.state, ConvState::Idle) {
        let observation = tokio::time::timeout(
            Duration::from_secs(60),
            runtime.foreground_dispatch.observe(),
        )
        .await
        .unwrap();
        if let Some(event) = runtime
            .accept_foreground_observation(observation)
            .await
            .unwrap()
        {
            runtime.process_event(event).await.unwrap();
        }
    }
}

fn user_cancel() -> Event {
    Event::UserCancel {
        reason: None,
        cause: crate::state_machine::event::CancelCause::UserRequested,
    }
}

#[tokio::test]
async fn ready_typed_outcome_survives_when_cancel_is_selected_first() {
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
    ];
    let (mut runtime, mut starts, _directory) = prepared_batch(content).await;
    launch_preparations(&mut runtime, 2).await;
    for _ in 0..2 {
        starts.recv().await.unwrap();
    }
    runtime.join_batch_call_for_test("fast").await;
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
    let tokens = runtime.tool_executor.tokens.lock().unwrap();
    assert!(tokens
        .iter()
        .find(|(name, _)| name == "slow-command")
        .unwrap()
        .1
        .is_cancelled());
    assert!(
        !tokens
            .iter()
            .find(|(name, _)| name == "fast-command")
            .unwrap()
            .1
            .is_cancelled(),
        "accepting an already completed receipt must not cancel its returned handle"
    );
    let messages = runtime.storage.get_all_messages("ordinary-batch");
    let results: Vec<_> = messages
        .iter()
        .filter_map(|message| match &message.content {
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
    assert!(results[0].is_error);
    assert!(!results[1].is_error);
    assert!(results[1].content.contains("completed fast-command"));
    assert!(messages.iter().find(|message| matches!(&message.content, MessageContent::Tool(result) if result.tool_use_id == "fast"))
        .unwrap().display_data.as_ref().unwrap()["duration_ms"].is_number());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    assert!(messages.iter().all(|message| !matches!(&message.content, MessageContent::System(text) if text.text.contains("Cancellation completed"))));
}

#[tokio::test]
async fn nonrepresentative_wait_keeps_its_cutoff_and_retires_only_its_task() {
    let content = vec![
        ContentBlock::tool_use(
            "slow",
            "bash",
            serde_json::json!({"op":"run","cmd":"slow-command"}),
        ),
        ContentBlock::tool_use(
            "wait",
            "bash",
            serde_json::json!({"op":"wait","handle":"separate-process", "wait_seconds":0}),
        ),
        ContentBlock::tool_use(
            "fast",
            "bash",
            serde_json::json!({"op":"run","cmd":"fast-command"}),
        ),
    ];
    let (mut runtime, mut starts, _directory) = prepared_batch(content).await;
    launch_preparations(&mut runtime, 3).await;
    for _ in 0..3 {
        starts.recv().await.unwrap();
    }
    runtime.join_batch_call_for_test("fast").await;
    let cutoff = runtime.foreground_dispatch.batch_deadline().unwrap();
    let observation = runtime.foreground_dispatch.observe().await;
    let event = runtime
        .accept_foreground_observation(observation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        runtime.bash_wait_transition_remaining(&runtime.state),
        None,
        "a sibling receipt cannot inherit the wait's cutoff"
    );
    runtime.process_event(event).await.unwrap();
    assert_eq!(runtime.foreground_dispatch.batch_deadline(), Some(cutoff));
    assert_eq!(runtime.deadline, None);
    tokio::time::pause();
    tokio::time::advance(cutoff.saturating_duration_since(tokio::time::Instant::now())).await;
    runtime.expire_batch_waits().await.unwrap();
    tokio::time::resume();
    assert!(runtime.foreground_dispatch.batch_deadline().is_none());
    {
        let tokens = runtime.tool_executor.tokens.lock().unwrap();
        assert!(
            tokens.iter().all(|(_, token)| !token.is_cancelled()),
            "task-only wait timeout cannot cancel either process/controller token"
        );
    }
    let ConvState::ToolExecuting {
        completed_results, ..
    } = &runtime.state
    else {
        panic!("slow call remains owned")
    };
    assert!(completed_results
        .iter()
        .any(|result| result.tool_use_id() == "wait"));
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn registered_bash_fast_sibling_finishes_before_held_foreground_call() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const CONVERSATION: &str = "registered-bash-batch";
    const GUARD: Duration = Duration::from_secs(60);
    let executable = std::path::PathBuf::from(
        std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE")
            .expect("run with dev.py's built native Bash launcher fixture"),
    );
    assert!(executable.is_absolute() && executable.is_file());
    let directory = tempfile::tempdir().unwrap();
    let fifo = |name: &str| {
        let path = directory.path().join(name);
        nix::unistd::mkfifo(
            &path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        let receiver = tokio::net::unix::pipe::OpenOptions::new()
            .open_receiver(&path)
            .unwrap();
        // The keeper prevents EOF before the child opens its writer.
        let sender = tokio::net::unix::pipe::OpenOptions::new()
            .open_sender(&path)
            .unwrap();
        (receiver, sender)
    };
    let (mut held_started, _held_start_keeper) = fifo("held-started");
    let (mut fast_finished, _fast_finish_keeper) = fifo("fast-finished");
    let (_release_receiver, mut release_held) = fifo("release-held");

    let registry = crate::tools::ToolRegistry::direct(vec![]);
    let storage = Arc::new(InMemoryStorage::new());
    storage.seed_tool_admission_policy(
        CONVERSATION,
        phoenix_core::domain::tool_availability::ToolAvailability::all(registry.definitions()),
    );
    let tools = crate::runtime::traits::ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let handles = Arc::new(crate::tools::BashHandleRegistry::new());
    let llm = Arc::new(MockLlmClient::new("test-model"));
    for (content, end_turn) in [
        (
            vec![
                ContentBlock::tool_use(
                    "held",
                    "bash",
                    serde_json::json!({
                        "op":"run",
                        "cmd":"printf h > held-started; IFS= read -r release < release-held || exit 97; printf 'held-result\\n'",
                        "wait_seconds":900,
                        "label":"held-first"
                    }),
                ),
                ContentBlock::tool_use(
                    "fast",
                    "bash",
                    serde_json::json!({
                        "op":"run",
                        "cmd":"printf 'fast-result\\n'; printf f > fast-finished",
                        "wait_seconds":900,
                        "label":"independent-fast"
                    }),
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
    let (events, receiver) = mpsc::channel(16);
    let (state_tx, mut states) = watch::channel(ConvState::Idle);
    let runtime = ConversationRuntime::new(
        ConvContext::new(
            CONVERSATION,
            directory.path().to_path_buf(),
            "test-model",
            200_000,
            16_384,
        ),
        ConvState::Idle,
        Arc::clone(&storage),
        Arc::clone(&llm),
        tools,
        Arc::new(BrowserSessionManager::default()),
        Arc::clone(&handles),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        receiver,
        events.clone(),
        SseBroadcaster::new(128, 0),
    );
    let mut actor = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    events
        .send(Event::UserMessage {
            text: "run the two independent commands".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: "registered-bash-input".into(),
            user_agent: None,
            skill_invocation: None,
        })
        .await
        .unwrap();

    // Capture observations without asserting until after release: the old
    // serial implementation must also release its held native child on failure.
    let before_release = tokio::time::timeout(GUARD, async {
        let mut started = [0];
        held_started
            .read_exact(&mut started)
            .await
            .map_err(|error| error.to_string())?;
        let mut finished = [0];
        fast_finished
            .read_exact(&mut finished)
            .await
            .map_err(|error| error.to_string())?;
        let partial = states
            .wait_for(|state| {
                matches!(state, ConvState::ToolExecuting { completed_results, .. }
                    if completed_results.iter().any(|result| result.tool_use_id() == "fast"))
            })
            .await
            .map_err(|error| error.to_string())?
            .clone();
        Ok::<_, String>((
            started,
            finished,
            partial,
            llm.recorded_requests().len(),
            handles.snapshot_live_pgids().await.len(),
        ))
    })
    .await;

    release_held.write_all(b"release\n").await.unwrap();
    let idle = tokio::time::timeout(
        GUARD,
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .map(|result| result.map(|_| ()));
    let _ = events.send(Event::Shutdown).await;
    let stopped = tokio::time::timeout(GUARD, &mut actor).await;
    if stopped.is_err() {
        actor.abort();
    }

    let (started, finished, partial, requests_while_held, live_while_held) = before_release
        .expect("fast registered Bash sibling must complete while first command is FIFO-held")
        .expect("native markers and state observer must remain available");
    idle.expect("released batch must reach Idle")
        .expect("state watcher must remain available until shutdown");
    assert_eq!(
        stopped.expect("actor shutdown must complete").unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown
    );
    assert_eq!(started, *b"h");
    assert_eq!(finished, *b"f");
    let ConvState::ToolExecuting {
        completed_results, ..
    } = partial
    else {
        panic!("held first command keeps this batch open");
    };
    assert_eq!(
        completed_results
            .iter()
            .map(|result| result.tool_use_id())
            .collect::<Vec<_>>(),
        ["fast"]
    );
    assert_eq!(requests_while_held, 1, "model must await the full batch");
    assert_eq!(live_while_held, 1, "held command must remain alive");
    assert!(handles.snapshot_live_pgids().await.is_empty());

    let messages = storage.get_all_messages(CONVERSATION);
    let results: Vec<_> = messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Tool(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(
        results
            .iter()
            .map(|result| result.tool_use_id.as_str())
            .collect::<Vec<_>>(),
        ["held", "fast"]
    );
    for (result, (expected_output, label)) in results.iter().zip([
        ("held-result", "held-first"),
        ("fast-result", "independent-fast"),
    ]) {
        assert!(!result.is_error);
        let output: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(output["status"], "exited");
        assert_eq!(output["exit_code"], 0);
        assert_eq!(output["label"], label);
        assert!(result.content.contains(expected_output));
    }
    let requests = llm.recorded_requests();
    assert_eq!(requests.len(), 2);
    let model_results: Vec<_> = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => Some((tool_use_id.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(model_results, [("held", false), ("fast", false)]);
}

#[tokio::test]
async fn cancel_stops_all_four_active_calls_and_never_starts_unissued_tail() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, events, mut starts, _) =
        runtime_with_calls(directory.path(), false, calls(6, "slow-command"));
    let tools = Arc::clone(&runtime.tool_executor);
    let storage = Arc::clone(&runtime.storage);
    let llm = Arc::clone(&runtime.llm_client);
    let (state_tx, mut states) = watch::channel(ConvState::Idle);
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    send_user(&events).await;
    for _ in 0..4 {
        starts.recv().await.unwrap();
    }
    tokio::time::pause();
    events.send(user_cancel()).await.unwrap();
    let timely = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    tokio::time::resume();
    events.send(Event::Shutdown).await.unwrap();
    task.await.unwrap();
    assert!(
        timely,
        "batch cancellation must settle within 100ms under the RAM fixture"
    );
    assert!(
        starts.try_recv().is_err(),
        "queued source members must never launch after Cancel"
    );
    let tokens = tools.tokens.lock().unwrap();
    assert_eq!(tokens.len(), 4);
    assert!(tokens.iter().all(|(_, token)| token.is_cancelled()));
    assert_eq!(llm.recorded_requests().len(), 1);
    let messages = storage.get_all_messages("ordinary-batch");
    let results: Vec<_> = messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Tool(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 6);
    assert_eq!(
        results
            .iter()
            .map(|result| result.tool_use_id.clone())
            .collect::<Vec<_>>(),
        (0..6)
            .map(|ordinal| format!("call-{ordinal}"))
            .collect::<Vec<_>>()
    );
    assert!(results[4..]
        .iter()
        .all(|result| result.content.contains("Skipped due to cancellation")));
}

async fn cancellation_during_batch_read(work_scope: bool) {
    let (runtime, mut starts, _directory) =
        prepared_batch_for_scope(calls(6, "slow-command"), work_scope).await;
    let (entered, release) = if work_scope {
        runtime.storage.hold_next_tool_scope_read()
    } else {
        runtime.storage.hold_next_tool_policy_read()
    };
    let tools = Arc::clone(&runtime.tool_executor);
    let (events, receiver) = mpsc::channel(8);
    let mut runtime = runtime;
    runtime.event_tx = events.clone();
    runtime.event_rx = receiver;
    let (state_tx, mut states) = watch::channel(runtime.state.clone());
    let task = tokio::spawn(runtime.with_state_watcher(state_tx).run());
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    events.send(user_cancel()).await.unwrap();
    let timely = tokio::time::timeout(
        Duration::from_millis(100),
        states.wait_for(|state| matches!(state, ConvState::Idle)),
    )
    .await
    .is_ok();
    let revoked = release.send(Ok(())).is_err();
    tokio::time::resume();
    events.send(Event::Shutdown).await.unwrap();
    task.await.unwrap();
    assert!(timely, "a held batch read cannot queue Cancel beyond 100ms");
    assert!(
        revoked,
        "cancel revokes the held read before its checkpoint completes"
    );
    let executed = tools.tokens.lock().unwrap();
    assert!(
        executed.len() <= 3,
        "held read and unissued tail cannot launch late"
    );
    assert!(executed.iter().all(|(_, token)| token.is_cancelled()));
    let mut count = 0;
    while starts.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, executed.len());
}

#[tokio::test]
async fn batch_policy_read_is_selected_with_immediate_cancellation() {
    cancellation_during_batch_read(false).await;
}

#[tokio::test]
async fn batch_scope_read_is_selected_with_immediate_cancellation() {
    cancellation_during_batch_read(true).await;
}

#[tokio::test]
async fn rejected_cancel_checkpoint_signals_all_controls_and_keeps_queued_proofs() {
    use futures::FutureExt as _;
    let (mut runtime, mut starts, _directory) = prepared_batch(calls(6, "slow-command")).await;
    launch_preparations(&mut runtime, 4).await;
    for _ in 0..4 {
        starts.recv().await.unwrap();
    }
    runtime.storage.set_fail_state_update(true);
    assert!(runtime
        .process_event(user_cancel())
        .await
        .unwrap_err()
        .contains("injected state update failure"));
    assert!(matches!(runtime.state, ConvState::ToolExecuting { .. }));
    assert!(runtime
        .tool_executor
        .tokens
        .lock()
        .unwrap()
        .iter()
        .all(|(_, token)| token.is_cancelled()));
    assert!(
        runtime
            .foreground_dispatch
            .observe()
            .now_or_never()
            .is_none(),
        "uncommitted cancellation cannot publish aborts or ordinary advancement"
    );
    assert!(starts.try_recv().is_err());
    assert!(runtime.llm_client.recorded_requests().is_empty());
    runtime.storage.set_fail_state_update(false);
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
    assert_eq!(runtime.tool_executor.tokens.lock().unwrap().len(), 4);
}

async fn required_sibling_read_failure(work_scope: bool) {
    let (mut runtime, mut starts, _directory) =
        prepared_batch_for_scope(calls(2, "slow-command"), work_scope).await;
    launch_preparations(&mut runtime, 1).await;
    starts.recv().await.unwrap();
    let (entered, release) = if work_scope {
        runtime.storage.hold_next_tool_scope_read()
    } else {
        runtime.storage.hold_next_tool_policy_read()
    };
    let tools = Arc::clone(&runtime.tool_executor);
    let llm = Arc::clone(&runtime.llm_client);
    let fence = runtime.fatal_local_authority_fence.clone();
    let task = tokio::spawn(runtime.run());
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .unwrap()
        .unwrap();
    release
        .send(Err("required sibling authority read failed".into()))
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(60), task)
            .await
            .unwrap()
            .unwrap(),
        RuntimeExitDisposition::FatalLocalAuthorityLoss
    );
    assert!(fence.fatal_authority_loss_is_latched());
    let tokens = tools.tokens.lock().unwrap();
    assert_eq!(tokens.len(), 1);
    assert!(tokens[0].1.is_cancelled());
    assert!(llm.recorded_requests().is_empty());
}

#[tokio::test]
async fn concurrent_required_sibling_policy_error_keeps_its_settlement_boundary() {
    required_sibling_read_failure(false).await;
}

#[tokio::test]
async fn concurrent_required_sibling_scope_error_keeps_its_settlement_boundary() {
    required_sibling_read_failure(true).await;
}

#[tokio::test]
async fn batch_denial_precedes_every_call_resource_and_preserves_sibling() {
    let content = vec![
        ContentBlock::tool_use(
            "denied",
            "bash",
            serde_json::json!({"op":"run", "cmd":"git add -A"}),
        ),
        ContentBlock::tool_use(
            "held",
            "bash",
            serde_json::json!({"op":"run", "cmd":"slow-command"}),
        ),
    ];
    let (mut runtime, mut starts, _directory) = prepared_batch(content).await;
    let mut events = runtime.broadcast_tx.subscribe();
    let observation = runtime.foreground_dispatch.observe().await;
    let event = runtime
        .accept_foreground_observation(observation)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&event, Event::ToolComplete { tool_use_id, result } if tool_use_id == "denied" && result.is_error())
    );
    assert!(runtime.tool_executor.tokens.lock().unwrap().is_empty());
    assert!(
        events.try_recv().is_err(),
        "denial cannot publish ordinary start resources"
    );
    runtime.process_event(event).await.unwrap();
    launch_preparations(&mut runtime, 1).await;
    starts.recv().await.unwrap();
    assert_eq!(runtime.tool_executor.tokens.lock().unwrap().len(), 1);
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
}

#[tokio::test]
async fn stale_batch_preparation_cannot_adopt_changed_source_with_reused_ids() {
    let (mut runtime, _, _directory) = prepared_batch(calls(2, "slow-command")).await;
    let observation = runtime.foreground_dispatch.observe().await;
    let ConvState::ToolExecuting {
        assistant_message, ..
    } = &mut runtime.state
    else {
        unreachable!()
    };
    assistant_message.content[0] = ContentBlock::tool_use(
        "call-0",
        "bash",
        serde_json::json!({"op":"run","cmd":"changed-source"}),
    );
    assert!(runtime
        .accept_foreground_observation(observation)
        .await
        .unwrap()
        .is_none());
    assert!(runtime.tool_executor.tokens.lock().unwrap().is_empty());
    runtime.abort_external_effects_in_memory();
}

#[tokio::test]
async fn shared_unknown_authority_precedes_stale_batch_selection() {
    let (mut runtime, _, _directory) = prepared_batch(calls(2, "slow-command")).await;
    let observation = runtime.foreground_dispatch.observe().await;
    let ConvState::ToolExecuting {
        assistant_message, ..
    } = &mut runtime.state
    else {
        unreachable!()
    };
    assistant_message.message_id = "successor".into();
    runtime
        .fatal_local_authority_fence
        .close("test_genuine_unknown_local_write");
    assert!(runtime
        .accept_foreground_observation(observation)
        .await
        .unwrap_err()
        .contains("fatal local authority loss"));
    assert!(runtime.tool_executor.tokens.lock().unwrap().is_empty());
    runtime.abort_external_effects_in_memory();
}

#[tokio::test]
async fn forced_batch_retirement_drains_ready_receipts_and_retires_every_owner_once() {
    let mut content = calls(6, "stubborn-command");
    content[1] = ContentBlock::tool_use(
        "call-1",
        "bash",
        serde_json::json!({"op":"run","cmd":"fast-command"}),
    );
    let (mut runtime, mut starts, _directory) = prepared_batch(content).await;
    launch_preparations(&mut runtime, 4).await;
    for _ in 0..4 {
        starts.recv().await.unwrap();
    }
    runtime.join_batch_call_for_test("call-1").await;
    runtime.process_event(user_cancel()).await.unwrap();
    let deadline = runtime.deadline.unwrap();
    tokio::time::pause();
    tokio::time::advance(deadline.saturating_duration_since(tokio::time::Instant::now())).await;
    runtime.handle_cancelling_tool_timeout().await;
    tokio::time::resume();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(starts.try_recv().is_err());
    let messages = runtime.storage.get_all_messages("ordinary-batch");
    let results: Vec<_> = messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Tool(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 6);
    assert_eq!(
        results
            .iter()
            .map(|result| result.tool_use_id.clone())
            .collect::<Vec<_>>(),
        (0..6)
            .map(|ordinal| format!("call-{ordinal}"))
            .collect::<Vec<_>>()
    );
    assert!(!results[1].is_error);
    assert!(results[1].content.contains("completed fast-command"));
    assert_eq!(messages.iter().filter(|message| matches!(&message.content, MessageContent::System(system) if system.text.contains("Cancellation completed"))).count(), 1);
    assert!(runtime.llm_client.recorded_requests().is_empty());
}

#[tokio::test]
async fn retired_progress_cannot_publish_into_a_later_round_reusing_call_ids() {
    let (mut runtime, mut starts, _directory) = prepared_batch(calls(2, "slow-command")).await;
    launch_preparations(&mut runtime, 2).await;
    for _ in 0..2 {
        starts.recv().await.unwrap();
    }
    let old_sink = runtime.tool_executor.progress.lock().unwrap()[0].clone();
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
    let mut events = runtime.broadcast_tx.subscribe();
    runtime.state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            "call-0",
            ToolInput::from_name_and_value(
                "bash",
                serde_json::json!({"op":"run","cmd":"successor"}),
            ),
        ),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: crate::state_machine::AssistantMessage::new(
            "new-assistant".into(),
            calls(1, "successor"),
            None,
            None,
        ),
    };
    old_sink.emit(phoenix_core::domain::bash_progress::BashToolProgress {
        handle: "retired-handle".into(),
        start_offset: 0,
        end_offset: 0,
        truncated_before: false,
        lines: vec![],
        partial: Some("late old output".into()),
    });
    assert!(events.try_recv().is_err(), "a retired invocation loses its progress publication capability even if its provider ID recurs");
}
