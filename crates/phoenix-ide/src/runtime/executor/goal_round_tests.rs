use super::*;
use phoenix_core::domain::provider_replay::ProviderReplaySettlement;
use phoenix_core::domain::{
    conversation_goal::{
        GoalActor, GoalAttribution, GoalChange, GoalExecution, GoalId, GoalMutation,
        GoalPendingReport, GoalReportCall, GoalReportOutcome, GoalState, GoalText, GoalTimestampUs,
        GoalVersion, ToolRoundResult,
    },
    db_schema::{MessageType, NonEmptyString},
    sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    },
    sm_state::{AssistantMessage, ThinkInput},
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
    TurnAuthorityId, TurnOutcome,
};

fn text(value: &str) -> GoalText {
    GoalText::parse(value).unwrap()
}

fn goal_report_round_state(report: &GoalPendingReport) -> (AssistantMessage, ConvState) {
    let assistant = AssistantMessage::new(
        "runtime-assistant".into(),
        vec![
            ContentBlock::ToolUse {
                id: "report".into(),
                name: "goal_report".into(),
                input: serde_json::to_value(report.outcome()).unwrap(),
            },
            ContentBlock::ToolUse {
                id: "other".into(),
                name: "think".into(),
                input: serde_json::json!({"thoughts":"retain normal tool output"}),
            },
        ],
        None,
        None,
    );
    let state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            "other",
            ToolInput::Think(ThinkInput {
                thoughts: "retain normal tool output".into(),
            }),
        ),
        remaining_tools: vec![],
        completed_results: vec![ToolRoundResult::PendingGoalReport {
            report: report.clone(),
            duration_ms: 4,
        }],
        pending_sub_agents: vec![],
        assistant_message: assistant.clone(),
    };
    (assistant, state)
}

async fn goal_fixture() -> (Database, AssistantMessage, GoalPendingReport, ConvState) {
    let db = Database::open_in_memory().await.unwrap();
    let root = db
        .create_conversation(CONVERSATION, "goal round", "/tmp", true, None, None)
        .await
        .unwrap();
    let actor = GoalActor::GeneratedHost {
        action_id: text("runtime-goal"),
    };
    db.apply_conversation_goal(
        &GoalMutation {
            owner: root.product_conversation_id.clone(),
            expected_version: GoalVersion::INITIAL,
            action_id: text("create-runtime-goal"),
            attribution: GoalAttribution {
                requested_by: actor.clone(),
                applied_by: actor,
            },
            change: GoalChange::Create {
                goal_id: GoalId::parse("runtime-goal").unwrap(),
                objective: text("complete the actual report round"),
            },
        },
        CONVERSATION,
        GoalTimestampUs::parse(10).unwrap(),
    )
    .await
    .unwrap();
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            origin: InputOrigin::UnknownHistorical,
            text: "do the work".into(),
            images: vec![],
            files: vec![],
            message_id: "goal-user-input".into(),
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "do the work".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    let accepted = db
        .workflow_repository()
        .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("runtime-goal-turn").unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(CONVERSATION.into()),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("created")
    };
    let workflow = db
        .workflow_repository()
        .workflow_id_for_turn(turn_id)
        .await
        .unwrap()
        .unwrap();
    let execution = GoalExecution::parse(
        i64::try_from(workflow.0).unwrap(),
        i64::try_from(turn_id.0).unwrap(),
    )
    .unwrap();
    let binding = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    let report = GoalPendingReport::new(
        binding.try_into().unwrap(),
        GoalReportCall {
            transcript_id: text(CONVERSATION),
            message_id: text("runtime-assistant"),
            tool_use_id: text("report"),
        },
        GoalReportOutcome::Complete(text("objective is complete")),
    );
    let (assistant, state) = goal_report_round_state(&report);
    db.update_conversation_state(CONVERSATION, &state)
        .await
        .unwrap();
    (db, assistant, report, state)
}

fn pending_checkpoint(assistant: AssistantMessage, report: GoalPendingReport) -> CheckpointData {
    CheckpointData::tool_round(
        assistant,
        vec![
            ToolRoundResult::PendingGoalReport {
                report,
                duration_ms: 4,
            },
            ToolResult::success("other".into(), "normal tool output".into()).into(),
        ],
    )
    .unwrap()
}

fn active(report: &GoalPendingReport) -> crate::runtime::traits::ActiveDirectTurn {
    crate::runtime::traits::ActiveDirectTurn {
        turn_id: TurnAuthorityId(u64::try_from(report.execution().turn_id()).unwrap()),
        generation: 0,
    }
}

