use super::*;
use crate::runtime::testing::{MockLlmClient, MockToolExecutor};
use crate::runtime::traits::{ActiveDirectTurn, DatabaseStorage};
use phoenix_core::domain::conversation_goal::{GoalRequestLimit, GoalTimestampUs};
use phoenix_core::domain::goal_control::{ControlCommand, ControlRequest};
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnIdentity,
};
use phoenix_core::domain::sm_state::AssistantMessage;
use phoenix_core::domain::sm_state::{ToolCall, ToolInput};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp, TurnOutcome,
};
use uuid::Uuid;

async fn admit(db: &crate::db::Database, transcript: &str, key: &str) -> ActiveDirectTurn {
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: key.into(),
            origin: phoenix_core::domain::db_schema::InputOrigin::UserApi,
            text: "continue approved work".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "continue approved work".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    let repository = db.workflow_repository();
    let accepted = repository
        .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(key).unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(transcript.into()),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(chrono::Utc::now().timestamp_micros()),
        })
        .await
        .established()
        .unwrap()
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("fresh direct turn expected")
    };
    ActiveDirectTurn {
        workflow_id: repository
            .workflow_id_for_turn(turn_id)
            .await
            .unwrap()
            .unwrap(),
        turn_id,
        generation: 0,
        first_user_message_id: Some(key.into()),
    }
}

#[allow(clippy::too_many_lines)] // Real producer, canonical checkpoint, and terminal projection share this fixture.
async fn terminal_self_tool(
    db: &crate::db::Database,
    transcript: &str,
    cwd: &Path,
    key: &str,
    name: &str,
    input: serde_json::Value,
) -> serde_json::Value {
    let active = admit(db, transcript, key).await;
    let turn_id = i64::try_from(active.turn_id.0).unwrap();
    let binding_before: (Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT observed_version,goal_id FROM conversation_goal_bindings WHERE turn_id=?",
    )
    .bind(turn_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let tool = ToolCall::new(
        key,
        ToolInput::Unknown {
            name: name.into(),
            input: input.clone(),
        },
    );
    let assistant = AssistantMessage::new(
        format!("assistant-{key}"),
        vec![ContentBlock::ToolUse {
            id: tool.id.clone(),
            name: name.into(),
            input,
        }],
        None,
        None,
    );
    let initial = ConvState::CancellingTool {
        executing_tool: tool.clone(),
        remaining_tools: vec![],
        completed_results: vec![],
        assistant_message: assistant,
    };
    let initial_timestamp = Utc::now();
    db.update_conversation_state_at(transcript, &initial, initial_timestamp)
        .await
        .unwrap();
    let (tx, rx) = mpsc::channel(16);
    let tools = Arc::new(MockToolExecutor::new());
    let llm = Arc::new(MockLlmClient::new("test-model"));
    let mut runtime = ConversationRuntime::new(
        ConvContext::new(transcript, cwd.to_path_buf(), "test-model", 200_000, 16_384),
        initial,
        DatabaseStorage::new(db.clone()),
        llm.clone(),
        tools.clone(),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        rx,
        tx,
        SseBroadcaster::new(128, 0),
    )
    .with_state_updated_at(initial_timestamp);
    runtime.active_direct_turn = Some(active);
    let (_, declarations) = runtime.prepare_overhead_without_recall().await.unwrap();
    for expected in ["create_goal", "enable_auto_continuation"] {
        assert!(declarations.iter().any(|tool| tool.name == expected));
    }
    let callable = declarations.iter().map(|tool| tool.name.clone()).collect();
    db.prepare_tool_availability(
        transcript,
        "test-route",
        None,
        &declarations,
        &callable,
        &[],
        &[],
    )
    .await
    .unwrap();
    let preference_before = db
        .auto_continue_on_context_exhaustion(
            &db.get_conversation(transcript)
                .await
                .unwrap()
                .product_conversation_id,
        )
        .await
        .unwrap();
    let goal_before = db
        .conversation_goal(
            &db.get_conversation(transcript)
                .await
                .unwrap()
                .product_conversation_id,
        )
        .await
        .unwrap();
    let event = runtime
        .execute_effect(Effect::ExecuteTool { tool })
        .await
        .unwrap()
        .expect("runtime-owned tool produces a synchronous round completion");
    assert!(matches!(
        &event,
        Event::ToolRoundComplete {
            result: ToolRoundResult::PendingSelfGoal { .. }
        }
    ));
    let owner = db
        .get_conversation(transcript)
        .await
        .unwrap()
        .product_conversation_id;
    assert_eq!(db.conversation_goal(&owner).await.unwrap(), goal_before);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&owner)
            .await
            .unwrap(),
        preference_before
    );
    assert!(db
        .get_messages(transcript)
        .await
        .unwrap()
        .iter()
        .all(|message| message.message_id != format!("assistant-{key}")));
    runtime.process_event(event).await.unwrap();
    assert!(matches!(runtime.state, ConvState::Idle));
    assert!(
        runtime.active_direct_turn.is_none(),
        "terminal settlement must finish"
    );
    let persisted = db.get_conversation(transcript).await.unwrap();
    assert!(matches!(persisted.state, ConvState::Idle));
    assert_eq!(persisted.state_updated_at, runtime.state_updated_at);
    assert!(runtime.terminal_transition_retry.is_none());
    assert!(tools.recorded_executions().is_empty());
    assert!(
        llm.recorded_requests().is_empty(),
        "self tools do not dispatch another execution"
    );
    let binding_after: (Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT observed_version,goal_id FROM conversation_goal_bindings WHERE turn_id=?",
    )
    .bind(turn_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        binding_after, binding_before,
        "the current turn is never rebound"
    );
    let messages = db.get_messages(transcript).await.unwrap();
    let result = messages
        .iter()
        .find_map(|message| match &message.content {
            MessageContent::Tool(result) if result.tool_use_id == key => Some(result),
            _ => None,
        })
        .expect("checkpoint publishes a finalized provider-visible tool result");
    serde_json::from_str(&result.content).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real runtime journey exercises distinct enable/create calls and terminal settlement.
async fn database_runtime_self_goal_tools_finalize_before_terminal_settlement_without_rebinding() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let db = db
        .install_goal_controls_for_tests()
        .await
        .unwrap()
        .install_goal_execution_for_tests(GoalRequestLimit::parse(2).unwrap())
        .await
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let transcript = "runtime-self-goal";
    db.create_conversation(
        transcript,
        transcript,
        cwd.path().to_str().unwrap(),
        true,
        None,
        None,
    )
    .await
    .unwrap();
    let owner = db
        .get_conversation(transcript)
        .await
        .unwrap()
        .product_conversation_id;
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        phoenix_db::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 0,
        },
        GoalTimestampUs::parse(Utc::now().timestamp_micros()).unwrap(),
        || panic!("lost control authority"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let refused = terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "off",
        "create_goal",
        serde_json::json!({"objective":"finish approved work"}),
    )
    .await;
    assert_eq!(refused["reason"], "auto_continuation_disabled");
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    let enabled = terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "enable",
        "enable_auto_continuation",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(enabled["status"], "auto_continuation_enabled");
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    let created = terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "create",
        "create_goal",
        serde_json::json!({"objective":"finish approved work"}),
    )
    .await;
    assert_eq!(created["status"], "goal_created");
    assert_eq!(
        db.conversation_goal(&owner)
            .await
            .unwrap()
            .unwrap()
            .objective()
            .as_str(),
        "finish approved work"
    );
}
