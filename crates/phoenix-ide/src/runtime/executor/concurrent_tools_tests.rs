use super::*;
use crate::runtime::testing::{InMemoryStorage, MockLlmClient};
use crate::runtime::traits::ToolExecutor;
use crate::tools::{ToolContext, ToolOutput};
use phoenix_llm::{LlmResponse, ToolDefinition, Usage};
use tokio::sync::Semaphore;

async fn next_started(starts: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(60), starts.recv())
        .await
        .expect("owned tool start must become observable")
        .expect("the owning tool task retains its start sender")
}

struct ControlledOrdinaryTools {
    started: mpsc::UnboundedSender<String>,
    release_first: Arc<Semaphore>,
    fail_fast: bool,
    tokens: std::sync::Mutex<Vec<(String, CancellationToken)>>,
    progress: std::sync::Mutex<Vec<Arc<dyn BashProgressSink>>>,
    fast_release: std::sync::Mutex<Option<Arc<Semaphore>>>,
    finished: std::sync::Mutex<Vec<String>>,
    execution_finished: tokio::sync::Notify,
}

struct ExecutionDrop<'a> {
    tools: &'a ControlledOrdinaryTools,
    command: &'a str,
}

impl Drop for ExecutionDrop<'_> {
    fn drop(&mut self) {
        self.tools
            .finished
            .lock()
            .unwrap()
            .push(self.command.into());
        self.tools.execution_finished.notify_one();
    }
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
        let _execution = ExecutionDrop {
            tools: self,
            command,
        };
        self.tokens
            .lock()
            .unwrap()
            .push((command.into(), context.cancel.clone()));
        self.progress
            .lock()
            .unwrap()
            .push(context.bash_progress_sink().unwrap());
        self.started.send(command.into()).unwrap();
        let fast_release = (command == "fast-command")
            .then(|| self.fast_release.lock().unwrap().clone())
            .flatten();
        if let Some(release) = fast_release {
            release.acquire().await.unwrap().forget();
        }
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
        fast_release: std::sync::Mutex::default(),
        finished: std::sync::Mutex::default(),
        execution_finished: tokio::sync::Notify::new(),
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
    runtime.begin_tool_preparation(&calls[0]).unwrap();
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
    prepared_batch_for_role(content, work_scope, false).await
}

async fn prepared_batch_for_role(
    content: Vec<ContentBlock>,
    work_scope: bool,
    child: bool,
) -> (
    BatchRuntime,
    mpsc::UnboundedReceiver<String>,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap();
    let (mut runtime, _, starts, _) = runtime_with_calls(directory.path(), false, content.clone());
    runtime.context.is_sub_agent = child;
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
    runtime.begin_tool_preparation(&current_tool).unwrap();
    (runtime, starts, directory)
}