#[tokio::test]
async fn conversation_goal_pending_intent_survives_state_and_reducer_until_actual_runtime_commit() {
    let (db, assistant, report, state) = goal_fixture().await;
    let restored: ConvState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(state, restored);
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    let mut receiver = rt.broadcast_tx.subscribe();
    let transition = transition(
        &restored,
        &rt.context,
        Event::ToolComplete {
            tool_use_id: "other".into(),
            result: ToolResult::success("other".into(), "normal tool output".into()),
        },
    )
    .unwrap();
    let checkpoint = transition
        .effects
        .into_iter()
        .find_map(|effect| {
            if let Effect::PersistCheckpoint { data } = effect {
                Some(data)
            } else {
                None
            }
        })
        .unwrap();
    let CheckpointData::ToolRound { tool_results, .. } = &checkpoint;
    assert!(matches!(
        tool_results[0],
        ToolRoundResult::PendingGoalReport { .. }
    ));
    assert!(db.get_messages(CONVERSATION).await.unwrap().is_empty());
    assert!(receiver.try_recv().is_err());
    rt.state = transition.new_state;
    rt.execute_effect(Effect::PersistCheckpoint { data: checkpoint })
        .await
        .unwrap();
    assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 3);
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        rt.state
    );
    assert!(matches!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Complete(_)
    ));
    assert!(receiver.try_recv().is_ok());
    let later = ConvState::LlmRequesting { attempt: 4 };
    db.update_conversation_state(CONVERSATION, &later)
        .await
        .unwrap();
    rt.execute_effect(Effect::PersistCheckpoint {
        data: pending_checkpoint(assistant, report.clone()),
    })
    .await
    .unwrap();
    assert_eq!(rt.state, later);
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        2
    );
}

#[tokio::test]
async fn conversation_goal_pending_runtime_owner_failure_never_broadcasts_success_or_changes_goal()
{
    let (db, assistant, report, state) = goal_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    let mut receiver = rt.broadcast_tx.subscribe();
    sqlx::query("CREATE TRIGGER fail_goal_checkpoint BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'goal runtime write failed'); END").execute(db.pool()).await.unwrap();
    let error = rt
        .execute_effect(Effect::PersistCheckpoint {
            data: pending_checkpoint(assistant.clone(), report.clone()),
        })
        .await
        .unwrap_err();
    assert!(error.contains("goal runtime write failed"));
    assert!(receiver.try_recv().is_err());
    assert!(db.get_messages(CONVERSATION).await.unwrap().is_empty());
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        state
    );
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    sqlx::query("DROP TRIGGER fail_goal_checkpoint")
        .execute(db.pool())
        .await
        .unwrap();
    rt.execute_effect(Effect::PersistCheckpoint {
        data: pending_checkpoint(assistant, report),
    })
    .await
    .unwrap();
    assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 3);
}

#[tokio::test]
async fn conversation_goal_runtime_pending_provider_owner_keeps_exact_public_replay_and_retry() {
    let (db, assistant, report, _) = goal_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    let replay = AnthropicResponseSet::with_public_content(
        AnthropicResponseIdentity {
            response_id: "goal-runtime-provider".into(),
            model: "claude-opus-5-5".into(),
        },
        assistant.content.clone(),
        vec![],
    )
    .unwrap()
    .with_owner_message_id(assistant.message_id.clone());
    rt.pending_provider_replay_update = Some(ProviderReplayUpdate::Anthropic(replay.clone()));
    sqlx::query("CREATE TRIGGER fail_goal_replay BEFORE INSERT ON active_provider_replay_state BEGIN SELECT RAISE(ABORT,'goal replay write failed'); END").execute(db.pool()).await.unwrap();
    assert!(rt
        .execute_effect(Effect::PersistCheckpoint {
            data: pending_checkpoint(assistant.clone(), report.clone())
        })
        .await
        .is_err());
    assert!(rt.pending_provider_replay_update.is_some());
    assert!(db.get_messages(CONVERSATION).await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER fail_goal_replay")
        .execute(db.pool())
        .await
        .unwrap();
    rt.execute_effect(Effect::PersistCheckpoint {
        data: pending_checkpoint(assistant, report),
    })
    .await
    .unwrap();
    assert!(rt.pending_provider_replay_update.is_none());
    assert_eq!(
        db.load_provider_replay_state(CONVERSATION)
            .await
            .unwrap()
            .unwrap()
            .response_sets,
        vec![replay]
    );
}

#[tokio::test]
async fn conversation_goal_runtime_terminal_pending_checkpoint_stages_existing_obligation_not_idle()
{
    let (db, assistant, report, original) = goal_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let current = active(&report);
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(current.clone()));
    let settlement = ActiveDirectTurnSettlement {
        turn: current,
        conversation_id: CONVERSATION.into(),
        terminal: crate::runtime::traits::ActiveDirectTurnTerminal::Completed,
        state: ConvState::Idle,
        state_updated_at: Utc::now(),
        execution_occurrence_message_id: None,
    };
    let mut admitted = rt.admit_authoritative_effect().unwrap();
    rt.persist_checkpoint_with_terminal_obligation(
        pending_checkpoint(assistant, report.clone()),
        &settlement,
        &mut admitted,
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        original
    );
    assert_eq!(
        rt.direct_turn_terminal_fact,
        TerminalFactDurability::Durable
    );
    let obligation = db
        .workflow_repository()
        .load_active_terminal_obligation(&ConversationAuthority(CONVERSATION.into()))
        .await
        .unwrap()
        .expect("live exact-turn terminal obligation");
    assert_eq!(obligation.turn_id, settlement.turn.turn_id);
    assert_eq!(obligation.expected_generation, settlement.turn.generation);
    assert!(matches!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Complete(_)
    ));
}

