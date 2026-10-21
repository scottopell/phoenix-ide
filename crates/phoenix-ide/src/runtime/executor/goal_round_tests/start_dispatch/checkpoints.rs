#[path = "checkpoints/coordinator.rs"]
mod coordinator;

use super::*;
use crate::runtime::executor::goal_request_checkpoint::GoalRequestCompletion;

fn provider_response(content: Vec<ContentBlock>) -> phoenix_core::domain::llm_types::LlmResponse {
    phoenix_core::domain::llm_types::LlmResponse::non_streaming(
        content,
        true,
        phoenix_core::domain::llm_types::Usage::default(),
    )
}

fn provider_response_with_replay(
    response_id: &str,
    content: Vec<ContentBlock>,
) -> phoenix_core::domain::llm_types::LlmResponse {
    use phoenix_core::domain::provider_replay::{AnthropicResponseIdentity, AnthropicResponseSet};
    let replay = AnthropicResponseSet::with_public_content(
        AnthropicResponseIdentity {
            response_id: response_id.into(),
            model: "claude-opus-5-5".into(),
        },
        content.clone(),
        Vec::new(),
    )
    .unwrap();
    let mut response = provider_response(content);
    response.provider_replay = Some(ProviderReplayUpdate::Anthropic(replay));
    response
}

async fn dispatch_response<T: ToolExecutor + 'static>(
    rt: &mut ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, T>,
) -> GoalRequestCompletion {
    rt.execute_effect(Effect::RequestLlm).await.unwrap();
    rt.llm_task_handle.as_mut().unwrap().await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        rt.goal_request_completion_rx.recv(),
    )
    .await
    .expect("actual provider completion liveness")
    .expect("receipt owner remains open")
}

fn actual_tool_runtime(
    initial: &mut TestRuntime,
    db: &Database,
    client: Arc<RenderingAnthropicClient>,
) -> ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, MockToolExecutor> {
    let (event_tx, event_rx) = mpsc::channel(8);
    ConversationRuntime::new(
        initial.context.clone(),
        initial.state.clone(),
        DatabaseStorage::new(db.clone()),
        client,
        MockToolExecutor::new().with_tool("think", ToolOutput::success("actual tool output")),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        event_rx,
        event_tx,
        SseBroadcaster::new(32, 4),
    )
    .with_state_updated_at(initial.state_updated_at)
    .with_active_direct_turn(initial.active_direct_turn.take().map(|active| *active))
}

async fn canonical_counts(db: &Database) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM messages),
        (SELECT count(*) FROM conversation_goal_requests),
        (SELECT count(*) FROM conversation_goal_request_checkpoints),
        (SELECT count(*) FROM direct_turn_terminal_obligations)",
    )
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn conversation_goal_actual_provider_response_commits_checkpoint_and_keeps_same_execution() {
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "actual canonical answer",
        )]));
    let before = canonical_counts(&db).await;
    let active = rt.active_direct_turn.as_deref().unwrap().clone();
    let completion = dispatch_response(&mut rt).await;
    let LlmOutcome::Response {
        content,
        provider_replay,
        tool_calls,
        end_turn,
        usage,
        request_id,
    } = &completion.outcome
    else {
        panic!("scripted provider must produce its actual response");
    };
    let duplicate = LlmOutcome::Response {
        content: content.clone(),
        provider_replay: provider_replay.clone(),
        tool_calls: tool_calls.clone(),
        end_turn: *end_turn,
        usage: usage.clone(),
        request_id: request_id.clone(),
    };
    let request_id = completion.admitted.intent().id.as_str().to_owned();
    assert_eq!(
        canonical_counts(&db).await,
        before,
        "dispatch is not a checkpoint"
    );
    let (watch_tx, mut watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    rt.process_goal_request_completion(completion)
        .await
        .unwrap();
    let after = canonical_counts(&db).await;
    assert_eq!(after, (before.0 + 1, before.1, before.2 + 1, before.3));
    let message = db.get_message_by_id(&request_id).await.unwrap();
    assert!(matches!(message.content, MessageContent::Agent(_)));
    assert_eq!(rt.state, ConvState::Idle);
    assert!(watch_rx.has_changed().unwrap());
    assert_eq!(*watch_rx.borrow_and_update(), ConvState::Idle);
    let persisted = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (persisted.state, persisted.state_updated_at),
        (rt.state.clone(), rt.state_updated_at)
    );
    assert_eq!(persisted.message_count, 1);
    assert_eq!(rt.active_direct_turn.as_deref(), Some(&active));
    let (owns, terminal): (i64, Option<String>) =
        sqlx::query_as("SELECT owns_conversation,terminal_kind FROM durable_turns WHERE turn_id=?")
            .bind(receipt.execution().turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!((owns, terminal), (1, None));
    let mut publications = rt.broadcast_tx.subscribe();
    assert!(rt
        .process_outcome(EffectOutcome::Llm(duplicate))
        .await
        .is_err());
    assert!(publications.try_recv().is_err());
    assert_eq!(canonical_counts(&db).await, after);
    assert!(!watch_rx.has_changed().unwrap());
    assert_eq!(
        client.requests.lock().unwrap().len(),
        1,
        "producer is not activated by this checkpoint"
    );
}

