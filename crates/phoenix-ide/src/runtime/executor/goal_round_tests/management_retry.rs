use super::*;
use phoenix_core::domain::db_schema::Message;
use phoenix_core::domain::responses_replay::ResponsesResponseSet;

#[derive(Clone, Copy, Debug)]
enum Shape {
    Ordinary,
    Fork,
}

#[derive(Clone, Copy, Debug)]
enum Replay {
    Absent,
    Anthropic,
    Responses,
}

async fn checkpoint(fixture: &Fixture, shape: Shape) -> CheckpointData {
    let request = create(fixture);
    let management = fixture
        .service
        .prepare(
            &fixture.source,
            &fixture.active,
            "management-retry-assistant",
            "management-retry-call",
            request.clone(),
        )
        .await
        .unwrap();
    let mut content = vec![ContentBlock::ToolUse {
        id: "management-retry-call".into(),
        name: "goal_manage".into(),
        input: serde_json::to_value(request).unwrap(),
    }];
    let mut results = vec![ToolRoundResult::PendingGoalManagement {
        management,
        duration_ms: 17,
    }];
    if matches!(shape, Shape::Fork) {
        content.push(ContentBlock::ToolUse {
            id: "management-retry-proposal-call".into(),
            name: "propose_task".into(),
            input: serde_json::json!({"task_file":"tasks/management-retry.md"}),
        });
        results.push(
            ToolResult::success_with_display(
                "management-retry-proposal-call".into(),
                "Fork proposal recorded — pending your review; continue your work".into(),
                Some(serde_json::json!({"fork_proposal_id":"management-retry-proposal"})),
            )
            .into(),
        );
    }
    CheckpointData::tool_round(
        AssistantMessage::new("management-retry-assistant".into(), content, None, None),
        results,
    )
    .unwrap()
}

fn effect(shape: Shape, checkpoint: CheckpointData) -> Effect {
    match shape {
        Shape::Ordinary => Effect::PersistCheckpoint { data: checkpoint },
        Shape::Fork => Effect::PersistForkProposal {
            proposal_id: "management-retry-proposal".into(),
            task_file: "tasks/management-retry.md".into(),
            title: "Checkpoint retry".into(),
            priority: phoenix_core::task_source::Priority::P2,
            body: "Retained proposal body\n".into(),
            checkpoint,
        },
    }
}

fn private_replay(replay: Replay, checkpoint: &CheckpointData) -> Option<ProviderReplayUpdate> {
    let CheckpointData::ToolRound {
        assistant_message, ..
    } = checkpoint;
    let blocks = &assistant_message.content;
    match replay {
        Replay::Absent => None,
        Replay::Anthropic => Some(ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: "management-runtime-retry-response".into(),
                    model: "claude-opus-5-5".into(),
                },
                blocks.clone(),
                vec![AnthropicPrivateBlock::Thinking {
                    index: ContentIndex(0),
                    thinking: "private management runtime retry".into(),
                    signature: "opaque-management-runtime-retry".into(),
                }],
            )
            .unwrap()
            .with_owner_message_id(assistant_message.message_id.clone()),
        )),
        Replay::Responses => {
            let mut output_items = vec![serde_json::json!({
                "type":"reasoning", "id":"management-runtime-retry-reasoning", "summary":[],
                "encrypted_content":"opaque-runtime-retry-reasoning"
            })];
            output_items.extend(blocks.iter().filter_map(|block| {
                let ContentBlock::ToolUse { id, name, input } = block else {
                    return None;
                };
                Some(serde_json::json!({
                    "type":"function_call", "id":format!("fc-{id}"), "call_id":id,
                    "name":name, "arguments":serde_json::to_string(input).unwrap()
                }))
            }));
            Some(ProviderReplayUpdate::Responses(ResponsesResponseSet {
                response_id: "management-runtime-retry-response".into(),
                model: "gpt-6-sol".into(),
                owner_message_id: assistant_message.message_id.clone(),
                public_content: blocks.clone(),
                output_items,
            }))
        }
    }
}