#[tokio::test]
async fn conversation_goal_restart_pairs_uncommitted_pending_report_as_error_without_applying_it() {
    let (db, _, report, _) = goal_fixture().await;
    db.reset_all_to_idle().await.unwrap();
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    assert_eq!(messages.len(), 3);
    let report_message = messages
        .iter()
        .find(|message| {
            message.message_id
                == phoenix_core::domain::tool_result_identity::tool_result_message_id(
                    "runtime-assistant",
                    "report",
                )
        })
        .unwrap();
    let MessageContent::Tool(content) = &report_message.content else {
        panic!("report result")
    };
    assert!(content.is_error);
    assert!(content.content.contains("before checkpoint commit"));
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
}

#[tokio::test]
async fn conversation_goal_reducer_cancel_carries_pending_intent_without_rendering_a_success() {
    let (_, _, report, state) = goal_fixture().await;
    let context = ConvContext::new(
        CONVERSATION,
        std::path::PathBuf::from("/tmp"),
        "claude-opus-5-5",
        200_000,
    );
    let cancelling = transition(
        &state,
        &context,
        Event::UserCancel {
            reason: None,
            cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
        },
    )
    .unwrap();
    let restored: ConvState =
        serde_json::from_value(serde_json::to_value(&cancelling.new_state).unwrap()).unwrap();
    let result = transition(
        &restored,
        &context,
        Event::ToolAborted {
            tool_use_id: "other".into(),
        },
    )
    .unwrap();
    let checkpoint = result
        .effects
        .into_iter()
        .find_map(|effect| {
            if let Effect::PersistCheckpoint { data } = effect {
                Some(data)
            } else {
                None
            }
        })
        .unwrap();
    let CheckpointData::ToolRound { tool_results, .. } = checkpoint;
    assert!(
        matches!(&tool_results[0], ToolRoundResult::PendingGoalReport { report: carried, .. } if carried == &report)
    );
    assert!(matches!(
        tool_results[1].finalized().unwrap().outcome,
        ToolOutcome::Cancelled { .. }
    ));
    assert!(finalized_round_results(&tool_results).is_err());
}

#[tokio::test]
async fn conversation_goal_mixed_legacy_trusted_payload_keeps_live_authority_and_never_leaks_durable_instructions(
) {
    let (db, mut assistant, report, mut original) = goal_fixture().await;
    let catalog = crate::skills::AuthenticatedCoordinatorSkillCatalog::discover(None).unwrap();
    let payload =
        phoenix_skills::invoke_trusted_coordinator_builtin("phoenix-api", &catalog).unwrap();
    let legacy = serde_json::to_value(ToolResult {
        tool_use_id: "other".into(),
        outcome: ToolOutcome::TrustedInstructions {
            output: payload.to_string(),
        },
        duration_ms: None,
    })
    .unwrap();
    let entry: ToolRoundResult = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&entry).unwrap(), legacy);
    let ContentBlock::ToolUse { name, input, .. } = &mut assistant.content[1] else {
        panic!("call")
    };
    *name = "skill".into();
    *input = serde_json::json!({"skill_name":"phoenix-api"});
    let ConvState::ToolExecuting {
        current_tool,
        assistant_message,
        ..
    } = &mut original
    else {
        panic!("executing")
    };
    *current_tool = ToolCall::new(
        "other",
        ToolInput::from_name_and_value("skill", serde_json::json!({"skill_name":"phoenix-api"})),
    );
    *assistant_message = assistant.clone();
    db.update_conversation_state(CONVERSATION, &original)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    let data = CheckpointData::tool_round(
        assistant,
        vec![
            ToolRoundResult::PendingGoalReport {
                report,
                duration_ms: 4,
            },
            entry,
        ],
    )
    .unwrap();
    rt.execute_effect(Effect::PersistCheckpoint { data: data.clone() })
        .await
        .unwrap();
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    assert!(!serde_json::to_string(&messages)
        .unwrap()
        .contains(payload.as_str()));
    let durable = messages
        .iter()
        .find_map(|message| {
            let MessageContent::Tool(content) = &message.content else {
                return None;
            };
            (content.tool_use_id == "other").then_some(content)
        })
        .unwrap();
    assert_eq!(
        durable.content,
        "Authenticated built-in skill instructions were delivered for this live request."
    );
    assert_eq!(rt.pending_trusted_tool_results.len(), 1);
    assert_eq!(rt.pending_trusted_tool_results[0].output, payload.as_str());
    rt.execute_effect(Effect::PersistCheckpoint { data })
        .await
        .unwrap();
    assert_eq!(rt.pending_trusted_tool_results.len(), 1);
    assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 3);
}

fn db_assistant(assistant: &AssistantMessage) -> crate::db::Message {
    crate::db::Message {
        origin: InputOrigin::SystemGenerated,
        message_id: assistant.message_id.clone(),
        conversation_id: CONVERSATION.into(),
        sequence_id: 1,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(assistant.content.clone()),
        display_data: assistant.display_data.clone(),
        usage_data: assistant.usage.clone(),
        created_at: assistant.created_at,
    }
}

