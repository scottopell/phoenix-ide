use chrono::{DateTime, Utc};
use phoenix_core::domain::{
    conversation_goal::{
        GoalChange, GoalExecution, GoalPendingReport, GoalReportCall, GoalReportOutcome, GoalState,
        ToolRoundResult,
    },
    db_schema::{
        ForkProposal, ForkProposalStatus, InputOrigin, Message, MessageContent, MessageType,
        ToolResult,
    },
    llm_types::ContentBlock,
    provider_replay::{
        AnthropicResponseIdentity, AnthropicResponseSet, ProviderReplaySettlement,
        ProviderReplayUpdate,
    },
    responses_replay::ResponsesResponseSet,
    sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    },
    sm_state::ConvState,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
    TurnAuthorityId, TurnCommand, TurnOutcome, TurnTerminal,
};

use super::super::{fixture, t};
use crate::{
    conversation_goals::round::{GoalRoundCheckpoint, ToolRoundWrite},
    workflow::{
        AcceptAuthoritativeTurn, DirectTurnTerminalObligationInput,
        PersistedConversationProjection, TerminalizeAuthoritativeTurnInput,
    },
    Database, DbResult,
};

#[derive(Clone, Copy, Debug)]
enum Owner {
    Normal,
    Anthropic,
    Responses,
    Fork,
    Terminal,
    ForkAnthropic,
    ForkResponses,
    TerminalAnthropic,
    TerminalResponses,
}

impl Owner {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Terminal | Self::TerminalAnthropic | Self::TerminalResponses
        )
    }
    fn replay(self) -> Option<Self> {
        match self {
            Self::Anthropic | Self::ForkAnthropic | Self::TerminalAnthropic => {
                Some(Self::Anthropic)
            }
            Self::Responses | Self::ForkResponses | Self::TerminalResponses => {
                Some(Self::Responses)
            }
            Self::Normal | Self::Fork | Self::Terminal => None,
        }
    }
}
const OWNERS: [Owner; 9] = [
    Owner::Normal,
    Owner::Anthropic,
    Owner::Responses,
    Owner::Fork,
    Owner::Terminal,
    Owner::ForkAnthropic,
    Owner::ForkResponses,
    Owner::TerminalAnthropic,
    Owner::TerminalResponses,
];

async fn admitted_fixture() -> (Database, Message, GoalPendingReport) {
    admitted_fixture_with_control(
        crate::conversation_goals::control::GoalControlInstallation::Disabled,
    )
    .await
}

async fn control_command(
    db: &Database,
    owner: &phoenix_core::domain::product_conversation::ProductConversationId,
    command: crate::conversation_goals::control::ControlCommand,
    expected_control_version: u64,
) {
    db.apply_goal_control(
        &crate::conversation_goals::control::ControlRequest {
            owner: owner.clone(),
            principal_id: uuid::Uuid::new_v4(),
            request_id: uuid::Uuid::new_v4(),
            command,
            expected_control_version,
        },
        super::super::at(),
    )
    .await
    .unwrap();
}

