use super::*;
use crate::goal_service::management::{
    testing::{fixture, Fixture},
    GoalManagementService,
};
use phoenix_core::domain::conversation_goal::{
    GoalManagementRequest, GoalPendingManagement, GoalSnapshot,
};
use phoenix_core::domain::provider_replay::{AnthropicPrivateBlock, ContentIndex};

fn create(fixture: &Fixture) -> GoalManagementRequest {
    GoalManagementRequest::Create {
        owner: fixture.target.clone(),
        expected_version: GoalVersion::INITIAL,
        objective: GoalText::parse("Canonical Coordinator provider objective").unwrap(),
    }
}

fn signed_management_response(request: &GoalManagementRequest) -> LlmResponse {
    let public = vec![ContentBlock::ToolUse {
        id: "provider-management-call".into(),
        name: "goal_manage".into(),
        input: serde_json::to_value(request).unwrap(),
    }];
    let mut response = journey_response(public.clone(), false);
    response.provider_replay = Some(ProviderReplayUpdate::Anthropic(
        AnthropicResponseSet::with_public_content(
            AnthropicResponseIdentity {
                response_id: "management-provider-response".into(),
                model: "claude-opus-5-5".into(),
            },
            public,
            vec![AnthropicPrivateBlock::Thinking {
                index: ContentIndex(0),
                thinking: "private management reasoning".into(),
                signature: "journey-opaque-signature".into(),
            }],
        )
        .unwrap(),
    ));
    response
}

fn provider_runtime(
    fixture: &Fixture,
    request: &GoalManagementRequest,
) -> (
    TestRuntime,
    Arc<RenderingAnthropicClient>,
    Arc<DeferredMcpCatalog>,
) {
    let client = Arc::new(RenderingAnthropicClient::scripted(vec![
        signed_management_response(request),
        journey_response(
            vec![ContentBlock::text("Management checkpoint committed.")],
            true,
        ),
    ]));
    let tools = Arc::new(DeferredMcpCatalog::new(true));
    let mut context = ConvContext::new(
        fixture.source.clone(),
        fixture.home.path().to_path_buf(),
        "claude-opus-5-5",
        200_000,
    );
    context.is_coordinator = true;
    let mut runtime = runtime_for_context(
        fixture.runtime.db().clone(),
        context,
        client.clone(),
        tools.clone(),
    )
    .with_active_direct_turn(Some(fixture.active.clone()));
    runtime.state = ConvState::LlmRequesting { attempt: 1 };
    runtime.goal_management = Some(Arc::new(GoalManagementService::new(Arc::downgrade(
        &fixture.runtime,
    ))));
    (runtime, client, tools)
}