async fn block_goal_before_report(db: &Database, report: &GoalPendingReport) {
    let binding = db
        .conversation_goal_execution_binding(report.execution())
        .await
        .unwrap()
        .unwrap();
    let prior = GoalPendingReport::new(
        binding.try_into().unwrap(),
        GoalReportCall {
            transcript_id: text(CONVERSATION),
            message_id: text("prior-blocking-assistant"),
            tool_use_id: text("prior-blocking-report"),
        },
        GoalReportOutcome::Blocked(text("waiting on an external prerequisite")),
    );
    let assistant = db_assistant(&AssistantMessage::new(
        "prior-blocking-assistant".into(),
        vec![ContentBlock::ToolUse {
            id: "prior-blocking-report".into(),
            name: "goal_report".into(),
            input: serde_json::to_value(prior.outcome()).unwrap(),
        }],
        None,
        None,
    ));
    let results = [ToolRoundResult::PendingGoalReport {
        report: prior,
        duration_ms: 1,
    }];
    let state = ConvState::LlmRequesting { attempt: 1 };
    let checkpoint = phoenix_db::conversation_goals::round::GoalRoundCheckpoint {
        assistant: &assistant,
        execution: report.execution(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    let committed = db
        .persist_goal_tool_round_and_state(&checkpoint)
        .await
        .unwrap();
    assert!(
        matches!(&committed.results[0].content, MessageContent::Tool(result) if !result.is_error)
    );
}

async fn assert_report_after_settlement(
    db: &Database,
    checkpoint: &phoenix_db::conversation_goals::round::GoalRoundCheckpoint<'_>,
    evidence: &phoenix_db::workflow::TerminalEvidenceExpectation,
    obligation: &phoenix_db::workflow::DirectTurnTerminalObligationInput,
) {
    use crate::runtime::traits::classify_goal_terminal_mutation;
    db.workflow_repository()
        .terminalize_authoritative_turn(&phoenix_db::workflow::TerminalizeAuthoritativeTurnInput {
            command: phoenix_workflow::TurnCommand::Complete {
                turn_id: obligation.turn_id,
                expected_generation: 0,
            },
            projection: Some(obligation.projection.clone()),
            provider_replay_settlement: ProviderReplaySettlement::Preserve,
        })
        .await
        .unwrap();
    assert!(matches!(
        classify_goal_terminal_mutation(
            db,
            checkpoint,
            evidence,
            obligation,
            "retry after settlement".into()
        )
        .await,
        TerminalMutationEstablishment::Established { .. }
    ));
    db.update_conversation_state(CONVERSATION, &ConvState::LlmRequesting { attempt: 4 })
        .await
        .unwrap();
    assert!(matches!(
        classify_goal_terminal_mutation(
            db,
            checkpoint,
            evidence,
            obligation,
            "projection changed".into()
        )
        .await,
        TerminalMutationEstablishment::Unclassifiable(_)
    ));
}

#[tokio::test]
async fn conversation_goal_terminal_response_loss_reconstructs_exact_success_or_refusal_without_reapplying(
) {
    use crate::runtime::traits::classify_goal_terminal_mutation;
    use phoenix_db::{
        conversation_goals::round::GoalRoundCheckpoint,
        workflow::{
            DirectTurnTerminalObligationInput, PersistedConversationProjection,
            TerminalEvidenceExpectation,
        },
    };
    for refuse in [false, true] {
        let (db, assistant, report, _) = goal_fixture().await;
        if refuse {
            block_goal_before_report(&db, &report).await;
        }
        let mut assistant = db_assistant(&assistant);
        assistant.sequence_id = db
            .get_messages(CONVERSATION)
            .await
            .unwrap()
            .iter()
            .map(|message| message.sequence_id)
            .max()
            .map_or(1, |sequence| sequence + 1);
        let results = [
            ToolRoundResult::PendingGoalReport {
                report: report.clone(),
                duration_ms: 4,
            },
            ToolResult::success("other".into(), "normal tool output".into()).into(),
        ];
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: report.execution(),
            results: &results,
            state: &ConvState::Idle,
            state_updated_at: Utc::now(),
        };
        let obligation = DirectTurnTerminalObligationInput {
            turn_id: active(&report).turn_id,
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Completed,
            projection: PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: checkpoint.state_updated_at,
            },
            response_message_id: None,
        };
        let evidence = TerminalEvidenceExpectation::GoalRound {
            assistant: Box::new(assistant.clone()),
            execution: checkpoint.execution,
            results: results.to_vec(),
        };
        assert!(matches!(
            classify_goal_terminal_mutation(
                &db,
                &checkpoint,
                &evidence,
                &obligation,
                "not committed".into()
            )
            .await,
            TerminalMutationEstablishment::KnownNotCommitted(_)
        ));
        let committed = db
            .persist_goal_terminal_tool_round(&checkpoint, &obligation, None)
            .await
            .unwrap();
        let classified = classify_goal_terminal_mutation(
            &db,
            &checkpoint,
            &evidence,
            &obligation,
            "commit response lost".into(),
        )
        .await;
        let TerminalMutationEstablishment::Established { value, .. } = classified else {
            panic!("committed evidence")
        };
        assert_eq!(
            serde_json::to_value(&value.results).unwrap(),
            serde_json::to_value(&committed.results).unwrap()
        );
        let MessageContent::Tool(result) = &value.results[0].content else {
            panic!("result")
        };
        assert_eq!(result.is_error, refuse);
        assert_eq!(
            db.conversation_goal(&report.mutation().owner)
                .await
                .unwrap()
                .unwrap()
                .version
                .get(),
            2
        );
        assert_report_after_settlement(&db, &checkpoint, &evidence, &obligation).await;
    }
}