async fn launch_preparations(runtime: &mut BatchRuntime, count: usize) {
    for _ in 0..count {
        let observation = runtime.foreground_dispatch.observe().await;
        assert!(runtime
            .accept_foreground_observation(observation)
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
        if let Some(event) = runtime.accept_foreground_observation(observation).unwrap() {
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
        next_started(&mut starts).await;
    }
    runtime.join_batch_call_for_test("fast").await;
    runtime.process_event(user_cancel()).await.unwrap();
    let mut observed_slow_cancellation = false;
    while !matches!(runtime.state, ConvState::Idle) {
        let observation = runtime.foreground_dispatch.observe().await;
        if let Some(event) = runtime.accept_foreground_observation(observation).unwrap() {
            observed_slow_cancellation |=
                matches!(&event, Event::ToolAborted { tool_use_id } if tool_use_id == "slow");
            runtime.process_event(event).await.unwrap();
        }
    }
    assert!(
        observed_slow_cancellation,
        "the unfinished call retains its typed cancellation disposition"
    );
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
    assert_eq!(results[0].content, "Cancelled by user");
    assert!(
        !results[0].is_error,
        "Cancelled retains its existing transcript wire disposition"
    );
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
        next_started(&mut starts).await;
    }
    runtime.join_batch_call_for_test("fast").await;
    let cutoff = runtime.foreground_dispatch.batch_deadline().unwrap();
    let observation = runtime.foreground_dispatch.observe().await;
    let event = runtime
        .accept_foreground_observation(observation)
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
        next_started(&mut starts).await;
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
        next_started(&mut starts).await;
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
    next_started(&mut starts).await;
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
    next_started(&mut starts).await;
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
        next_started(&mut starts).await;
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
        next_started(&mut starts).await;
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

#[tokio::test]
async fn child_batch_notifies_parent_only_after_all_owners_and_full_checkpoint() {
    let mut content = calls(2, "slow-command");
    content[1] = ContentBlock::tool_use(
        "call-1",
        "bash",
        serde_json::json!({"op":"run","cmd":"stubborn-command"}),
    );
    let (mut runtime, mut starts, _directory) = prepared_batch_for_role(content, false, true).await;
    let (parent, mut notifications) = mpsc::channel(4);
    runtime.parent_dispatch = Some((
        "parent".into(),
        Arc::new(crate::runtime::TestConversationEventDispatcher::new(parent)),
    ));
    launch_preparations(&mut runtime, 2).await;
    for _ in 0..2 {
        next_started(&mut starts).await;
    }
    runtime.process_event(user_cancel()).await.unwrap();
    runtime.join_batch_call_for_test("call-0").await;
    let observation = runtime.foreground_dispatch.observe().await;
    let event = runtime
        .accept_foreground_observation(observation)
        .unwrap()
        .unwrap();
    runtime.process_event(event).await.unwrap();
    assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
    assert!(
        notifications.try_recv().is_err(),
        "a child cannot notify parent while a sibling still owns an invocation"
    );
    let deadline = runtime.deadline.unwrap();
    tokio::time::pause();
    tokio::time::advance(deadline.saturating_duration_since(tokio::time::Instant::now())).await;
    runtime.handle_cancelling_tool_timeout().await;
    tokio::time::resume();
    assert!(matches!(runtime.state, ConvState::Failed { .. }));
    assert!(matches!(
        runtime.storage.get_current_state("ordinary-batch"),
        Some(ConvState::Failed { .. })
    ));
    let event = notifications
        .try_recv()
        .expect("complete child barrier notifies parent");
    assert!(
        matches!(event, Event::SubAgentResult { agent_id, .. } if agent_id == "ordinary-batch")
    );
    assert!(notifications.try_recv().is_err());
    let results: Vec<_> = runtime
        .storage
        .get_all_messages("ordinary-batch")
        .into_iter()
        .filter_map(|message| match message.content {
            MessageContent::Tool(result) => Some(result.tool_use_id),
            _ => None,
        })
        .collect();
    assert_eq!(results, ["call-0", "call-1"]);
    assert!(runtime.llm_client.recorded_requests().is_empty());
}

#[tokio::test]
async fn serial_special_source_cancellation_drains_affirmative_unissued_tail() {
    let content = vec![
        ContentBlock::tool_use("native", "spawn_agents", serde_json::json!({"tasks":[]})),
        ContentBlock::tool_use(
            "first",
            "bash",
            serde_json::json!({"op":"run","cmd":"slow-command"}),
        ),
        ContentBlock::tool_use(
            "second",
            "bash",
            serde_json::json!({"op":"run","cmd":"slow-command"}),
        ),
    ];
    let directory = tempfile::tempdir().unwrap();
    let (mut runtime, _, _, _) = runtime_with_calls(directory.path(), false, content.clone());
    let first = ToolCall::new(
        "first",
        ToolInput::from_name_and_value(
            "bash",
            serde_json::json!({"op":"run","cmd":"slow-command"}),
        ),
    );
    let second = ToolCall::new("second", first.input.clone());
    runtime.state = ConvState::ToolExecuting {
        current_tool: first.clone(),
        remaining_tools: vec![second],
        completed_results: vec![ToolResult::success(
            "native".into(),
            "authentic native completion".into(),
        )
        .into()],
        pending_sub_agents: vec![],
        assistant_message: crate::state_machine::AssistantMessage::new(
            "mixed-owner".into(),
            content,
            None,
            None,
        ),
    };
    runtime.begin_tool_preparation(&first).unwrap();
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
    assert!(runtime.tool_executor.tokens.lock().unwrap().is_empty());
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
        ["native", "first", "second"]
    );
    assert_eq!(results[0].content, "authentic native completion");
    assert_eq!(results[2].content, "Skipped due to cancellation");
}

#[tokio::test]
async fn lost_batch_task_settles_one_exact_error_without_cancelling_its_sibling() {
    use futures::FutureExt as _;
    let (mut runtime, mut starts, _directory) = prepared_batch(calls(2, "slow-command")).await;
    launch_preparations(&mut runtime, 2).await;
    for _ in 0..2 {
        next_started(&mut starts).await;
    }
    runtime.abort_batch_call_for_test("call-1").await;
    let observation = runtime.foreground_dispatch.observe().await;
    let event = runtime
        .accept_foreground_observation(observation)
        .unwrap()
        .unwrap();
    assert!(
        matches!(&event, Event::ToolComplete { tool_use_id, result } if tool_use_id == "call-1" && result.is_error() && result.output().contains("aborted or panicked"))
    );
    runtime.process_event(event).await.unwrap();
    assert!(
        runtime
            .foreground_dispatch
            .observe()
            .now_or_never()
            .is_none(),
        "the lost sender owns exactly one failure receipt"
    );
    assert!(runtime
        .tool_executor
        .tokens
        .lock()
        .unwrap()
        .iter()
        .all(|(_, token)| !token.is_cancelled()));
    assert!(runtime.llm_client.recorded_requests().is_empty());
    runtime.process_event(user_cancel()).await.unwrap();
    drain_cancelled_batch(&mut runtime).await;
}

async fn sqlite_batch_database(directory: &std::path::Path) -> crate::db::Database {
    let db = crate::db::Database::open(directory.join("batch-commit.db").to_str().unwrap())
        .await
        .unwrap();
    crate::db::run_pending_migrations(db.pool()).await.unwrap();
    db.create_conversation(
        "ordinary-batch",
        "ordinary-batch",
        directory.to_str().unwrap(),
        true,
        None,
        None,
    )
    .await
    .unwrap();
    db
}

fn sqlite_batch_state(content: Vec<ContentBlock>) -> ConvState {
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
    ConvState::ToolExecuting {
        current_tool: current_tool.clone(),
        remaining_tools: calls,
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: crate::state_machine::AssistantMessage::new(
            "sqlite-batch-owner".into(),
            content,
            None,
            None,
        ),
    }
}

async fn seed_sqlite_tool_policy(db: &crate::db::Database, definitions: &[ToolDefinition]) {
    db.prepare_tool_availability(
        "ordinary-batch",
        "batch-commit-test",
        None,
        definitions,
        &definitions
            .iter()
            .map(|definition| definition.name.clone())
            .collect(),
        &[],
        &[],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn sqlite_rejects_sibling_commit_without_refill_publication_or_model_advance() {
    let directory = tempfile::tempdir().unwrap();
    let db = sqlite_batch_database(directory.path()).await;
    let mut content = calls(6, "slow-command");
    content[1] = ContentBlock::tool_use(
        "call-1",
        "bash",
        serde_json::json!({"op":"run","cmd":"fast-command"}),
    );
    let (ram, _, mut starts, _) = runtime_with_calls(directory.path(), false, content.clone());
    let tools = ram.tool_executor.as_ref().clone();
    let llm = ram.llm_client.as_ref().clone();
    let release_fast = Arc::new(Semaphore::new(0));
    *tools.fast_release.lock().unwrap() = Some(release_fast.clone());
    let definitions = tools.definitions().await;
    seed_sqlite_tool_policy(&db, &definitions).await;
    let initial = sqlite_batch_state(content);
    let ConvState::ToolExecuting { current_tool, .. } = &initial else {
        unreachable!()
    };
    let current_tool = current_tool.clone();
    db.update_conversation_state("ordinary-batch", &initial)
        .await
        .unwrap();
    let (events, receiver) = mpsc::channel(16);
    let (watcher, observed) = watch::channel(initial.clone());
    let mut runtime = ConversationRuntime::new(
        ram.context.clone(),
        initial.clone(),
        crate::runtime::traits::DatabaseStorage::new(db.clone()),
        llm.clone(),
        tools.clone(),
        ram.browser_sessions.clone(),
        ram.bash_handles.clone(),
        ram.tmux_registry.clone(),
        ram.llm_registry.clone(),
        ram.terminals.clone(),
        receiver,
        events,
        SseBroadcaster::new(128, 0),
    )
    .with_state_watcher(watcher);
    runtime.begin_tool_preparation(&current_tool).unwrap();
    for _ in 0..4 {
        let observation = runtime.foreground_dispatch.observe().await;
        assert!(runtime
            .accept_foreground_observation(observation)
            .unwrap()
            .is_none());
    }
    for _ in 0..4 {
        next_started(&mut starts).await;
    }
    sqlx::query("CREATE TRIGGER reject_batch_state BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT, 'batch sibling state commit rejected'); END")
        .execute(db.pool()).await.unwrap();
    release_fast.add_permits(1);
    runtime.join_batch_call_for_test("call-1").await;

    let mut publications = runtime.broadcast_tx.subscribe();
    let observation = runtime.foreground_dispatch.observe().await;
    let event = runtime
        .accept_foreground_observation(observation)
        .unwrap()
        .unwrap();
    let error = runtime.process_event(event).await.unwrap_err();
    assert!(
        error.contains("batch sibling state commit rejected"),
        "{error}"
    );
    assert!(runtime
        .fatal_local_authority_fence
        .fatal_authority_loss_is_latched());
    assert_eq!(*observed.borrow(), initial);
    assert_eq!(
        db.get_conversation("ordinary-batch").await.unwrap().state,
        initial
    );
    assert!(db.get_messages("ordinary-batch").await.unwrap().is_empty());
    assert!(publications.try_recv().is_err());
    assert!(starts.try_recv().is_err());
    assert_eq!(tools.tokens.lock().unwrap().len(), 4);
    assert!(llm.recorded_requests().is_empty());
    assert_eq!(
        Box::pin(runtime.run()).await,
        RuntimeExitDisposition::FatalLocalAuthorityLoss
    );
    assert!(tools
        .tokens
        .lock()
        .unwrap()
        .iter()
        .filter(|(command, _)| command == "slow-command")
        .all(|(_, token)| token.is_cancelled()));
}

mod commit_cancellation;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native_bash;