#[allow(clippy::too_many_lines)]
async fn admitted_fixture_with_control(
    installation: crate::conversation_goals::control::GoalControlInstallation,
) -> (Database, Message, GoalPendingReport) {
    let (db, command) = fixture().await;
    let db = match installation {
        crate::conversation_goals::control::GoalControlInstallation::Disabled => db,
        crate::conversation_goals::control::GoalControlInstallation::Installed => {
            let db = db.install_goal_controls_for_tests().await.unwrap();
            control_command(
                &db,
                &command.owner,
                crate::conversation_goals::control::ControlCommand::Release,
                0,
            )
            .await;
            db
        }
    };
    db.apply_conversation_goal(&command, "full-root", super::super::at())
        .await
        .unwrap();
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            origin: InputOrigin::UnknownHistorical,
            text: "Work on the admitted goal".into(),
            images: vec![],
            files: vec![],
            message_id: "goal-owner-input".into(),
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "Work on the admitted goal".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    let accepted = db
        .workflow_repository()
        .accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("goal-owner-turn").unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority("full-root".into()),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("new accepted turn")
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
    if installation == crate::conversation_goals::control::GoalControlInstallation::Installed {
        use crate::workflow::{
            ClaimAuthoritativeTurnInput, LocalAuthorityResult, MaterializeAuthoritativeTurnInput,
            MaterializeAuthoritativeTurnOutcome,
        };
        let repo = db.workflow_repository();
        let claim = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id: workflow,
                process_incarnation: phoenix_workflow::ProcessIncarnation(1),
                now: Timestamp(2),
                lease_until: phoenix_workflow::LeaseExpiry(100),
            })
            .await
            .unwrap();
        let LocalAuthorityResult::DurableFactEstablished(
            MaterializeAuthoritativeTurnOutcome::Materialized(materialized),
        ) = repo
            .materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
                turn_id,
                authority: claim.authority.unwrap(),
                prepared: payload,
                sequence_id: 2,
                created_at: Timestamp(3),
                accepted_state: ConvState::LlmRequesting { attempt: 1 },
                state_updated_at: Utc::now(),
                now: Timestamp(3),
            })
            .await
        else {
            panic!("materialized accepted Runtime turn")
        };
        let canonical: Option<String> =
            sqlx::query_scalar("SELECT canonical_message_id FROM durable_turns WHERE turn_id=?")
                .bind(execution.turn_id())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            canonical.as_deref(),
            Some(materialized.message.message_id.as_str())
        );
        assert_eq!(
            materialized.message.message_id,
            "full-root:goal-owner-input"
        );
        assert_eq!(materialized.generation, 0);
    }
    assert_eq!(execution, GoalExecution::parse(1, 1).unwrap());
    let binding = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    let report = GoalPendingReport::new(
        binding.try_into().unwrap(),
        GoalReportCall {
            transcript_id: t("full-root"),
            message_id: t("owner-assistant"),
            tool_use_id: t("report-call"),
        },
        GoalReportOutcome::Complete(t("finished the objective")),
    );
    let assistant = Message {
        message_id: "owner-assistant".into(),
        conversation_id: "full-root".into(),
        sequence_id: if installation
            == crate::conversation_goals::control::GoalControlInstallation::Installed
        {
            3
        } else {
            2
        },
        message_type: MessageType::Agent,
        content: MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: "report-call".into(),
            name: "goal_report".into(),
            input: serde_json::to_value(report.outcome()).unwrap(),
        }]),
        display_data: None,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: Utc::now(),
    };
    db.update_conversation_state("full-root", &ConvState::LlmRequesting { attempt: 1 })
        .await
        .unwrap();
    (db, assistant, report)
}

fn replay_update(owner: Owner, assistant: &Message) -> ProviderReplayUpdate {
    let MessageContent::Agent(blocks) = &assistant.content else {
        panic!("assistant")
    };
    match owner {
        Owner::Responses | Owner::ForkResponses | Owner::TerminalResponses => ProviderReplayUpdate::Responses(ResponsesResponseSet {
            response_id: "goal-response".into(),
            model: "model".into(),
            owner_message_id: assistant.message_id.clone(),
            public_content: blocks.clone(),
            output_items: blocks.iter().filter_map(|block| {
                let ContentBlock::ToolUse { id, name, input } = block else { return None; };
                Some(serde_json::json!({"type":"function_call","id":format!("fc-{id}"),"call_id":id,"name":name,"arguments":serde_json::to_string(input).unwrap()}))
            }).collect(),
        }),
        Owner::Normal | Owner::Anthropic | Owner::Fork | Owner::Terminal | Owner::ForkAnthropic | Owner::TerminalAnthropic => ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: "goal-response".into(),
                    model: "model".into(),
                },
                blocks.clone(),
                vec![],
            )
            .unwrap()
            .with_owner_message_id(assistant.message_id.clone()),
        ),
    }
}