#[tokio::test]
async fn conversation_goal_actual_tool_completion_checkpoints_persisted_tool_source_before_next_request(
) {
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response_with_replay(
            "tool-round-response",
            vec![ContentBlock::tool_use(
                "actual-think-call",
                "think",
                serde_json::json!({"thoughts":"checkpoint the actual round"}),
            )],
        ));
    let completion = dispatch_response(&mut rt).await;
    let first_request = completion.admitted.intent().id.as_str().to_owned();
    rt.process_goal_request_completion(completion)
        .await
        .unwrap();
    assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
    let persisted = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (persisted.state, persisted.state_updated_at),
        (rt.state.clone(), rt.state_updated_at)
    );
    let before = canonical_counts(&db).await;
    assert_eq!(before.2, 0);
    assert!(rt.pending_provider_replay_update.is_some());
    assert!(
        db.load_provider_replay_state(CONVERSATION)
            .await
            .unwrap()
            .is_none(),
        "real provider replay must not persist with intermediate ToolExecuting state"
    );
    let (generation, outcome) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        rt.tool_outcome_rx.recv(),
    )
    .await
    .expect("actual tool completion liveness")
    .unwrap();
    assert!(!rt.tool_outcome_is_stale(generation));
    rt.process_outcome(EffectOutcome::Tool(outcome))
        .await
        .unwrap();
    let counts = canonical_counts(&db).await;
    assert_eq!(counts.0, before.0 + 2);
    assert_eq!(counts.2, 1);
    assert_eq!(counts.3, 0);
    assert!(rt.pending_goal_request_checkpoint.is_none());
    assert!(db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_some());
    let checkpointed: String =
        sqlx::query_scalar("SELECT request_id FROM conversation_goal_request_checkpoints")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(checkpointed, first_request);
    let next: (i64, String) = sqlx::query_as("SELECT ordinal,predecessor_request_id FROM conversation_goal_requests ORDER BY ordinal DESC LIMIT 1")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(next, (2, first_request));
    rt.llm_task_handle.take().unwrap().await.unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].provider_replay.is_some(),
        "successor observes the committed replay bundle"
    );
}