#[tokio::test]
async fn conversation_goal_terminal_late_replay_failure_is_known_rollback() {
    let (db, assistant, report, original) = goal_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let current = active(&report);
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(current.clone()));
    rt.pending_provider_replay_update = Some(ProviderReplayUpdate::Anthropic(
        AnthropicResponseSet::with_public_content(
            AnthropicResponseIdentity {
                response_id: "terminal-goal".into(),
                model: "claude-opus-5-5".into(),
            },
            assistant.content.clone(),
            vec![],
        )
        .unwrap()
        .with_owner_message_id(assistant.message_id.clone()),
    ));
    let settlement = ActiveDirectTurnSettlement {
        turn: current,
        conversation_id: CONVERSATION.into(),
        terminal: crate::runtime::traits::ActiveDirectTurnTerminal::Completed,
        state: ConvState::Idle,
        state_updated_at: Utc::now(),
        execution_occurrence_message_id: None,
    };
    sqlx::query("CREATE TRIGGER fail_terminal_goal BEFORE INSERT ON direct_turn_terminal_obligations BEGIN SELECT RAISE(ABORT,'terminal goal late failure'); END").execute(db.pool()).await.unwrap();
    let mut admitted = rt.admit_authoritative_effect().unwrap();
    assert!(rt
        .persist_checkpoint_with_terminal_obligation(
            pending_checkpoint(assistant.clone(), report.clone()),
            &settlement,
            &mut admitted
        )
        .await
        .is_err());
    assert_ne!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    assert!(rt.pending_provider_replay_update.is_some());
    assert!(db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_none());
    assert!(db.get_messages(CONVERSATION).await.unwrap().is_empty());
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        original
    );
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    sqlx::query("DROP TRIGGER fail_terminal_goal")
        .execute(db.pool())
        .await
        .unwrap();
    rt.persist_checkpoint_with_terminal_obligation(
        pending_checkpoint(assistant, report),
        &settlement,
        &mut admitted,
    )
    .await
    .unwrap();
    assert!(rt.pending_provider_replay_update.is_none());
    assert!(db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        original
    );
}

#[tokio::test]
async fn conversation_goal_terminal_checkpoint_after_cancel_is_known_not_committed_without_fatal_authority(
) {
    let dir = tempfile::tempdir().unwrap();
    let (db, assistant, report, _) = goal_fixture().await;
    let settlement = ActiveDirectTurnSettlement {
        turn: active(&report),
        conversation_id: CONVERSATION.into(),
        terminal: crate::runtime::traits::ActiveDirectTurnTerminal::Completed,
        state: ConvState::Idle,
        state_updated_at: Utc::now(),
        execution_occurrence_message_id: None,
    };
    db.workflow_repository()
        .terminalize_authoritative_turn(&phoenix_db::workflow::TerminalizeAuthoritativeTurnInput {
            command: phoenix_workflow::TurnCommand::Cancel {
                turn_id: active(&report).turn_id,
                expected_generation: 0,
            },
            projection: Some(phoenix_db::workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            }),
            provider_replay_settlement: ProviderReplaySettlement::Preserve,
        })
        .await
        .unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    let mut admitted = rt.admit_authoritative_effect().unwrap();
    let error = rt
        .persist_checkpoint_with_terminal_obligation(
            pending_checkpoint(assistant, report.clone()),
            &settlement,
            &mut admitted,
        )
        .await
        .unwrap_err();
    assert!(error.contains("StaleGeneration"), "{error}");
    assert_ne!(rt.local_terminal_authority, LocalTerminalAuthority::Fatal);
    assert!(db.get_messages(CONVERSATION).await.unwrap().is_empty());
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
}

fn journey_response(content: Vec<ContentBlock>, end_turn: bool) -> LlmResponse {
    let replay = (!end_turn).then(|| {
        ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: uuid::Uuid::new_v4().to_string(),
                    model: "claude-opus-5-5".into(),
                },
                content.clone(),
                vec![],
            )
            .unwrap(),
        )
    });
    LlmResponse {
        content,
        end_turn,
        usage: phoenix_core::domain::llm_types::Usage::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
        provider_replay: replay,
    }
}

async fn journey_provider_step(rt: &mut TestRuntime) {
    let (generation, outcome) =
        tokio::time::timeout(std::time::Duration::from_secs(30), rt.llm_outcome_rx.recv())
            .await
            .expect("provider completion")
            .expect("outcome channel");
    rt.process_generation_tagged_llm_outcome(generation, outcome)
        .await;
}

async fn journey_report_step(rt: &mut TestRuntime) {
    let event = tokio::time::timeout(std::time::Duration::from_secs(30), rt.event_rx.recv())
        .await
        .expect("internal report completion")
        .expect("event channel");
    assert!(matches!(&event, Event::GoalReportPrepared { .. }));
    rt.process_event(event).await.unwrap();
}

