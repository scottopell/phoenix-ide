use super::*;
use crate::conversation_goals::request_checkpoint::{
    GoalRequestCheckpoint, GoalRequestCheckpointOutcome, GoalRequestEvidence,
};

fn text(value: &str) -> phoenix_core::domain::conversation_goal::GoalText {
    phoenix_core::domain::conversation_goal::GoalText::parse(value).unwrap()
}

fn response(input: &GoalStartAdoptionInput) -> crate::Message {
    crate::Message {
        message_id: input.intent().unwrap().id.as_str().to_owned(),
        conversation_id: input.receipt.historical_transcript().as_str().to_owned(),
        sequence_id: 1,
        message_type: crate::MessageType::Agent,
        content: crate::MessageContent::Agent(vec![
            phoenix_core::domain::llm_types::ContentBlock::text("canonical answer"),
        ]),
        display_data: None,
        usage_data: None,
        origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
        created_at: input.state_updated_at + chrono::Duration::seconds(1),
    }
}

async fn counts(db: &Database) -> (i64, i64, i64, i64) {
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
async fn conversation_goal_canonical_response_checkpoint_commit_cuts_and_exact_replay() {
    for after_commit in [false, true] {
        let (db, input) = materialized().await;
        let admitted = context(&db, &input).await.unwrap().unwrap().into_admitted();
        let assistant = response(&input);
        let source = ConvState::LlmRequesting { attempt: 1 };
        let state = ConvState::Idle;
        let stamp = input.state_updated_at + chrono::Duration::seconds(2);
        let command = GoalRequestCheckpoint {
            admitted: &admitted,
            source: &source,
            source_updated_at: input.state_updated_at,
            assistant: &assistant,
            evidence: GoalRequestEvidence::Response,
            state: &state,
            state_updated_at: stamp,
            provider_replay: None,
        };
        let before = counts(&db).await;
        let result = db
            .commit_goal_request_checkpoint_test_cut(&command, after_commit)
            .await;
        if after_commit {
            assert!(
                matches!(result.unwrap(), GoalRequestCheckpointOutcome::Committed(committed) if committed.replayed)
            );
        } else {
            assert!(result.is_err());
            assert_eq!(counts(&db).await, before);
            assert_eq!(
                db.get_conversation(&assistant.conversation_id)
                    .await
                    .unwrap()
                    .state,
                source
            );
            assert!(
                matches!(db.commit_goal_request_checkpoint(&command).await.unwrap(),
                GoalRequestCheckpointOutcome::Committed(committed) if !committed.replayed)
            );
        }
        assert_eq!(
            counts(&db).await,
            (before.0 + 1, before.1, before.2 + 1, before.3)
        );
        let restored = db
            .get_conversation(&assistant.conversation_id)
            .await
            .unwrap();
        assert_eq!(restored.state, state);
        assert_eq!(restored.state_updated_at, stamp);
        assert_eq!(restored.message_count, 1);
        assert!(
            matches!(db.commit_goal_request_checkpoint(&command).await.unwrap(),
            GoalRequestCheckpointOutcome::Committed(committed) if committed.replayed)
        );
        assert_eq!(
            counts(&db).await,
            (before.0 + 1, before.1, before.2 + 1, before.3)
        );
        let (owns, terminal): (i64, Option<String>) = sqlx::query_as(
            "SELECT owns_conversation,terminal_kind FROM durable_turns WHERE turn_id=?",
        )
        .bind(input.receipt.execution().turn_id())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            (owns, terminal),
            (1, None),
            "answer checkpoint retains same execution"
        );
    }
}