async fn provider_preparation(runtime: &mut TestRuntime) -> Event {
    let events = runtime.execute_effect(Effect::RequestLlm).await.unwrap();
    for event in events {
        runtime.process_event(event).await.unwrap();
    }
    let (generation, outcome) =
        tokio::time::timeout(Duration::from_secs(10), runtime.llm_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
    runtime.active_llm_generation = generation;
    runtime
        .process_outcome(EffectOutcome::Llm(outcome))
        .await
        .unwrap();
    let event = runtime.event_rx.try_recv().unwrap();
    assert!(matches!(event, Event::GoalManagementPrepared { .. }));
    event
}

async fn assert_no_target_mutation(fixture: &Fixture) {
    let db = fixture.runtime.db();
    assert!(db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn conversation_goal_coordinator_provider_management_commits_only_at_its_canonical_checkpoint(
) {
    let fixture = fixture(true).await;
    let db = fixture.runtime.db();
    let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
    let prepared = provider_preparation(&mut runtime).await;
    assert_no_target_mutation(&fixture).await;
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 1);
    let Event::GoalManagementPrepared { management, .. } = &prepared else {
        panic!("management intent")
    };
    let call = management.source().clone();
    assert_eq!(call.transcript_id.as_str(), fixture.source);
    assert_eq!(management.generation(), fixture.active.generation);
    assert!(runtime.pending_provider_replay_update.is_some());
    assert!(db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap()
        .is_none());
    runtime.process_event(prepared).await.unwrap();
    assert!(db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap()
        .is_some());
    assert_checkpoint(&fixture, &call).await;
    assert!(tools.recorded_executions().is_empty());
    let (generation, outcome) =
        tokio::time::timeout(Duration::from_secs(10), runtime.llm_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
    runtime.active_llm_generation = generation;
    runtime
        .process_outcome(EffectOutcome::Llm(outcome))
        .await
        .unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(runtime.active_direct_turn.is_none());
    assert!(db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap()
        .is_none());
    let ownership: (Option<String>, i64) =
        sqlx::query_as("SELECT terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?")
            .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(ownership, (Some("Completed".into()), 0));
    assert_provider_declarations(&client);
    assert_canonical_delivery(&fixture, &client, &call).await;
    assert!(tools.recorded_executions().is_empty());
}

#[tokio::test]
async fn conversation_goal_coordinator_provider_management_rejects_missing_or_stale_runtime_authority(
) {
    for authority in ["missing", "stale", "ordinary"] {
        let fixture = fixture(authority != "ordinary").await;
        let db = fixture.runtime.db();
        let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
        match authority {
            "missing" => runtime.active_direct_turn = None,
            "stale" => runtime.active_direct_turn.as_mut().unwrap().generation += 1,
            _ => {}
        }
        let events = runtime.execute_effect(Effect::RequestLlm).await.unwrap();
        for event in events {
            runtime.process_event(event).await.unwrap();
        }
        let (generation, outcome) =
            tokio::time::timeout(Duration::from_secs(10), runtime.llm_outcome_rx.recv())
                .await
                .unwrap()
                .unwrap();
        runtime.active_llm_generation = generation;
        assert!(runtime
            .process_outcome(EffectOutcome::Llm(outcome))
            .await
            .is_err());
        assert!(runtime.event_rx.try_recv().is_err());
        assert_no_target_mutation(&fixture).await;
        assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 1);
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert!(tools.recorded_executions().is_empty());
    }
}

#[tokio::test]
async fn conversation_goal_coordinator_provider_management_rechecks_source_after_preparation() {
    let fixture = fixture(true).await;
    let db = fixture.runtime.db();
    let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
    let prepared = provider_preparation(&mut runtime).await;
    let terminal = phoenix_db::workflow::TerminalizeAuthoritativeTurnInput {
        command: phoenix_workflow::TurnCommand::Cancel {
            turn_id: fixture.active.turn_id,
            expected_generation: fixture.active.generation,
        },
        projection: Some(phoenix_db::workflow::DirectTurnConversationProjection {
            state: ConvState::Idle,
            state_updated_at: chrono::Utc::now(),
        }),
        provider_replay_settlement: ProviderReplaySettlement::for_conversation_state(
            &fixture.source,
            &ConvState::Idle,
        ),
    };
    db.workflow_repository()
        .terminalize_authoritative_turn(&terminal)
        .await
        .unwrap();
    assert!(runtime.process_event(prepared).await.is_err());
    assert_no_target_mutation(&fixture).await;
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 1);
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert!(tools.recorded_executions().is_empty());
}

#[tokio::test]
async fn conversation_goal_coordinator_provider_rejects_forged_or_host_stale_pending_management() {
    for changed in ["arguments", "tool_use", "runtime_generation"] {
        let fixture = fixture(true).await;
        let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
        let prepared = provider_preparation(&mut runtime).await;
        let Event::GoalManagementPrepared {
            management,
            duration_ms,
        } = prepared
        else {
            panic!("prepared")
        };
        let mut encoded = serde_json::to_value(&management).unwrap();
        match changed {
            "arguments" => encoded["request"]["objective"] = serde_json::json!("forged objective"),
            "tool_use" => encoded["source"]["tool_use_id"] = serde_json::json!("forged call"),
            _ => runtime.active_direct_turn.as_mut().unwrap().generation += 1,
        }
        let forged: GoalPendingManagement = serde_json::from_value(encoded).unwrap();
        assert!(runtime
            .process_event(Event::GoalManagementPrepared {
                management: forged,
                duration_ms
            })
            .await
            .is_err());
        assert_no_target_mutation(&fixture).await;
        assert_eq!(
            fixture
                .runtime
                .db()
                .get_messages(&fixture.source)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert!(tools.recorded_executions().is_empty());
    }
}

#[tokio::test]
async fn conversation_goal_management_declaration_stays_absent_without_parent_coordinator_installation(
) {
    for mode in ["default_off", "ordinary", "child"] {
        let fixture = fixture(true).await;
        let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
        match mode {
            "default_off" => runtime.goal_management = None,
            "ordinary" => runtime.context.is_coordinator = false,
            _ => runtime.context.is_sub_agent = true,
        }
        *client.responses.lock().unwrap() = std::collections::VecDeque::from([journey_response(
            vec![ContentBlock::text("No management capability.")],
            true,
        )]);
        let events = runtime.execute_effect(Effect::RequestLlm).await.unwrap();
        for event in events {
            runtime.process_event(event).await.unwrap();
        }
        let _ = tokio::time::timeout(Duration::from_secs(10), runtime.llm_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0]
            .tool_availability
            .declarations()
            .iter()
            .any(|tool| tool.name == "goal_manage"));
        drop(requests);
        assert_no_target_mutation(&fixture).await;
        assert!(tools.recorded_executions().is_empty());
    }
}

#[tokio::test]
async fn conversation_goal_coordinator_provider_management_rolls_back_replay_and_retries_without_another_tool_execution(
) {
    let fixture = fixture(true).await;
    let db = fixture.runtime.db();
    let (mut runtime, client, tools) = provider_runtime(&fixture, &create(&fixture));
    let prepared = provider_preparation(&mut runtime).await;
    assert!(runtime.pending_provider_replay_update.is_some());
    let before = serde_json::to_value(db.get_messages(&fixture.source).await.unwrap()).unwrap();
    sqlx::query("CREATE TRIGGER fail_provider_management BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'provider management state failed'); END")
        .execute(db.pool()).await.unwrap();
    assert!(runtime.process_event(prepared.clone()).await.is_err());
    assert_no_target_mutation(&fixture).await;
    assert_eq!(
        serde_json::to_value(db.get_messages(&fixture.source).await.unwrap()).unwrap(),
        before
    );
    assert!(db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap()
        .is_none());
    assert!(runtime.pending_provider_replay_update.is_some());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert!(tools.recorded_executions().is_empty());
    sqlx::query("DROP TRIGGER fail_provider_management")
        .execute(db.pool())
        .await
        .unwrap();
    runtime.process_event(prepared).await.unwrap();
    assert_eq!(
        db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 3);
    assert!(db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap()
        .is_some());
    let (generation, outcome) =
        tokio::time::timeout(Duration::from_secs(10), runtime.llm_outcome_rx.recv())
            .await
            .unwrap()
            .unwrap();
    runtime.active_llm_generation = generation;
    runtime
        .process_outcome(EffectOutcome::Llm(outcome))
        .await
        .unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    assert!(tools.recorded_executions().is_empty());
}

async fn assert_checkpoint(
    fixture: &Fixture,
    call: &phoenix_core::domain::conversation_goal::GoalAgentSource,
) {
    let db = fixture.runtime.db();
    let goal = db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(goal.version.get(), 1);
    assert!(matches!(goal.state, GoalState::Underway));
    let history = db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].mutation.attribution.requested_by,
        GoalActor::Coordinator(call.clone())
    );
    assert_eq!(
        history[0].mutation.attribution.applied_by,
        GoalActor::Coordinator(call.clone())
    );
    let messages = db.get_messages(&fixture.source).await.unwrap();
    let assistant = messages
        .iter()
        .find(|message| message.message_id == call.message_id.as_str())
        .unwrap();
    let result = messages
        .iter()
        .find(|message| {
            matches!(&message.content,
        MessageContent::Tool(content) if content.tool_use_id == call.tool_use_id.as_str())
        })
        .unwrap();
    assert_eq!(result.sequence_id, assistant.sequence_id + 1);
    assert_eq!(result.created_at, assistant.created_at);
    let MessageContent::Tool(content) = &result.content else {
        panic!("canonical result")
    };
    assert!(!content.is_error);
    assert_eq!(
        serde_json::from_str::<GoalSnapshot>(&content.content).unwrap(),
        goal
    );
}