fn proposal(assistant: &Message) -> ForkProposal {
    ForkProposal {
        id: "goal-proposal".into(),
        origin_conversation_id: assistant.conversation_id.clone(),
        task_file: "tasks/child.md".into(),
        title: "Follow-up".into(),
        priority: "p2".into(),
        body: "Child work".into(),
        status: ForkProposalStatus::Pending,
        fork_conversation_id: None,
        refinement_conversation_id: None,
        created_at: assistant.created_at,
        resolved_at: None,
    }
}

fn obligation(checkpoint: &GoalRoundCheckpoint<'_>) -> DirectTurnTerminalObligationInput {
    DirectTurnTerminalObligationInput {
        turn_id: TurnAuthorityId(u64::try_from(checkpoint.execution.turn_id()).unwrap()),
        expected_generation: 0,
        terminal: TurnTerminal::Completed,
        projection: PersistedConversationProjection {
            state: checkpoint.state.clone(),
            state_updated_at: checkpoint.state_updated_at,
        },
        response_message_id: None,
    }
}

async fn persist(
    owner: Owner,
    db: &Database,
    checkpoint: &GoalRoundCheckpoint<'_>,
) -> DbResult<(Vec<Message>, bool)> {
    let committed = match owner {
        Owner::Normal => db.persist_goal_tool_round_and_state(checkpoint).await?,
        Owner::Anthropic | Owner::Responses => {
            db.persist_goal_tool_round_state_and_replay(
                checkpoint,
                &replay_update(owner, checkpoint.assistant),
            )
            .await?
        }
        Owner::Fork | Owner::ForkAnthropic | Owner::ForkResponses => {
            let update = owner
                .replay()
                .map(|provider| replay_update(provider, checkpoint.assistant));
            db.persist_goal_fork_tool_round(
                checkpoint,
                &proposal(checkpoint.assistant),
                update.as_ref(),
            )
            .await?
        }
        Owner::Terminal | Owner::TerminalAnthropic | Owner::TerminalResponses => {
            let update = owner
                .replay()
                .map(|provider| replay_update(provider, checkpoint.assistant));
            let committed = db
                .persist_goal_terminal_tool_round(
                    checkpoint,
                    &obligation(checkpoint),
                    update.as_ref(),
                )
                .await?;
            return Ok((committed.results, committed.replayed));
        }
    };
    Ok((committed.results, committed.replayed))
}

fn selected_state(owner: Owner) -> ConvState {
    match owner {
        Owner::Terminal | Owner::TerminalAnthropic | Owner::TerminalResponses => ConvState::Idle,
        Owner::Normal
        | Owner::Anthropic
        | Owner::Responses
        | Owner::Fork
        | Owner::ForkAnthropic
        | Owner::ForkResponses => ConvState::LlmRequesting { attempt: 2 },
    }
}

async fn assert_no_round(db: &Database, report: &GoalPendingReport) {
    let expected = if db.goal_control_installation()
        == crate::conversation_goals::control::GoalControlInstallation::Installed
    {
        2
    } else {
        1
    };
    assert_eq!(db.get_messages("full-root").await.unwrap().len(), expected);
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
        db.get_conversation("full-root").await.unwrap().state,
        ConvState::LlmRequesting { attempt: 1 }
    );
    assert!(db
        .load_provider_replay_state("full-root")
        .await
        .unwrap()
        .is_none());
    assert!(db
        .load_responses_replay_state("full-root")
        .await
        .unwrap()
        .is_empty());
    let side_rows: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM fork_proposals)+(SELECT count(*) FROM direct_turn_terminal_obligations)").fetch_one(db.pool()).await.unwrap();
    assert_eq!(side_rows, 0);
}