async fn snapshot(fixture: &Fixture) -> serde_json::Value {
    let db = fixture.runtime.db();
    let source = db.get_conversation(&fixture.source).await.unwrap();
    let turn: (i64, Option<String>, i64) = sqlx::query_as(
        "SELECT generation,terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?",
    )
    .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
    .fetch_one(db.pool())
    .await
    .unwrap();
    serde_json::json!({
        "state":source.state, "state_updated_at":source.state_updated_at,
        "turn":turn,
        "messages":db.get_messages(&fixture.source).await.unwrap(),
        "history":db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL).await.unwrap(),
        "proposals":db.list_fork_proposals_for_origin(&fixture.source).await.unwrap(),
        "anthropic":db.load_provider_replay_state(&fixture.source).await.unwrap(),
        "responses":db.load_responses_replay_state(&fixture.source).await.unwrap(),
    })
}

async fn assert_private_replay(fixture: &Fixture, update: Option<&ProviderReplayUpdate>) {
    let db = fixture.runtime.db();
    let anthropic = db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap();
    let responses = db
        .load_responses_replay_state(&fixture.source)
        .await
        .unwrap();
    match update {
        None | Some(ProviderReplayUpdate::Clear) => {
            assert!(anthropic.is_none());
            assert!(responses.is_empty());
        }
        Some(ProviderReplayUpdate::Anthropic(set)) => {
            assert_eq!(anthropic.unwrap().response_sets, vec![set.clone()]);
            assert!(responses.is_empty());
        }
        Some(ProviderReplayUpdate::Responses(set)) => {
            assert!(anthropic.is_none());
            assert_eq!(responses, vec![set.clone()]);
        }
    }
}

fn published_messages(receiver: &mut tokio::sync::broadcast::Receiver<SseEvent>) -> Vec<Message> {
    let mut messages = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                if let SseEvent::Message { message, .. } = event {
                    messages.push(message);
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => return messages,
            Err(error) => panic!("checkpoint publication channel failed: {error}"),
        }
    }
}

async fn apply(
    runtime: &mut TestRuntime,
    shape: Shape,
    checkpoint: CheckpointData,
) -> Result<(), String> {
    runtime
        .apply_transition_result(
            crate::state_machine::transition::TransitionResult::new(ConvState::LlmRequesting {
                attempt: 2,
            })
            .with_effect(effect(shape, checkpoint)),
        )
        .await
        .map(|_| ())
}

#[tokio::test]
async fn conversation_goal_runtime_management_retry_reuses_durable_sequence_for_both_checkpoint_owners(
) {
    for shape in [Shape::Ordinary, Shape::Fork] {
        for replay in [Replay::Absent, Replay::Anthropic, Replay::Responses] {
            for refused in [false, true] {
                retained_retry(shape, replay, refused).await;
            }
        }
    }
}

