use super::*;
use phoenix_core::domain::provider_replay::ProviderReplaySettlement;
use phoenix_core::domain::{
    conversation_goal::{
        GoalActor, GoalAttribution, GoalChange, GoalExecution, GoalId, GoalMutation,
        GoalPendingReport, GoalReportCall, GoalReportOutcome, GoalState, GoalText, GoalTimestampUs,
        GoalVersion, ToolRoundResult,
    },
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
    assert!(db
        .workflow_repository()
        .load_terminal_obligation(settlement.turn.turn_id)
        .await
        .unwrap()
        .is_some());
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
    assert!(tool_results[1].finalized().unwrap().is_error());
    assert!(finalized_round_results(&tool_results).is_err());
}

#[tokio::test]
async fn conversation_goal_mixed_legacy_trusted_payload_keeps_live_authority_and_never_leaks_durable_instructions(
) {
    let (db, mut assistant, report, mut original) = goal_fixture().await;
    let catalog = crate::skills::AuthenticatedCoordinatorSkillCatalog::discover(None).unwrap();
    let payload =
        phoenix_skills::invoke_trusted_coordinator_builtin("phoenix-api", &catalog).unwrap();
    let legacy = serde_json::json!({"tool_use_id":"other","outcome":{"TrustedInstructions":{"output":payload}}});
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
        ToolInput::parse("skill", serde_json::json!({"skill_name":"phoenix-api"})),
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
    assert!(!serde_json::to_string(&messages).unwrap().contains(&payload));
    assert_eq!(rt.pending_trusted_tool_results.len(), 1);
    assert_eq!(rt.pending_trusted_tool_results[0].instructions, payload);
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

#[tokio::test]
async fn conversation_goal_terminal_response_loss_reconstructs_exact_success_or_refusal_without_reapplying(
) {
    use crate::runtime::traits::classify_goal_terminal_mutation;
    use phoenix_core::domain::conversation_goal::GoalDisposition;
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
            let mut mutation = report.mutation();
            let actor = GoalActor::GeneratedHost {
                action_id: text("block-before-report"),
            };
            mutation.action_id = text("block-before-report");
            mutation.attribution = GoalAttribution {
                requested_by: actor.clone(),
                applied_by: actor,
            };
            let GoalChange::Report { binding, .. } = &mutation.change else {
                panic!("report binding")
            };
            let phoenix_core::domain::conversation_goal::GoalAdmissionObservation::Goal(fence) =
                &binding.observation
            else {
                panic!("goal observation")
            };
            mutation.change = GoalChange::SetDisposition {
                goal_id: fence.goal_id.clone(),
                disposition: GoalDisposition::Blocked(text("waiting on an external prerequisite")),
            };
            db.apply_conversation_goal(
                &mutation,
                CONVERSATION,
                GoalTimestampUs::parse(11).unwrap(),
            )
            .await
            .unwrap();
        }
        let assistant = db_assistant(&assistant);
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
        db.workflow_repository()
            .terminalize_authoritative_turn(
                &phoenix_db::workflow::TerminalizeAuthoritativeTurnInput {
                    command: phoenix_workflow::TurnCommand::Complete {
                        turn_id: obligation.turn_id,
                        expected_generation: 0,
                    },
                    projection: Some(obligation.projection.clone()),
                    provider_replay_settlement: ProviderReplaySettlement::Preserve,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            classify_goal_terminal_mutation(
                &db,
                &checkpoint,
                &evidence,
                &obligation,
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
                &db,
                &checkpoint,
                &evidence,
                &obligation,
                "projection changed".into()
            )
            .await,
            TerminalMutationEstablishment::Unclassifiable(_)
        ));
    }
}

#[tokio::test]
async fn conversation_goal_terminal_late_replay_failure_is_known_rollback_and_retirement_is_not_fatal(
) {
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

    let (db, assistant, report, _) = goal_fixture().await;
    db.workflow_repository()
        .terminalize_authoritative_turn(&phoenix_db::workflow::TerminalizeAuthoritativeTurnInput {
            command: phoenix_workflow::TurnCommand::Cancel {
                turn_id: active(&report).turn_id,
                expected_generation: 0,
                cause: phoenix_workflow::CancelCause::UserRequested,
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
    rt.persist_checkpoint_with_terminal_obligation(
        pending_checkpoint(assistant, report.clone()),
        &settlement,
        &mut admitted,
    )
    .await
    .unwrap();
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