#[tokio::test]
async fn conversation_goal_late_actual_response_after_stop_input_slot_or_reconstruction_has_zero_effects(
) {
    for fence in ["stop", "queue", "slot", "reconstruction"] {
        let (_directory, db, receipt, mut rt, client) = adopted().await;
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::text(
                "obsolete answer",
            )]));
        let completion = dispatch_response(&mut rt).await;
        match fence {
            "stop" => {
                use phoenix_db::conversation_goals::control::{ControlCommand, ControlRequest};
                db.apply_goal_control(
                    &ControlRequest {
                        owner: receipt.input().goal().owner.clone(),
                        principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
                        request_id: uuid::Uuid::new_v4(),
                        command: ControlCommand::Stop,
                        expected_control_version: 1,
                    },
                    GoalTimestampUs::parse(100).unwrap(),
                )
                .await
                .unwrap();
            }
            "queue" => {
                let entry = phoenix_core::domain::sm_event::SteerEntry {
                    message_id: "completion-race-input".into(),
                    text: "accepted input wins".into(),
                    origin: InputOrigin::UnknownHistorical,
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: None,
                    skill_invocation: None,
                };
                db.append_steering_entry(CONVERSATION, &entry, "completion-race-fingerprint")
                    .await
                    .unwrap();
            }
            "slot" => {
                db.update_conversation_state_at(
                    CONVERSATION,
                    &rt.state,
                    rt.state_updated_at + chrono::Duration::microseconds(1),
                )
                .await
                .unwrap();
            }
            "reconstruction" => {
                let active = rt.active_direct_turn.as_deref().unwrap();
                assert!(matches!(
                    db.workflow_repository()
                        .recover_goal_start_runtime(
                            &phoenix_db::workflow::GoalStartRecoveryInput::new(
                                CONVERSATION.to_owned(),
                                (active.turn_id, active.generation),
                                (rt.state.clone(), rt.state_updated_at),
                                Timestamp(100),
                            )
                        )
                        .await
                        .unwrap(),
                    phoenix_db::workflow::GoalStartRecoveryOutcome::Recovered(_)
                ));
            }
            _ => unreachable!(),
        }
        let before = canonical_counts(&db).await;
        let mut publications = rt.broadcast_tx.subscribe();
        assert!(
            rt.process_goal_request_completion(completion)
                .await
                .is_err(),
            "{fence}"
        );
        assert!(publications.try_recv().is_err(), "{fence}");
        assert_eq!(canonical_counts(&db).await, before, "{fence}");
    }
}

#[tokio::test]
async fn conversation_goal_summary_completion_cannot_fallback_to_ordinary_canonical_state_write() {
    let (_directory, db, _receipt, mut rt, _client) = adopted().await;
    let before = db.get_conversation(CONVERSATION).await.unwrap();
    let counts = canonical_counts(&db).await;
    let mut publications = rt.broadcast_tx.subscribe();
    let error = rt
        .process_event(Event::ContinuationResponse {
            operation_id: "unowned-summary".into(),
            summary: "must not write this as ordinary continuation".into(),
        })
        .await
        .unwrap_err();
    assert!(error.contains("Summary completion is held"), "{error}");
    let after = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (after.state, after.state_updated_at),
        (before.state, before.state_updated_at)
    );
    assert_eq!(canonical_counts(&db).await, counts);
    assert!(publications.try_recv().is_err());
}

#[tokio::test]
async fn conversation_goal_two_actual_response_operations_cannot_consume_each_others_receipt() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    for _ in 0..2 {
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::text(
                "same logical retry answer",
            )]));
    }
    let old = dispatch_response(&mut rt).await;
    let current = dispatch_response(&mut rt).await;
    assert_eq!(old.admitted.intent().id, current.admitted.intent().id);
    assert_ne!(old.generation, current.generation);
    let before = canonical_counts(&db).await;
    let mut publications = rt.broadcast_tx.subscribe();
    rt.process_goal_request_completion(old).await.unwrap();
    assert_eq!(canonical_counts(&db).await, before);
    assert!(publications.try_recv().is_err());
    assert!(rt.pending_goal_request_checkpoint.is_none());
    rt.process_goal_request_completion(current).await.unwrap();
    assert_eq!(
        canonical_counts(&db).await,
        (before.0 + 1, before.1, before.2 + 1, before.3)
    );
    assert_eq!(client.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn conversation_goal_actual_report_round_commits_mutation_and_checkpoint_once_then_holds_next_request(
) {
    let (_directory, db, receipt, mut rt, client) = adopted().await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::tool_use(
            "actual-report-complete",
            "goal_report",
            serde_json::json!({"status":"complete","reason":"actual admitted report"}),
        )]));
    let before = canonical_counts(&db).await;
    let completion = dispatch_response(&mut rt).await;
    let request_id = completion.admitted.intent().id.as_str().to_owned();
    let result = rt.process_goal_request_completion(completion).await;
    assert!(
        result.is_err(),
        "completed goal cannot admit another provider request"
    );
    let after = canonical_counts(&db).await;
    assert_eq!(after, (before.0 + 2, before.1, before.2 + 1, before.3));
    let goal = db
        .conversation_goal(&receipt.input().goal().owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(goal.version.get(), receipt.input().goal().version.get() + 1);
    assert!(matches!(
        goal.state,
        phoenix_core::domain::conversation_goal::GoalState::Complete(_)
    ));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    let assistant = messages
        .iter()
        .find(|message| message.message_id == request_id)
        .unwrap();
    let MessageContent::Agent(content) = &assistant.content else {
        panic!("canonical assistant required");
    };
    assert!(content.iter().any(|block| matches!(block,
        ContentBlock::ToolUse { id, .. } if id == "actual-report-complete")));
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message.content, MessageContent::Tool(_)))
            .count(),
        1
    );
    assert!(rt.pending_goal_request_checkpoint.is_none());
    assert_eq!(canonical_counts(&db).await, after);
}