async fn journey_input(db: &Database) {
    db.add_message(
        "goal-user-input",
        CONVERSATION,
        &MessageContent::user("do the work"),
        None,
        None,
    )
    .await
    .unwrap();
    db.update_conversation_state(CONVERSATION, &ConvState::LlmRequesting { attempt: 2 })
        .await
        .unwrap();
}

fn enable_admitted_report(rt: &mut TestRuntime, report: &GoalPendingReport) {
    let GoalChange::Report { binding, .. } = report.mutation().change else {
        panic!("admission binding")
    };
    rt.goal_report_emitter = Some(GoalReportEmitter {
        binding: binding.try_into().unwrap(),
    });
}

async fn assert_actual_report_committed(
    db: &Database,
    report: &GoalPendingReport,
    rt: &TestRuntime,
) -> String {
    let committed = db.get_messages(CONVERSATION).await.unwrap();
    assert_eq!(committed.len(), 3);
    let stored_result = committed
        .iter()
        .find_map(|message| {
            let MessageContent::Tool(result) = &message.content else {
                return None;
            };
            (result.tool_use_id == "actual-report").then_some(result)
        })
        .unwrap();
    assert!(!stored_result.is_error);
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        rt.state
    );
    assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    let replay = db
        .load_provider_replay_state(CONVERSATION)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.response_sets.len(), 1);
    let assistant = committed
        .iter()
        .find(|message| matches!(&message.content, MessageContent::Agent(_)))
        .unwrap();
    assert_eq!(
        replay.response_sets[0].owner_message_id,
        assistant.message_id
    );
    let MessageContent::Agent(blocks) = &assistant.content else {
        unreachable!()
    };
    assert_eq!(&replay.response_sets[0].public_content, blocks);
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        2
    );
    stored_result.content.clone()
}

#[tokio::test]
async fn conversation_goal_actual_internal_emitter_processes_round_before_next_model_request_and_output(
) {
    let (db, _, report, _) = goal_fixture().await;
    journey_input(&db).await;
    let client = Arc::new(RenderingAnthropicClient::scripted(vec![
        journey_response(
            vec![ContentBlock::ToolUse {
                id: "actual-report".into(),
                name: "goal_report".into(),
                input: serde_json::json!({"status":"complete","reason":"actual model work finished"}),
            }],
            false,
        ),
        journey_response(
            vec![ContentBlock::text("Delivered the completed result.")],
            true,
        ),
    ]));
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    enable_admitted_report(&mut rt, &report);
    dispatch(&mut rt).await;
    journey_provider_step(&mut rt).await;
    assert!(db
        .get_messages(CONVERSATION)
        .await
        .unwrap()
        .iter()
        .all(|message| !matches!(&message.content, MessageContent::Tool(_))));
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    journey_report_step(&mut rt).await;
    let stored_result = assert_actual_report_committed(&db, &report, &rt).await;
    journey_provider_step(&mut rt).await;
    let requests = client.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests[0]
        .tool_availability
        .declarations()
        .iter()
        .any(
            |tool| tool.name == "goal_report" && tool.input_schema["additionalProperties"] == false
        ));
    let second_wire = client.wires.lock().unwrap()[1].clone();
    let results: Vec<_> = second_wire["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result" && block["tool_use_id"] == "actual-report")
        .collect();
    assert_eq!(results.len(), 1);
    assert!(!results[0]["is_error"].as_bool().unwrap_or(false));
    assert!(results[0]["content"]
        .to_string()
        .contains("recorded complete at owner version 2"));
    assert!(results[0]["content"]
        .to_string()
        .contains(stored_result.as_str()));
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    assert_eq!(messages.len(), 4);
    assert!(messages.iter().any(|message| matches!(&message.content, MessageContent::Agent(blocks) if blocks.iter().any(|block| matches!(block, ContentBlock::Text { text } if text == "Delivered the completed result.")))));
    assert!(matches!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Complete(_)
    ));
}

