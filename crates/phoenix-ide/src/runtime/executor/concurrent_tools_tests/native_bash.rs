use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONVERSATION: &str = "registered-bash-batch";
const GUARD: Duration = Duration::from_secs(60);
type NativeRuntime = ConversationRuntime<
    Arc<InMemoryStorage>,
    Arc<MockLlmClient>,
    crate::runtime::traits::ToolRegistryExecutor,
>;

fn fifo(
    directory: &std::path::Path,
    name: &str,
) -> (
    tokio::net::unix::pipe::Receiver,
    tokio::net::unix::pipe::Sender,
) {
    let path = directory.join(name);
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
}

fn queue_native_responses(llm: &MockLlmClient) {
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
}

fn native_runtime(
    directory: &std::path::Path,
) -> (
    NativeRuntime,
    mpsc::Sender<Event>,
    watch::Receiver<ConvState>,
) {
    let registry = crate::tools::ToolRegistry::direct(vec![]);
    let storage = Arc::new(InMemoryStorage::new());
    storage.seed_tool_admission_policy(
        CONVERSATION,
        phoenix_core::domain::tool_availability::ToolAvailability::all(registry.definitions()),
    );
    let tools = crate::runtime::traits::ToolRegistryExecutor::builtin_only(registry, Arc::from([]));
    let handles = Arc::new(crate::tools::BashHandleRegistry::new());
    let llm = Arc::new(MockLlmClient::new("test-model"));
    queue_native_responses(&llm);
    let (events, receiver) = mpsc::channel(16);
    let (state_tx, states) = watch::channel(ConvState::Idle);
    let runtime = ConversationRuntime::new(
        ConvContext::new(
            CONVERSATION,
            directory.to_path_buf(),
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
    (runtime.with_state_watcher(state_tx), events, states)
}

fn assert_native_results(storage: &InMemoryStorage, llm: &MockLlmClient) {
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
        let output: serde_json::Value = serde_json::from_str(result.content()).unwrap();
        assert_eq!(output["status"], "exited");
        assert_eq!(output["exit_code"], 0);
        assert_eq!(output["label"], label);
        assert!(result.content().contains(expected_output));
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

type HeldObservation = ([u8; 1], [u8; 1], ConvState, usize, usize);

async fn observe_native_overlap(
    held_started: &mut tokio::net::unix::pipe::Receiver,
    fast_finished: &mut tokio::net::unix::pipe::Receiver,
    states: &mut watch::Receiver<ConvState>,
    llm: &MockLlmClient,
    handles: &crate::tools::BashHandleRegistry,
) -> Result<HeldObservation, String> {
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
}

#[tokio::test]
async fn registered_bash_fast_sibling_finishes_before_held_foreground_call() {
    let executable = std::path::PathBuf::from(
        std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE")
            .expect("run with dev.py's built native Bash launcher fixture"),
    );
    assert!(executable.is_absolute() && executable.is_file());
    let directory = tempfile::tempdir().unwrap();
    let (mut held_started, _held_start_keeper) = fifo(directory.path(), "held-started");
    let (mut fast_finished, _fast_finish_keeper) = fifo(directory.path(), "fast-finished");
    let (_release_receiver, mut release_held) = fifo(directory.path(), "release-held");

    let (runtime, events, mut states) = native_runtime(directory.path());
    let storage = Arc::clone(&runtime.storage);
    let llm = Arc::clone(&runtime.llm_client);
    let handles = Arc::clone(&runtime.bash_handles);
    let mut actor = tokio::spawn(runtime.run());
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
    let before_release = tokio::time::timeout(
        GUARD,
        observe_native_overlap(
            &mut held_started,
            &mut fast_finished,
            &mut states,
            &llm,
            &handles,
        ),
    )
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
            .map(ToolRoundResult::tool_use_id)
            .collect::<Vec<_>>(),
        ["fast"]
    );
    assert_eq!(requests_while_held, 1, "model must await the full batch");
    assert_eq!(live_while_held, 1, "held command must remain alive");
    assert!(handles.snapshot_live_pgids().await.is_empty());

    assert_native_results(&storage, &llm);
}