#[tokio::test]
async fn conversation_goal_actual_tool_round_rollback_keeps_exact_source_and_no_publication_or_successor(
) {
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response_with_replay(
            "rollback-tool-response",
            vec![ContentBlock::tool_use(
                "rollback-think",
                "think",
                serde_json::json!({"thoughts":"whole round or nothing"}),
            )],
        ));
    let current = dispatch_response(&mut rt).await;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    rt.process_goal_request_completion(current).await.unwrap();
    let source = rt.state.clone();
    let source_stamp = rt.state_updated_at;
    assert!(matches!(source, ConvState::ToolExecuting { .. }));
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());
    let before = canonical_counts(&db).await;
    let (generation, outcome) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        rt.tool_outcome_rx.recv(),
    )
    .await
    .expect("real tool completion liveness")
    .unwrap();
    assert!(!rt.tool_outcome_is_stale(generation));
    sqlx::raw_sql(
        "CREATE TRIGGER reject_actual_tool_checkpoint BEFORE UPDATE OF state ON conversations
        BEGIN SELECT RAISE(ABORT,'injected actual tool checkpoint state failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(rt
        .process_outcome(EffectOutcome::Tool(outcome))
        .await
        .is_err());
    assert_eq!(canonical_counts(&db).await, before);
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (stored.state, stored.state_updated_at),
        (source.clone(), source_stamp)
    );
    let pending = rt.pending_goal_request_checkpoint.as_ref().unwrap();
    assert_eq!(pending.source, source);
    assert_eq!(pending.source_updated_at, source_stamp);
    assert!(!watch_rx.has_changed().unwrap());
    assert!(rt.pending_provider_replay_update.is_some());
    assert!(db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_none());
    assert!(publications.try_recv().is_err());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_stale_same_id_completion_preserves_newer_handle_and_checkpoint_receipt()
{
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    for _ in 0..3 {
        client
            .responses
            .lock()
            .unwrap()
            .push_back(provider_response(vec![ContentBlock::tool_use(
                "same-slot-think",
                "think",
                serde_json::json!({"thoughts":"real owned round"}),
            )]));
    }
    let mut old_error = dispatch_response(&mut rt).await;
    let old_response = dispatch_response(&mut rt).await;
    let current = dispatch_response(&mut rt).await;
    assert_eq!(old_error.admitted.intent().id, current.admitted.intent().id);
    assert_eq!(
        old_response.admitted.intent().id,
        current.admitted.intent().id
    );
    assert!(old_error.generation < old_response.generation);
    assert!(old_response.generation < current.generation);
    let handle_id = rt.llm_task_handle.as_ref().unwrap().id();
    let current_generation = current.generation;
    let before = canonical_counts(&db).await;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    old_error.outcome = LlmOutcome::NetworkError {
        message: "late actual task failure".into(),
    };
    rt.process_goal_request_completion(old_error).await.unwrap();
    assert_eq!(rt.llm_task_handle.as_ref().unwrap().id(), handle_id);
    assert!(rt.active_llm_attempt.is_some());
    assert!(rt.active_goal_request_dispatch.is_some());
    assert_eq!(canonical_counts(&db).await, before);
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());

    rt.process_goal_request_completion(current).await.unwrap();
    assert!(matches!(rt.state, ConvState::ToolExecuting { .. }));
    assert_eq!(
        rt.pending_goal_request_checkpoint
            .as_ref()
            .unwrap()
            .generation,
        current_generation
    );
    let tool_handle = rt.tool_task_handle.as_ref().unwrap().id();
    rt.process_goal_request_completion(old_response)
        .await
        .unwrap();
    assert_eq!(rt.tool_task_handle.as_ref().unwrap().id(), tool_handle);
    let pending = rt.pending_goal_request_checkpoint.as_ref().unwrap();
    assert_eq!(pending.generation, current_generation);
    assert_eq!(pending.source, rt.state);
    assert_eq!(pending.source_updated_at, rt.state_updated_at);
    assert_eq!(canonical_counts(&db).await, before);
    assert!(
        !watch_rx.has_changed().unwrap(),
        "tool source is durable but not published before canonical commit"
    );
    assert!(
        publications.try_recv().is_err(),
        "no assistant, tool preview or selected state before commit"
    );
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_pending_actual_round_refuses_next_dispatch_without_clearing_receipt() {
    let (_directory, db, _receipt, mut initial, client) = adopted().await;
    let mut rt = actual_tool_runtime(&mut initial, &db, client.clone());
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::tool_use(
            "checkpoint-first",
            "think",
            serde_json::json!({"thoughts":"no next dispatch yet"}),
        )]));
    let current = dispatch_response(&mut rt).await;
    let id = current.admitted.intent().id.clone();
    rt.process_goal_request_completion(current).await.unwrap();
    let before = canonical_counts(&db).await;
    assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
    assert_eq!(canonical_counts(&db).await, before);
    assert_eq!(
        rt.pending_goal_request_checkpoint
            .as_ref()
            .unwrap()
            .admitted
            .intent()
            .id,
        id
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    rt.abort_external_effects_in_memory();
}