#[tokio::test]
async fn conversation_goal_coordinator_provider_state_commands_use_the_tagged_disposition_contract()
{
    use phoenix_core::domain::conversation_goal::{GoalCommandDisposition, GoalDisposition};
    for disposition in [
        GoalCommandDisposition::Underway,
        GoalCommandDisposition::Blocked(GoalText::parse("blocked by dependency").unwrap()),
        GoalCommandDisposition::Complete(GoalText::parse("verified complete").unwrap()),
        GoalCommandDisposition::Exhausted(GoalText::parse("budget exhausted").unwrap()),
    ] {
        let fixture = fixture(true).await;
        let db = fixture.runtime.db();
        let seed = seed_target_goal(
            &fixture,
            matches!(disposition, GoalCommandDisposition::Underway),
        )
        .await;
        let request = GoalManagementRequest::State {
            owner: fixture.target.clone(),
            expected_version: seed.version,
            goal_id: seed.goal_id,
            disposition: disposition.clone(),
        };
        let (mut runtime, client, tools) = provider_runtime(&fixture, &request);
        let prepared = provider_preparation(&mut runtime).await;
        assert_eq!(
            db.conversation_goal(&fixture.target)
                .await
                .unwrap()
                .unwrap()
                .version,
            seed.version
        );
        runtime.process_event(prepared).await.unwrap();
        let goal = db
            .conversation_goal(&fixture.target)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(goal.version.get(), seed.version.get() + 1);
        assert_eq!(goal.state, GoalDisposition::from(disposition).state());
        assert_eq!(
            db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
                .await
                .unwrap()
                .len(),
            usize::try_from(seed.version.get() + 1).unwrap()
        );
        let (generation, outcome) =
            tokio::time::timeout(Duration::from_secs(10), runtime.llm_outcome_rx.recv())
                .await
                .unwrap()
                .unwrap();
        runtime.active_llm_generation = generation;
        runtime
            .process_outcome(EffectOutcome::Llm(outcome))
            .await
            .unwrap();
        assert!(matches!(runtime.state, ConvState::Idle));
        assert_eq!(client.requests.lock().unwrap().len(), 2);
        assert!(tools.recorded_executions().is_empty());
    }
}

