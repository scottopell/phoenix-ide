use super::{
    mpsc, Arc, BrowserSessionManager, ContentBlock, ConvContext, ConvState, ConversationRuntime,
    Effect, Event, MessageContent, ModelRegistry, Path, SseBroadcaster, ToolRoundResult, Utc,
};
use crate::runtime::testing::{MockLlmClient, MockToolExecutor};
use crate::runtime::traits::{ActiveDirectTurn, DatabaseStorage};
use phoenix_core::domain::db_schema::{tool_result_transcript_content, ToolContent, ToolResult};
use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion;
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnIdentity,
};
use phoenix_core::domain::sm_state::AssistantMessage;
use phoenix_core::domain::sm_state::{ToolCall, ToolInput};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp, TurnOutcome,
};

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
            accepted_at: Timestamp(chrono::Utc::now().timestamp_micros().try_into().unwrap()),
        })
        .await
        .established()
        .unwrap()
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("fresh direct turn expected")
    };
    ActiveDirectTurn {
        turn_id,
        generation: 0,
    }
}

#[derive(Clone, Copy)]
enum RoundEnding {
    Complete,
    CancelCurrent,
    CancelFollowing,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // Shared real-runtime fixture owns admission, checkpoint, and terminal settlement assertions.
async fn terminal_self_tool(
    db: &crate::db::Database,
    transcript: &str,
    cwd: &Path,
    key: &str,
    name: &str,
    input: serde_json::Value,
    ending: RoundEnding,
) -> ToolContent {
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
    let following = matches!(ending, RoundEnding::CancelFollowing).then(|| {
        ToolCall::new(
            format!("{key}-following"),
            ToolInput::Unknown {
                name: "think".into(),
                input: serde_json::json!({"thoughts":"next tool"}),
            },
        )
    });
    let mut blocks = vec![ContentBlock::ToolUse {
        id: tool.id.clone(),
        name: name.into(),
        input,
    }];
    if let Some(following) = &following {
        blocks.push(ContentBlock::ToolUse {
            id: following.id.clone(),
            name: following.name().into(),
            input: following.input.to_value(),
        });
    }
    let assistant = AssistantMessage::new(format!("assistant-{key}"), blocks, None, None);
    let initial = ConvState::ToolExecuting {
        current_tool: tool.clone(),
        remaining_tools: following.clone().into_iter().collect(),
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: assistant,
    };
    let initial_timestamp = Utc::now();
    db.update_conversation_state_at(transcript, &initial, initial_timestamp)
        .await
        .unwrap();
    let (tx, rx) = mpsc::channel(16);
    let tools = Arc::new(MockToolExecutor::new());
    let llm = Arc::new(MockLlmClient::new("test-model"));
    llm.queue_response(phoenix_llm::LlmResponse {
        provider_replay: None,
        content: vec![ContentBlock::text("done")],
        end_turn: true,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    });
    let last_sequence = db.get_last_sequence_id(transcript).await.unwrap();
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
        SseBroadcaster::new(128, last_sequence),
    )
    .with_state_updated_at(initial_timestamp);
    runtime.active_direct_turn = Some(Box::new(active));
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
            result: ToolRoundResult::PendingSelfGoal { .. },
            ..
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
    let completion = if let Some(following) = &following {
        let next =
            crate::state_machine::transition(&runtime.state, &runtime.context, event).unwrap();
        let ConvState::ToolExecuting {
            current_tool,
            completed_results,
            ..
        } = &next.new_state
        else {
            panic!("pending self-goal must advance to the following tool");
        };
        assert_eq!(current_tool.id, following.id);
        assert!(matches!(
            completed_results.as_slice(),
            [ToolRoundResult::PendingSelfGoal { .. }]
        ));
        runtime.state = next.new_state;
        runtime.state_updated_at = Utc::now();
        // The following tool is staged without dispatching its executor.
        db.update_conversation_state_at(transcript, &runtime.state, runtime.state_updated_at)
            .await
            .unwrap();
        Event::ToolAborted {
            tool_use_id: following.id.clone(),
        }
    } else {
        event
    };
    let cancelled = !matches!(ending, RoundEnding::Complete);
    if cancelled {
        runtime
            .process_event(Event::UserCancel {
                reason: None,
                cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
            })
            .await
            .unwrap();
        assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
    }
    runtime.process_event(completion).await.unwrap();
    if !cancelled {
        assert!(matches!(runtime.state, ConvState::LlmRequesting { .. }));
        runtime
            .llm_task_handle
            .take()
            .expect("next provider request is owned")
            .await
            .unwrap();
        while let Ok(event) = runtime.event_rx.try_recv() {
            runtime.process_event(event).await.unwrap();
        }
    }
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
    assert_eq!(llm.recorded_requests().len(), usize::from(!cancelled));
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
    if cancelled {
        assert_eq!(db.conversation_goal(&owner).await.unwrap(), goal_before);
        assert_eq!(
            db.auto_continue_on_context_exhaustion(&owner)
                .await
                .unwrap(),
            preference_before
        );
    }
    if let Some(following) = &following {
        let following_result = messages
            .iter()
            .find_map(|message| match &message.content {
                MessageContent::Tool(result) if result.tool_use_id == following.id => Some(result),
                _ => None,
            })
            .expect("following tool cancellation is durable in the same round");
        assert_eq!(
            following_result,
            &tool_result_transcript_content(&ToolResult::cancelled(
                following.id.clone(),
                "Cancelled by user"
            ))
        );
    }
    let result = messages
        .iter()
        .find_map(|message| match &message.content {
            MessageContent::Tool(result) if result.tool_use_id == key => Some(result),
            _ => None,
        })
        .expect("checkpoint publishes a finalized provider-visible tool result");
    result.clone()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real runtime journey exercises distinct enable/create calls and terminal settlement.
async fn database_runtime_self_goal_tools_finalize_before_terminal_settlement_without_rebinding() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
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
    db.set_auto_continue_on_context_exhaustion(&owner, AutoContinueOnContextExhaustion::Disabled)
        .await
        .unwrap();
    let cancelled = Box::pin(terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "cancelled-enable",
        "enable_auto_continuation",
        serde_json::json!({}),
        RoundEnding::CancelCurrent,
    ))
    .await;
    assert_eq!(
        cancelled,
        tool_result_transcript_content(&ToolResult::cancelled(
            "cancelled-enable".into(),
            "Cancelled by user"
        ))
    );
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&owner)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled
    );
    let pending_cancelled = Box::pin(terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "pending-enable",
        "enable_auto_continuation",
        serde_json::json!({}),
        RoundEnding::CancelFollowing,
    ))
    .await;
    let pending_cancelled: serde_json::Value =
        serde_json::from_str(&pending_cancelled.content).unwrap();
    assert_eq!(pending_cancelled["status"], "refused");
    assert_eq!(
        pending_cancelled["reason"],
        "conversation_stopped_or_paused"
    );
    let refused = Box::pin(terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "off",
        "create_goal",
        serde_json::json!({"objective":"finish approved work"}),
        RoundEnding::Complete,
    ))
    .await;
    let refused: serde_json::Value = serde_json::from_str(&refused.content).unwrap();
    assert_eq!(refused["reason"], "auto_continuation_disabled");
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    let enabled = Box::pin(terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "enable",
        "enable_auto_continuation",
        serde_json::json!({}),
        RoundEnding::Complete,
    ))
    .await;
    let enabled: serde_json::Value = serde_json::from_str(&enabled.content).unwrap();
    assert_eq!(enabled["status"], "auto_continuation_enabled");
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    let created = Box::pin(terminal_self_tool(
        &db,
        transcript,
        cwd.path(),
        "create",
        "create_goal",
        serde_json::json!({"objective":"finish approved work"}),
        RoundEnding::Complete,
    ))
    .await;
    let created: serde_json::Value = serde_json::from_str(&created.content).unwrap();
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