async fn retained_retry(shape: Shape, replay: Replay, refused: bool) {
    let fixture = fixture(true).await;
    let checkpoint = checkpoint(&fixture, shape).await;
    let update = private_replay(replay, &checkpoint);
    let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
    runtime.pending_provider_replay_update = update.clone();
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(runtime.state.clone());
    runtime.state_watcher = Some(watch_tx);
    let mut receiver = runtime.broadcast_tx.subscribe();
    let guard = if refused {
        Some(
            fixture
                .runtime
                .lock_product_message_admission(fixture.target.as_str())
                .await,
        )
    } else {
        None
    };
    apply(&mut runtime, shape, checkpoint.clone())
        .await
        .unwrap();
    drop(guard);
    let first = published_messages(&mut receiver);
    assert_committed(&fixture, shape, refused, &first).await;
    assert_private_replay(&fixture, update.as_ref()).await;
    assert!(runtime.pending_provider_replay_update.is_none());
    assert_eq!(*watch_rx.borrow(), runtime.state);
    let db = fixture.runtime.db();
    let later = ConvState::LlmRequesting { attempt: 7 };
    db.update_conversation_state(&fixture.source, &later)
        .await
        .unwrap();
    let durable = db.get_conversation(&fixture.source).await.unwrap();
    let before = snapshot(&fixture).await;
    let reserved_floor = runtime.broadcast_tx.current_seq();
    runtime.pending_provider_replay_update = update;
    let _product = fixture
        .runtime
        .lock_product_message_admission(fixture.target.as_str())
        .await;
    let _transcript = fixture
        .runtime
        .lock_message_acceptance("management-target")
        .await;
    apply(&mut runtime, shape, checkpoint).await.unwrap();
    assert_eq!(snapshot(&fixture).await, before);
    assert_eq!(
        serde_json::to_value(published_messages(&mut receiver)).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert!(runtime.broadcast_tx.current_seq() > reserved_floor);
    assert_eq!(runtime.state, later);
    assert_eq!(runtime.state_updated_at, durable.state_updated_at);
    assert_eq!(*watch_rx.borrow(), later);
    assert!(runtime.pending_provider_replay_update.is_none());
    assert!(matches!(
        runtime.local_terminal_authority,
        LocalTerminalAuthority::Classified
    ));
    assert_eq!(runtime.active_direct_turn.as_deref(), Some(&fixture.active));
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(tools.recorded_executions().is_empty());
}

async fn assert_committed(fixture: &Fixture, shape: Shape, refused: bool, messages: &[Message]) {
    let db = fixture.runtime.db();
    assert_eq!(
        messages.len(),
        if matches!(shape, Shape::Fork) { 3 } else { 2 }
    );
    let persisted = db.get_messages(&fixture.source).await.unwrap();
    for message in messages {
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            serde_json::to_value(
                persisted
                    .iter()
                    .find(|row| row.message_id == message.message_id)
                    .unwrap()
            )
            .unwrap()
        );
    }
    let MessageContent::Tool(result) = &messages[1].content else {
        panic!("canonical management result")
    };
    assert_eq!(result.is_error, refused);
    assert_eq!(messages[1].sequence_id, messages[0].sequence_id + 1);
    assert_eq!(
        db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        usize::from(!refused)
    );
    let proposals = db
        .list_fork_proposals_for_origin(&fixture.source)
        .await
        .unwrap();
    assert_eq!(proposals.len(), usize::from(matches!(shape, Shape::Fork)));
    if let Some(proposal) = proposals.first() {
        assert_eq!(proposal.id, "management-retry-proposal");
        assert_eq!(proposal.body, "Retained proposal body\n");
        assert_eq!(
            messages[2].display_data.as_ref().unwrap()["fork_proposal_id"],
            proposal.id
        );
    }
}

#[tokio::test]
async fn conversation_goal_runtime_management_sequence_reconciliation_still_rejects_changed_assistant_payload(
) {
    for shape in [Shape::Ordinary, Shape::Fork] {
        let fixture = fixture(true).await;
        let checkpoint = checkpoint(&fixture, shape).await;
        let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
        let mut receiver = runtime.broadcast_tx.subscribe();
        apply(&mut runtime, shape, checkpoint.clone())
            .await
            .unwrap();
        let _ = published_messages(&mut receiver);
        let before = snapshot(&fixture).await;
        let CheckpointData::ToolRound {
            mut assistant_message,
            tool_results,
        } = checkpoint;
        assistant_message
            .content
            .push(ContentBlock::text("Changed retry payload"));
        let changed = CheckpointData::tool_round(assistant_message, tool_results).unwrap();
        assert!(apply(&mut runtime, shape, changed).await.is_err());
        assert_eq!(snapshot(&fixture).await, before);
        assert!(published_messages(&mut receiver).is_empty());
        assert!(matches!(
            runtime.local_terminal_authority,
            LocalTerminalAuthority::Fatal
        ));
        assert!(client.requests.lock().unwrap().is_empty());
        assert!(tools.recorded_executions().is_empty());
    }
}