async fn seed_target_goal(fixture: &Fixture, blocked: bool) -> GoalSnapshot {
    use phoenix_db::conversation_goals::user_commands::{
        GoalUserChange, GoalUserCommand, GoalUserResult,
    };
    let db = fixture.runtime.db();
    let command = GoalUserCommand {
        owner: fixture.target.clone(),
        principal_id: uuid::Uuid::parse_str(&db.instance_id().await.unwrap().to_string()).unwrap(),
        request_id: uuid::Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: GoalUserChange::Create {
            objective: GoalText::parse("Existing objective").unwrap(),
        },
    };
    let receipt = db
        .apply_user_goal_command(
            &command,
            "management-target",
            GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros()).unwrap(),
        )
        .await
        .unwrap();
    let GoalUserResult::Accepted { entry } = receipt.result else {
        panic!("seed goal accepted")
    };
    if !blocked {
        return entry.snapshot;
    }
    let blocked = GoalUserCommand {
        request_id: uuid::Uuid::new_v4(),
        expected_version: entry.snapshot.version,
        change: GoalUserChange::State {
            goal_id: entry.snapshot.goal_id,
            disposition: phoenix_core::domain::conversation_goal::GoalDisposition::Blocked(
                GoalText::parse("seed dependency").unwrap(),
            ),
        },
        ..command
    };
    let receipt = db
        .apply_user_goal_command(
            &blocked,
            "management-target",
            GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros()).unwrap(),
        )
        .await
        .unwrap();
    let GoalUserResult::Accepted { entry } = receipt.result else {
        panic!("blocked seed accepted")
    };
    entry.snapshot
}

fn assert_provider_declarations(client: &RenderingAnthropicClient) {
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(
            request
                .tool_availability
                .declarations()
                .iter()
                .filter(|tool| tool.name == "goal_manage")
                .count(),
            1
        );
        assert!(!request
            .tool_availability
            .declarations()
            .iter()
            .any(|tool| tool.name == "goal_report"));
    }
}

async fn assert_canonical_delivery(
    fixture: &Fixture,
    client: &RenderingAnthropicClient,
    call: &phoenix_core::domain::conversation_goal::GoalAgentSource,
) {
    let persisted = fixture
        .runtime
        .db()
        .get_messages(&fixture.source)
        .await
        .unwrap();
    let canonical: Vec<_> = persisted
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Tool(result) if result.tool_use_id == call.tool_use_id.as_str() => {
                Some(result)
            }
            _ => None,
        })
        .collect();
    assert_eq!(canonical.len(), 1);
    let wires = client.wires.lock().unwrap();
    let wire = &wires[1];
    assert_eq!(wire["model"], "claude-opus-5-5");
    let messages = wire["messages"].as_array().unwrap();
    let tool_results: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| {
            block["type"] == "tool_result" && block["tool_use_id"] == call.tool_use_id.as_str()
        })
        .collect();
    assert_eq!(tool_results.len(), 1);
    assert_eq!(tool_results[0]["content"], canonical[0].content);
    assert_eq!(tool_results[0]["is_error"], false);
    assert!(!canonical[0].is_error);
    let assistant = messages
        .iter()
        .find(|message| {
            message["role"] == "assistant"
                && message["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "tool_use" && block["id"] == call.tool_use_id.as_str()
                    })
                })
        })
        .unwrap();
    let blocks = assistant["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "private management reasoning");
    assert_eq!(blocks[0]["signature"], "journey-opaque-signature");
    assert_eq!(blocks[1]["type"], "tool_use");
    assert_eq!(blocks[1]["id"], call.tool_use_id.as_str());
}