async fn assert_owner_replay_state(owner: Owner, db: &Database) {
    if matches!(owner.replay(), Some(Owner::Anthropic)) {
        assert_eq!(
            db.load_provider_replay_state("full-root")
                .await
                .unwrap()
                .unwrap()
                .response_sets
                .len(),
            1
        );
    }
    if matches!(owner.replay(), Some(Owner::Responses)) {
        assert_eq!(
            db.load_responses_replay_state("full-root")
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

async fn terminalize_goal_owner_fixture(db: &Database, checkpoint: &GoalRoundCheckpoint<'_>) {
    db.workflow_repository()
        .terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Complete {
                turn_id: TurnAuthorityId(1),
                expected_generation: 0,
            },
            projection: Some(obligation(checkpoint).projection),
            provider_replay_settlement: ProviderReplaySettlement::Preserve,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn all_actual_goal_round_owners_commit_and_exactly_replay_without_rewinding_state() {
    for owner in OWNERS {
        let (db, assistant, report) = admitted_fixture().await;
        let results = [ToolRoundResult::PendingGoalReport {
            report: report.clone(),
            duration_ms: 7,
        }];
        let state = selected_state(owner);
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 1).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        let (original, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
        assert!(!replayed, "{owner:?}");
        let goal = db
            .conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(goal.state, GoalState::Complete(_)), "{owner:?}");
        let MessageContent::Tool(result) = &original[0].content else {
            panic!("finalized")
        };
        assert!(!result.is_error, "{owner:?}");
        if owner.terminal() {
            assert_eq!(
                db.get_conversation("full-root").await.unwrap().state,
                ConvState::LlmRequesting { attempt: 1 },
                "terminal obligation must not project Idle early"
            );
            terminalize_goal_owner_fixture(&db, &checkpoint).await;
            assert_eq!(
                db.get_conversation("full-root").await.unwrap().state,
                ConvState::Idle
            );
            let (settled_replay, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
            assert!(replayed);
            assert_eq!(
                serde_json::to_value(settled_replay).unwrap(),
                serde_json::to_value(&original).unwrap()
            );
        } else {
            assert_eq!(
                db.get_conversation("full-root").await.unwrap().state,
                state,
                "{owner:?}"
            );
        }
        assert_owner_replay_state(owner, &db).await;
        let later = ConvState::LlmRequesting { attempt: 3 };
        db.update_conversation_state("full-root", &later)
            .await
            .unwrap();
        if owner.terminal() {
            assert!(
                persist(owner, &db, &checkpoint).await.is_err(),
                "a consumed obligation cannot prove a changed projection"
            );
        } else {
            let (retained, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
            assert!(replayed, "{owner:?}");
            assert_eq!(
                serde_json::to_value(&retained).unwrap(),
                serde_json::to_value(&original).unwrap(),
                "{owner:?}"
            );
        }
        assert_eq!(
            db.get_conversation("full-root").await.unwrap().state,
            later,
            "{owner:?}"
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
        assert_eq!(db.get_messages("full-root").await.unwrap().len(), 3);
    }
}

#[tokio::test]
async fn conversation_goal_installed_control_fences_every_actual_report_round_owner() {
    use crate::conversation_goals::control::{ControlCommand, GoalControlInstallation};
    for owner in OWNERS {
        for stopped in [false, true] {
            let (db, assistant, report) =
                admitted_fixture_with_control(GoalControlInstallation::Installed).await;
            if stopped {
                control_command(&db, &report.mutation().owner, ControlCommand::Stop, 1).await;
            }
            let results = [ToolRoundResult::PendingGoalReport {
                report: report.clone(),
                duration_ms: 7,
            }];
            let state = selected_state(owner);
            let checkpoint = GoalRoundCheckpoint {
                assistant: &assistant,
                execution: report.execution(),
                results: &results,
                state: &state,
                state_updated_at: Utc::now(),
            };
            let (messages, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
            assert!(!replayed, "{owner:?}");
            let MessageContent::Tool(result) = &messages[0].content else {
                panic!("paired report")
            };
            assert_eq!(result.is_error, stopped, "{owner:?}");
            let goal = db
                .conversation_goal(&report.mutation().owner)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(goal.version.get(), if stopped { 1 } else { 2 }, "{owner:?}");
            assert_eq!(
                matches!(goal.state, GoalState::Complete(_)),
                !stopped,
                "{owner:?}"
            );
            if stopped {
                control_command(&db, &report.mutation().owner, ControlCommand::Release, 2).await;
            } else {
                control_command(&db, &report.mutation().owner, ControlCommand::Stop, 1).await;
            }
            let (retained, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
            assert!(replayed, "{owner:?}");
            assert_eq!(
                serde_json::to_value(&retained).unwrap(),
                serde_json::to_value(&messages).unwrap(),
                "{owner:?}"
            );
            assert_owner_replay_state(owner, &db).await;
        }
    }
}

#[tokio::test]
async fn conversation_goal_installed_missing_control_schema_rolls_back_every_report_owner() {
    use crate::conversation_goals::control::GoalControlInstallation;
    for owner in OWNERS {
        let (db, assistant, report) =
            admitted_fixture_with_control(GoalControlInstallation::Installed).await;
        sqlx::query("DROP TABLE conversation_goal_control_bindings")
            .execute(db.pool())
            .await
            .unwrap();
        let results = [ToolRoundResult::PendingGoalReport {
            report: report.clone(),
            duration_ms: 7,
        }];
        let state = selected_state(owner);
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: report.execution(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        assert!(persist(owner, &db, &checkpoint).await.is_err(), "{owner:?}");
        assert_no_round(&db, &report).await;
    }
}

#[tokio::test]
async fn every_goal_round_owner_rolls_back_report_messages_state_and_side_rows_on_infra_failure() {
    for owner in OWNERS {
        let (db, assistant, report) = admitted_fixture().await;
        let query = match owner {
            Owner::Normal => sqlx::query(
                "CREATE TRIGGER fail_owner BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'owner write failed'); END",
            ),
            Owner::Anthropic => sqlx::query(
                "CREATE TRIGGER fail_owner BEFORE INSERT ON active_provider_replay_state BEGIN SELECT RAISE(ABORT,'owner write failed'); END",
            ),
            Owner::Responses => sqlx::query(
                "CREATE TRIGGER fail_owner BEFORE INSERT ON active_responses_replay_sets BEGIN SELECT RAISE(ABORT,'owner write failed'); END",
            ),
            Owner::Fork | Owner::ForkAnthropic | Owner::ForkResponses => sqlx::query(
                "CREATE TRIGGER fail_owner BEFORE INSERT ON fork_proposals BEGIN SELECT RAISE(ABORT,'owner write failed'); END",
            ),
            Owner::Terminal | Owner::TerminalAnthropic | Owner::TerminalResponses => sqlx::query(
                "CREATE TRIGGER fail_owner BEFORE INSERT ON direct_turn_terminal_obligations BEGIN SELECT RAISE(ABORT,'owner write failed'); END",
            ),
        };
        query.execute(db.pool()).await.unwrap();
        let results = [ToolRoundResult::PendingGoalReport {
            report: report.clone(),
            duration_ms: 7,
        }];
        let state = selected_state(owner);
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 1).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        let error = persist(owner, &db, &checkpoint).await.unwrap_err();
        assert!(
            error.to_string().contains("owner write failed"),
            "{owner:?}: {error}"
        );
        assert_no_round(&db, &report).await;
        sqlx::query("DROP TRIGGER fail_owner")
            .execute(db.pool())
            .await
            .unwrap();
        let (_, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
        assert!(!replayed, "{owner:?}");
    }
}

#[tokio::test]
async fn all_goal_owners_preserve_other_results_and_committed_semantic_refusals() {
    for owner in OWNERS {
        let (db, mut assistant, report) = admitted_fixture().await;
        let GoalChange::Report { binding, .. } = report.mutation().change else {
            panic!("report")
        };
        let first = GoalPendingReport::new(
            binding.try_into().unwrap(),
            GoalReportCall {
                transcript_id: t("full-root"),
                message_id: t("owner-assistant"),
                tool_use_id: t("first-report"),
            },
            GoalReportOutcome::Blocked(t("access unavailable; restore access")),
        );
        let MessageContent::Agent(blocks) = &mut assistant.content else {
            panic!("assistant")
        };
        blocks.insert(
            0,
            ContentBlock::ToolUse {
                id: "first-report".into(),
                name: "goal_report".into(),
                input: serde_json::to_value(first.outcome()).unwrap(),
            },
        );
        blocks.insert(
            1,
            ContentBlock::ToolUse {
                id: "other".into(),
                name: "think".into(),
                input: serde_json::json!({"thoughts":"retain me"}),
            },
        );
        let results = [
            ToolRoundResult::PendingGoalReport {
                report: first,
                duration_ms: 1,
            },
            ToolRoundResult::Finalized(ToolResult::success(
                "other".into(),
                "unrelated result".into(),
            )),
            ToolRoundResult::PendingGoalReport {
                report: report.clone(),
                duration_ms: 2,
            },
        ];
        let state = selected_state(owner);
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 1).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        let (original, _) = persist(owner, &db, &checkpoint).await.unwrap();
        for (index, expected_error) in [(0, false), (1, false), (2, true)] {
            let MessageContent::Tool(content) = &original[index].content else {
                panic!("result")
            };
            assert_eq!(content.is_error, expected_error, "{owner:?}");
        }
        let MessageContent::Tool(other) = &original[1].content else {
            panic!("other")
        };
        assert_eq!(other.content, "unrelated result");
        assert!(matches!(
            db.conversation_goal(&report.mutation().owner)
                .await
                .unwrap()
                .unwrap()
                .state,
            GoalState::Blocked(_)
        ));
        let (retained, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
        assert!(replayed, "{owner:?}");
        assert_eq!(
            serde_json::to_value(&retained).unwrap(),
            serde_json::to_value(&original).unwrap(),
            "{owner:?}"
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
    }
}

#[tokio::test]
async fn all_goal_round_owners_refuse_changed_arguments_and_source_on_first_write_and_retry() {
    for owner in OWNERS {
        let (db, assistant, report) = admitted_fixture().await;
        let results = [ToolRoundResult::PendingGoalReport {
            report: report.clone(),
            duration_ms: 7,
        }];
        let state = selected_state(owner);
        let timestamp: DateTime<Utc> = Utc::now();
        let mut altered = assistant.clone();
        let MessageContent::Agent(blocks) = &mut altered.content else {
            panic!("assistant")
        };
        let ContentBlock::ToolUse { input, .. } = &mut blocks[0] else {
            panic!("call")
        };
        *input =
            serde_json::to_value(GoalReportOutcome::Complete(t("different arguments"))).unwrap();
        let invalid = GoalRoundCheckpoint {
            assistant: &altered,
            execution: GoalExecution::parse(1, 1).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: timestamp,
        };
        assert!(persist(owner, &db, &invalid).await.is_err(), "{owner:?}");
        assert_no_round(&db, &report).await;
        let valid = GoalRoundCheckpoint {
            assistant: &assistant,
            ..invalid
        };
        persist(owner, &db, &valid).await.unwrap();
        assert!(persist(owner, &db, &invalid).await.is_err(), "{owner:?}");
        let mut other_source = assistant.clone();
        other_source.origin = InputOrigin::UnknownHistorical;
        let invalid_source = GoalRoundCheckpoint {
            assistant: &other_source,
            ..valid
        };
        assert!(
            persist(owner, &db, &invalid_source).await.is_err(),
            "{owner:?}"
        );
        assert_eq!(db.get_messages("full-root").await.unwrap().len(), 3);
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
}

#[tokio::test]
async fn combined_goal_round_replay_failures_roll_back_existing_side_rows_and_state() {
    for owner in [
        Owner::ForkAnthropic,
        Owner::ForkResponses,
        Owner::TerminalAnthropic,
        Owner::TerminalResponses,
    ] {
        let (db, assistant, report) = admitted_fixture().await;
        let query = match owner.replay().unwrap() {
            Owner::Anthropic => sqlx::query("CREATE TRIGGER fail_replay BEFORE INSERT ON active_provider_replay_state BEGIN SELECT RAISE(ABORT,'combined replay failure'); END"),
            Owner::Responses => sqlx::query("CREATE TRIGGER fail_replay BEFORE INSERT ON active_responses_replay_sets BEGIN SELECT RAISE(ABORT,'combined replay failure'); END"),
            Owner::Normal | Owner::Fork | Owner::Terminal | Owner::ForkAnthropic | Owner::ForkResponses | Owner::TerminalAnthropic | Owner::TerminalResponses => unreachable!("provider only"),
        };
        query.execute(db.pool()).await.unwrap();
        let results = [ToolRoundResult::PendingGoalReport {
            report: report.clone(),
            duration_ms: 7,
        }];
        let state = selected_state(owner);
        let checkpoint = GoalRoundCheckpoint {
            assistant: &assistant,
            execution: GoalExecution::parse(1, 1).unwrap(),
            results: &results,
            state: &state,
            state_updated_at: Utc::now(),
        };
        let error = persist(owner, &db, &checkpoint).await.unwrap_err();
        assert!(
            error.to_string().contains("combined replay failure"),
            "{owner:?}: {error}"
        );
        assert_no_round(&db, &report).await;
        sqlx::query("DROP TRIGGER fail_replay")
            .execute(db.pool())
            .await
            .unwrap();
        let (original, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
        assert!(!replayed, "{owner:?}");
        let (retained, replayed) = persist(owner, &db, &checkpoint).await.unwrap();
        assert!(replayed, "{owner:?}");
        assert_eq!(
            serde_json::to_value(original).unwrap(),
            serde_json::to_value(retained).unwrap()
        );
        assert_eq!(
            db.get_conversation("full-root").await.unwrap().state,
            if owner.terminal() {
                ConvState::LlmRequesting { attempt: 1 }
            } else {
                state
            }
        );
    }
}

#[tokio::test]
async fn terminal_goal_round_commit_cuts_use_existing_obligation_and_never_project_idle_early() {
    let (db, assistant, report) = admitted_fixture().await;
    let results = [ToolRoundResult::PendingGoalReport {
        report: report.clone(),
        duration_ms: 7,
    }];
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &ConvState::Idle,
        state_updated_at: Utc::now(),
    };
    let terminal = obligation(&checkpoint);
    for cut in [
        crate::TerminalEvidenceTransactionCut::BeforeCommit,
        crate::TerminalEvidenceTransactionCut::AfterCommit,
    ] {
        assert!(db
            .persist_terminal_tool_round_input(
                "full-root",
                &assistant,
                ToolRoundWrite::Pending {
                    execution: checkpoint.execution,
                    results: &results
                },
                &terminal,
                cut,
                None,
            )
            .await
            .is_err());
        if cut == crate::TerminalEvidenceTransactionCut::BeforeCommit {
            assert_no_round(&db, &report).await;
        }
    }
    let (_, replayed) = persist(Owner::Terminal, &db, &checkpoint).await.unwrap();
    assert!(replayed);
    assert_eq!(
        db.get_conversation("full-root").await.unwrap().state,
        ConvState::LlmRequesting { attempt: 1 }
    );
    let terminal_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM direct_turn_terminal_obligations WHERE turn_id=1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(terminal_rows, 1);
    let mut changed = terminal.clone();
    changed.projection.state = ConvState::LlmRequesting { attempt: 5 };
    assert!(db
        .persist_goal_terminal_tool_round(&checkpoint, &changed, None)
        .await
        .is_err());
}