#[tokio::test]
async fn conversation_goal_actual_summary_response_fails_closed_without_ordinary_continuation() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    let request = summary_state(&db, &mut rt, "unsupported-summary-checkpoint", 1).await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "actual summary",
        )]));
    send_summary(&mut rt, request).await;
    let summary = tokio::time::timeout(std::time::Duration::from_secs(30), rt.event_rx.recv())
        .await
        .expect("actual summary event liveness")
        .unwrap();
    assert!(
        matches!(&summary, Event::ContinuationResponse { summary, .. } if summary == "actual summary")
    );
    let before = canonical_counts(&db).await;
    let source = rt.state.clone();
    let source_stamp = rt.state_updated_at;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(source.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    assert!(rt.process_event(summary).await.is_err());
    assert_eq!(canonical_counts(&db).await, before);
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (stored.state, stored.state_updated_at),
        (source, source_stamp)
    );
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
}

#[tokio::test]
async fn conversation_goal_actual_response_rollback_publishes_neither_message_nor_selected_state() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    client
        .responses
        .lock()
        .unwrap()
        .push_back(provider_response(vec![ContentBlock::text(
            "rollback this answer",
        )]));
    let completion = dispatch_response(&mut rt).await;
    let source = rt.state.clone();
    let source_stamp = rt.state_updated_at;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(source.clone());
    rt.state_watcher = Some(watch_tx);
    let mut publications = rt.broadcast_tx.subscribe();
    let before = canonical_counts(&db).await;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_canonical_response_state BEFORE UPDATE OF state ON conversations
        BEGIN SELECT RAISE(ABORT,'injected actual response state failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(rt
        .process_goal_request_completion(completion)
        .await
        .is_err());
    assert_eq!(canonical_counts(&db).await, before);
    let persisted = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(
        (persisted.state, persisted.state_updated_at),
        (source.clone(), source_stamp)
    );
    assert_eq!(*watch_rx.borrow(), source);
    assert!(!watch_rx.has_changed().unwrap());
    assert!(publications.try_recv().is_err());
    assert_eq!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}