#[tokio::test]
async fn conversation_goal_canonical_state_failure_rolls_back_response_checkpoint_and_fts() {
    let (db, input) = materialized().await;
    let admitted = context(&db, &input).await.unwrap().unwrap().into_admitted();
    let assistant = response(&input);
    let source = ConvState::LlmRequesting { attempt: 1 };
    let before = counts(&db).await;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_goal_selected_state BEFORE UPDATE OF state ON conversations
        BEGIN SELECT RAISE(ABORT,'injected canonical state failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(db
        .commit_goal_request_checkpoint(&GoalRequestCheckpoint {
            admitted: &admitted,
            source: &source,
            source_updated_at: input.state_updated_at,
            assistant: &assistant,
            evidence: GoalRequestEvidence::Response,
            state: &ConvState::Idle,
            state_updated_at: assistant.created_at,
            provider_replay: None,
        })
        .await
        .is_err());
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.get_conversation(&assistant.conversation_id)
            .await
            .unwrap()
            .state,
        source
    );
    let indexed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM message_fts_rows WHERE message_id=?")
            .bind(&assistant.message_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(indexed, 0);
}

#[tokio::test]
async fn conversation_goal_canonical_obsolete_owner_or_slot_cannot_publish_response() {
    for fence in ["slot", "reconstruction", "queue", "stop"] {
        let (db, input) = materialized().await;
        let admitted = context(&db, &input).await.unwrap().unwrap().into_admitted();
        let assistant = response(&input);
        let source = ConvState::LlmRequesting { attempt: 1 };
        match fence {
            "slot" => db
                .update_conversation_state_at(
                    &assistant.conversation_id,
                    &source,
                    input.state_updated_at + chrono::Duration::microseconds(1),
                )
                .await
                .unwrap(),
            "reconstruction" => {
                assert!(matches!(
                    db.workflow_repository()
                        .recover_goal_start_runtime(&crate::workflow::GoalStartRecoveryInput::new(
                            assistant.conversation_id.clone(),
                            (
                                TurnAuthorityId(
                                    u64::try_from(input.receipt.execution().turn_id()).unwrap()
                                ),
                                input.authority.generation.0
                            ),
                            (source.clone(), input.state_updated_at),
                            Timestamp(30),
                        ))
                        .await
                        .unwrap(),
                    crate::workflow::GoalStartRecoveryOutcome::Recovered(_)
                ));
            }
            "queue" => {
                let entry = phoenix_core::domain::sm_event::SteerEntry {
                    message_id: "accepted-checkpoint-input".into(),
                    text: "accepted input".into(),
                    origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: None,
                    skill_invocation: None,
                };
                db.append_steering_entry(
                    &assistant.conversation_id,
                    &entry,
                    "checkpoint-input-fingerprint",
                )
                .await
                .unwrap();
            }
            "stop" => {
                use crate::conversation_goals::control::{ControlCommand, ControlRequest};
                db.apply_goal_control(
                    &ControlRequest {
                        owner: input.receipt.input().goal().owner.clone(),
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
            _ => unreachable!(),
        }
        let before = counts(&db).await;
        let retained = db
            .get_conversation(&assistant.conversation_id)
            .await
            .unwrap();
        assert!(
            matches!(
                db.commit_goal_request_checkpoint(&GoalRequestCheckpoint {
                    admitted: &admitted,
                    source: &source,
                    source_updated_at: input.state_updated_at,
                    assistant: &assistant,
                    evidence: GoalRequestEvidence::Response,
                    state: &ConvState::Idle,
                    state_updated_at: assistant.created_at,
                    provider_replay: None,
                })
                .await
                .unwrap(),
                GoalRequestCheckpointOutcome::Refused
            ),
            "{fence}"
        );
        assert_eq!(counts(&db).await, before, "{fence}");
        let current = db
            .get_conversation(&assistant.conversation_id)
            .await
            .unwrap();
        assert_eq!(
            (current.state, current.state_updated_at),
            (retained.state, retained.state_updated_at),
            "{fence}"
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_complete_report_checkpoint_cuts_replay_whole_bundle_after_bound_goal_changes(
) {
    use phoenix_core::domain::{
        conversation_goal::{
            GoalPendingReport, GoalReportCall, GoalReportOutcome, ToolRoundResult,
        },
        llm_types::{ContentBlock, Usage},
        sm_state::{AssistantMessage, ToolCall, ToolInput},
    };
    for after_commit in [false, true] {
        let (db, input) = materialized().await;
        let context = context(&db, &input).await.unwrap().unwrap();
        let binding = context.admitted().report_binding().unwrap();
        let admitted = context.into_admitted();
        let outcome = GoalReportOutcome::Complete(text("committed complete report"));
        let tool_input = serde_json::to_value(&outcome).unwrap();
        let block = ContentBlock::tool_use("complete-call", "goal_report", tool_input.clone());
        let mut assistant = response(&input);
        assistant.content = crate::MessageContent::Agent(vec![block.clone()]);
        assistant.usage_data = Some(Usage::default());
        let source_assistant = AssistantMessage {
            message_id: assistant.message_id.clone(),
            content: vec![block],
            usage: assistant.usage_data.clone(),
            display_data: None,
            created_at: assistant.created_at,
        };
        let source = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "complete-call",
                ToolInput::from_name_and_value("goal_report", tool_input),
            ),
            remaining_tools: Vec::new(),
            completed_results: Vec::new(),
            assistant_message: source_assistant,
            pending_sub_agents: Vec::new(),
        };
        let source_stamp = input.state_updated_at + chrono::Duration::seconds(2);
        db.update_conversation_state_at(&assistant.conversation_id, &source, source_stamp)
            .await
            .unwrap();
        let results = [ToolRoundResult::PendingGoalReport {
            report: GoalPendingReport::new(
                binding,
                GoalReportCall {
                    transcript_id: text(&assistant.conversation_id),
                    message_id: text(&assistant.message_id),
                    tool_use_id: text("complete-call"),
                },
                outcome,
            ),
            duration_ms: 1,
        }];
        let state = ConvState::Idle;
        let command = GoalRequestCheckpoint {
            admitted: &admitted,
            source: &source,
            source_updated_at: source_stamp,
            assistant: &assistant,
            evidence: GoalRequestEvidence::Round {
                results: &results,
                management_targets: &[],
            },
            state: &state,
            state_updated_at: source_stamp + chrono::Duration::seconds(1),
            provider_replay: None,
        };
        let before = counts(&db).await;
        let initial = db
            .conversation_goal(&input.receipt.input().goal().owner)
            .await
            .unwrap()
            .unwrap();
        let cut = db
            .commit_goal_request_checkpoint_test_cut(&command, after_commit)
            .await;
        if after_commit {
            assert!(
                matches!(cut.unwrap(), GoalRequestCheckpointOutcome::Committed(committed) if committed.replayed)
            );
        } else {
            assert!(cut.is_err());
            assert_eq!(counts(&db).await, before);
            assert_eq!(
                db.conversation_goal(&initial.owner).await.unwrap().unwrap(),
                initial
            );
            assert!(
                matches!(db.commit_goal_request_checkpoint(&command).await.unwrap(),
                GoalRequestCheckpointOutcome::Committed(committed) if !committed.replayed)
            );
        }
        let complete = db.conversation_goal(&initial.owner).await.unwrap().unwrap();
        assert_eq!(complete.version.get(), initial.version.get() + 1);
        assert!(matches!(
            complete.state,
            phoenix_core::domain::conversation_goal::GoalState::Complete(_)
        ));
        let persisted = counts(&db).await;
        assert_eq!(persisted, (before.0 + 2, before.1, before.2 + 1, before.3));
        assert!(
            matches!(db.commit_goal_request_checkpoint(&command).await.unwrap(),
            GoalRequestCheckpointOutcome::Committed(committed) if committed.replayed)
        );
        assert_eq!(counts(&db).await, persisted);
        assert_eq!(
            db.conversation_goal(&initial.owner).await.unwrap().unwrap(),
            complete
        );
    }
}