#[tokio::test]
async fn conversation_goal_actual_report_checkpoint_failure_keeps_round_goal_and_replay_unadvanced_until_retry(
) {
    let (db, _, report, _) = goal_fixture().await;
    journey_input(&db).await;
    let client = Arc::new(RenderingAnthropicClient::scripted(vec![
        journey_response(
            vec![ContentBlock::ToolUse {
                id: "rollback-report".into(),
                name: "goal_report".into(),
                input: serde_json::json!({"status":"complete","reason":"only the committed result is true"}),
            }],
            false,
        ),
        journey_response(vec![ContentBlock::text("Committed after retry.")], true),
    ]));
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    enable_admitted_report(&mut rt, &report);
    dispatch(&mut rt).await;
    journey_provider_step(&mut rt).await;
    let event = rt.event_rx.try_recv().expect("prepared report");
    assert!(matches!(&event, Event::GoalReportPrepared { .. }));
    let before_state = db.get_conversation(CONVERSATION).await.unwrap().state;
    let before_replay = db.load_provider_replay_state(CONVERSATION).await.unwrap();
    assert!(before_replay.is_some());
    let mut receiver = rt.broadcast_tx.subscribe();
    sqlx::query("CREATE TRIGGER fail_actual_goal_round BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'actual goal checkpoint failed'); END")
        .execute(db.pool()).await.unwrap();
    let error = rt.process_event(event.clone()).await.unwrap_err();
    assert!(error.contains("actual goal checkpoint failed"));
    assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 1);
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        before_state
    );
    assert_eq!(
        db.load_provider_replay_state(CONVERSATION).await.unwrap(),
        before_replay
    );
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert!(rt.llm_task_handle.is_none());
    while let Ok(event) = receiver.try_recv() {
        assert!(
            !matches!(event, SseEvent::Message { message, .. } if matches!(&message.content, MessageContent::Tool(_)))
        );
    }
    sqlx::query("DROP TRIGGER fail_actual_goal_round")
        .execute(db.pool())
        .await
        .unwrap();
    rt.process_event(event).await.unwrap();
    assert_eq!(db.get_messages(CONVERSATION).await.unwrap().len(), 3);
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        rt.state
    );
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        2
    );
    assert_eq!(
        db.conversation_goal_history(&report.mutation().owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        2
    );
    journey_provider_step(&mut rt).await;
    assert_eq!(client.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn conversation_goal_actual_report_prepared_before_stop_cannot_commit_success_after_stop() {
    let (db, _, report, _) = goal_fixture().await;
    journey_input(&db).await;
    let client = Arc::new(RenderingAnthropicClient::scripted(vec![journey_response(
        vec![ContentBlock::ToolUse {
            id: "stopped-report".into(),
            name: "goal_report".into(),
            input: serde_json::json!({"status":"complete","reason":"prepared before stop"}),
        }],
        false,
    )]));
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(active(&report)));
    enable_admitted_report(&mut rt, &report);
    dispatch(&mut rt).await;
    journey_provider_step(&mut rt).await;
    let prepared = rt.event_rx.try_recv().expect("prepared report before Stop");
    assert!(matches!(&prepared, Event::GoalReportPrepared { .. }));
    assert!(phoenix_core::domain::sm_event::SubAgentEvent::try_from(prepared.clone()).is_err());
    rt.process_event(Event::UserCancel {
        reason: None,
        cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
    })
    .await
    .unwrap();
    assert!(matches!(rt.state, ConvState::CancellingTool { .. }));
    rt.process_event(prepared).await.unwrap();
    assert!(matches!(rt.state, ConvState::Idle));
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    assert_eq!(
        db.conversation_goal_history(&report.mutation().owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert!(messages.iter().any(|message| matches!(&message.content, MessageContent::Tool(result) if result.tool_use_id == "stopped-report" && result.is_error)));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert!(rt.llm_task_handle.is_none());
    assert!(rt.llm_outcome_rx.try_recv().is_err());
}

#[tokio::test]
async fn conversation_goal_internal_binding_is_parent_only_even_when_injected_in_child_context() {
    let (db, _, report, _) = goal_fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db,
        dir.path(),
        Arc::new(RenderingAnthropicClient::new()),
        Arc::new(DeferredMcpCatalog::new(false)),
    );
    assert!(rt.parent_goal_report_emitter().is_none());
    enable_admitted_report(&mut rt, &report);
    assert!(rt.parent_goal_report_emitter().is_some());
    rt.context.is_sub_agent = true;
    assert!(rt.parent_goal_report_emitter().is_none());
    rt.context.is_sub_agent = false;
    rt.context.is_coordinator = true;
    assert!(rt.parent_goal_report_emitter().is_none());
}

#[tokio::test]
async fn conversation_goal_actual_mixed_proposal_and_report_are_all_paired_errors_without_goal_history(
) {
    let (db, _, report, _) = goal_fixture().await;
    journey_input(&db).await;
    let client = Arc::new(RenderingAnthropicClient::scripted(vec![
        journey_response(
            vec![
                ContentBlock::ToolUse {
                    id: "mixed-report".into(),
                    name: "goal_report".into(),
                    input: serde_json::json!({"status":"complete","reason":"must not apply mixed batch"}),
                },
                ContentBlock::ToolUse {
                    id: "mixed-proposal".into(),
                    name: "propose_task".into(),
                    input: serde_json::json!({"task_file":"tasks/00001-p2-ready--later.md"}),
                },
            ],
            false,
        ),
        journey_response(
            vec![ContentBlock::text("I will issue one tool at a time.")],
            true,
        ),
    ]));
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime(
        db.clone(),
        dir.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::with_proposals()),
    )
    .with_active_direct_turn(Some(active(&report)));
    enable_admitted_report(&mut rt, &report);
    dispatch(&mut rt).await;
    journey_provider_step(&mut rt).await;
    assert!(rt.event_rx.try_recv().is_err());
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    let results: Vec<_> = messages
        .iter()
        .filter_map(|message| {
            if let MessageContent::Tool(result) = &message.content {
                Some(result)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.is_error));
    for tool_id in ["mixed-report", "mixed-proposal"] {
        assert_eq!(
            results
                .iter()
                .filter(|result| result.tool_use_id == tool_id)
                .count(),
            1
        );
    }
    assert_eq!(
        db.conversation_goal_history(&report.mutation().owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(db
        .list_fork_proposals_for_origin(CONVERSATION)
        .await
        .unwrap()
        .is_empty());
    journey_provider_step(&mut rt).await;
    assert_eq!(client.requests.lock().unwrap().len(), 2);
}

async fn goal_proposal_context(db: &Database, dir: &tempfile::TempDir) -> ConvContext {
    let path = dir.path().to_str().unwrap().to_owned();
    db.update_conversation_mode(
        CONVERSATION,
        &crate::db::ConvMode::Branch {
            branch_name: NonEmptyString::new("goal-journey").unwrap(),
            worktree_path: NonEmptyString::new(path.clone()).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
        },
    )
    .await
    .unwrap();
    let mut context = ConvContext::new(
        CONVERSATION,
        dir.path().to_path_buf(),
        "claude-opus-5-5",
        200_000,
    );
    context.mode = phoenix_core::domain::sm_state::ModeKind::Branch;
    context.mode_context = Some(phoenix_core::domain::mode_context::ModeContext::Branch {
        branch_name: "goal-journey".into(),
        base_branch: "main".into(),
        worktree_path: path,
    });
    context
}

#[tokio::test]
async fn conversation_goal_report_then_separate_valid_proposal_round_preserves_both_real_exchanges()
{
    let (db, _, report, _) = goal_fixture().await;
    journey_input(&db).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("tasks")).unwrap();
    let task_file = "tasks/00001-p2-ready--follow-up.md";
    std::fs::write(
        dir.path().join(task_file),
        "# Follow-up\n\nContinue the next bounded unit of work.\n",
    )
    .unwrap();

    let client = Arc::new(RenderingAnthropicClient::scripted(vec![
        journey_response(
            vec![ContentBlock::ToolUse {
                id: "proposal-preceding-report".into(),
                name: "goal_report".into(),
                input: serde_json::json!({"status":"complete","reason":"ready for the separate follow-up proposal"}),
            }],
            false,
        ),
        journey_response(
            vec![ContentBlock::ToolUse {
                id: "sole-proposal".into(),
                name: "propose_task".into(),
                input: serde_json::json!({"task_file":task_file}),
            }],
            false,
        ),
    ]));
    let context = goal_proposal_context(&db, &dir).await;
    let mut rt = runtime_for_context(
        db.clone(),
        context,
        client.clone(),
        Arc::new(DeferredMcpCatalog::with_proposals()),
    )
    .with_active_direct_turn(Some(active(&report)));
    enable_admitted_report(&mut rt, &report);
    dispatch(&mut rt).await;
    journey_provider_step(&mut rt).await;
    journey_report_step(&mut rt).await;
    journey_provider_step(&mut rt).await;
    assert!(
        matches!(&rt.state, ConvState::AwaitingTaskApproval { task_file: pending, .. } if pending == task_file)
    );
    assert_eq!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        rt.state
    );
    assert!(db
        .list_fork_proposals_for_origin(CONVERSATION)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        2
    );
    assert!(rt.llm_task_handle.is_none());
    assert!(rt.llm_outcome_rx.try_recv().is_err());
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    let messages = db.get_messages(CONVERSATION).await.unwrap();
    for id in ["proposal-preceding-report", "sole-proposal"] {
        let results: Vec<_> = messages
            .iter()
            .filter_map(|message| {
                if let MessageContent::Tool(result) = &message.content {
                    (result.tool_use_id == id).then_some(result)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
    }
}

#[tokio::test]
async fn conversation_goal_model_cannot_supply_binding_or_actor_and_production_gate_defaults_absent(
) {
    for inject_binding in [false, true] {
        let (db, _, report, _) = goal_fixture().await;
        journey_input(&db).await;
        let client = Arc::new(RenderingAnthropicClient::scripted(vec![
            journey_response(
                vec![ContentBlock::ToolUse {
                    id: "forged-report".into(),
                    name: "goal_report".into(),
                    input: serde_json::json!({"status":"complete","reason":"forged","actor":"host","binding":{"turn_id":1}}),
                }],
                false,
            ),
            journey_response(
                vec![ContentBlock::text("No goal mutation was admitted.")],
                true,
            ),
        ]));
        let dir = tempfile::tempdir().unwrap();
        let mut rt = runtime(
            db.clone(),
            dir.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(false)),
        )
        .with_active_direct_turn(Some(active(&report)));
        assert!(rt.goal_report_emitter.is_none());
        if inject_binding {
            enable_admitted_report(&mut rt, &report);
        }
        dispatch(&mut rt).await;
        journey_provider_step(&mut rt).await;
        let event = rt
            .event_rx
            .try_recv()
            .expect("synchronous invalid report result");
        assert!(matches!(&event, Event::ToolComplete { result, .. } if result.is_error()));
        rt.process_event(event).await.unwrap();
        assert_eq!(
            db.conversation_goal(&report.mutation().owner)
                .await
                .unwrap()
                .unwrap()
                .version
                .get(),
            1
        );
        let messages = db.get_messages(CONVERSATION).await.unwrap();
        let result = messages
            .iter()
            .find_map(|message| {
                if let MessageContent::Tool(result) = &message.content {
                    Some(result)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(result.is_error);
        assert_eq!(
            client.requests.lock().unwrap()[0]
                .tool_availability
                .declarations()
                .iter()
                .any(|tool| tool.name == "goal_report"),
            inject_binding
        );
        journey_provider_step(&mut rt).await;
    }
}
