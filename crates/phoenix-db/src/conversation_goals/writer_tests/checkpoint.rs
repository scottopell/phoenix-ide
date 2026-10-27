use super::*;
use crate::conversation_goals::round::{CommittedGoalRound, GoalRoundCheckpoint, ToolRoundWrite};
use crate::provider_replay::CanonicalCommitCut;
use chrono::{DateTime, Duration, Utc};
use phoenix_core::domain::conversation_goal::{
    GoalPendingReport, GoalReportCall, GoalReportOutcome, ToolRoundResult,
};
use phoenix_core::domain::db_schema::{
    InputOrigin, Message, MessageContent, MessageType, ToolResult,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::provider_replay::{
    AnthropicPrivateBlock, AnthropicResponseIdentity, AnthropicResponseSet, ContentIndex,
    ProviderReplayUpdate,
};
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::domain::tool_result_identity::tool_result_message_id;

mod canonical;
mod terminal;

struct ReportRound {
    assistant: Message,
    execution: GoalExecution,
    results: Vec<ToolRoundResult>,
    state: ConvState,
    timestamp: DateTime<Utc>,
}

impl ReportRound {
    fn checkpoint(&self) -> GoalRoundCheckpoint<'_> {
        GoalRoundCheckpoint {
            assistant: &self.assistant,
            execution: self.execution,
            results: &self.results,
            state: &self.state,
            state_updated_at: self.timestamp,
        }
    }

    fn replay(&self) -> ProviderReplayUpdate {
        let MessageContent::Agent(blocks) = &self.assistant.content else {
            panic!("fixture requires assistant blocks");
        };
        ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: format!("response-{}", self.assistant.message_id),
                    model: "checkpoint-model".into(),
                },
                blocks.clone(),
                vec![AnthropicPrivateBlock::Thinking {
                    index: ContentIndex(0),
                    thinking: "checkpoint-private-evidence".into(),
                    signature: "checkpoint-signature".into(),
                }],
            )
            .unwrap()
            .with_owner_message_id(self.assistant.message_id.clone()),
        )
    }

    async fn commit(&self, db: &Database) -> crate::DbResult<CommittedGoalRound> {
        db.persist_goal_tool_round_state_and_replay(&self.checkpoint(), &self.replay())
            .await
    }

    async fn cut(
        &self,
        db: &Database,
        cut: CanonicalCommitCut,
    ) -> crate::DbResult<CommittedGoalRound> {
        db.persist_canonical_state_and_replay_input(
            &self.assistant.conversation_id,
            &self.assistant,
            ToolRoundWrite::Pending {
                execution: self.execution,
                results: &self.results,
            },
            &self.state,
            self.timestamp,
            Some(&self.replay()),
            Some(cut),
        )
        .await
    }
}

async fn report_fixture() -> (Database, ProductConversationId, ReportRound) {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "checkpoint-owner").await;
    db.watch_product_conversation(&owner).await.unwrap();
    db.apply_conversation_goal(
        &create(
            &owner,
            GoalVersion::INITIAL,
            "checkpoint-goal",
            "checkpoint-create",
        ),
        &transcript,
        at(),
    )
    .await
    .unwrap();
    release(&db, &owner).await;
    let execution = accept_execution(
        &db.workflow_repository(),
        &runtime_input(&transcript, "checkpoint-turn"),
    )
    .await;
    let binding = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    let outcome = GoalReportOutcome::Complete(GoalText::parse("verified done").unwrap());
    let assistant = Message {
        message_id: "checkpoint-assistant".into(),
        conversation_id: transcript.clone(),
        sequence_id: 10,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: "checkpoint-call".into(),
            name: "goal_report".into(),
            input: serde_json::to_value(&outcome).unwrap(),
        }]),
        display_data: None,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: Utc::now(),
    };
    let report = GoalPendingReport::new(
        binding.try_into().unwrap(),
        GoalReportCall {
            transcript_id: GoalText::parse(transcript).unwrap(),
            message_id: GoalText::parse(&assistant.message_id).unwrap(),
            tool_use_id: GoalText::parse("checkpoint-call").unwrap(),
        },
        outcome,
    );
    let round = ReportRound {
        assistant,
        execution,
        results: vec![ToolRoundResult::PendingGoalReport {
            report,
            duration_ms: 17,
        }],
        state: ConvState::LlmRequesting { attempt: 1 },
        timestamp: Utc::now() + Duration::seconds(10),
    };
    (db, owner, round)
}

async fn evidence(
    db: &Database,
    owner: &ProductConversationId,
    round: &ReportRound,
) -> serde_json::Value {
    let transcript = &round.assistant.conversation_id;
    let state: (String, String, String, String) = sqlx::query_as(
        "SELECT state,state_kind,state_updated_at,updated_at FROM conversations WHERE id=?",
    )
    .bind(transcript)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let obligations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM direct_turn_terminal_obligations")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let receipts: Vec<(String, Vec<u8>, String, String, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT assistant_message_id,selected_state_sha256,selected_state_updated_at,replay_kind,replay_payload_sha256
         FROM canonical_tool_round_receipts WHERE conversation_id=? ORDER BY assistant_message_id",
    ).bind(transcript).fetch_all(db.pool()).await.unwrap();
    let replay_head: Option<(String, String, String)> = sqlx::query_as(
        "SELECT model,response_id,payload FROM active_provider_replay_state WHERE conversation_id=?",
    ).bind(transcript).fetch_optional(db.pool()).await.unwrap();
    serde_json::json!({
        "goal": db.conversation_goal(owner).await.unwrap(),
        "history": db.conversation_goal_history(owner, GoalVersion::INITIAL).await.unwrap(),
        "facts": counts(db).await,
        "messages": db.get_messages(transcript).await.unwrap(),
        "state": state,
        "replay": db.load_provider_replay_state(transcript).await.unwrap(),
        "obligations": obligations,
        "receipts": receipts,
        "replay_head": replay_head,
    })
}

async fn replace_goal(db: &Database, owner: &ProductConversationId, round: &ReportRound, id: &str) {
    let current = db.conversation_goal(owner).await.unwrap().unwrap();
    let mutation = create(owner, current.version(), id, &format!("replace-{id}"));
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    apply_goal_tx(
        &mut tx,
        &mutation,
        None,
        GoalTimestampUs::parse((round.timestamp + Duration::seconds(1)).timestamp_micros())
            .unwrap(),
        db.goal_write_policies(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn assert_result(result: &Message, is_error: bool) {
    let MessageContent::Tool(content) = &result.content else {
        panic!("committed result must be a tool message");
    };
    assert_eq!(content.tool_use_id, "checkpoint-call");
    assert_eq!(content.is_error, is_error);
    assert_eq!(result.display_data.as_ref().unwrap()["duration_ms"], 17);
    assert_eq!(
        result.message_id,
        tool_result_message_id("checkpoint-assistant", "checkpoint-call")
    );
    if is_error {
        assert!(content.content.starts_with("Goal report rejected:"));
    } else {
        assert!(content.content.contains("recorded complete"));
    }
}

#[tokio::test]
async fn accepted_report_commits_history_result_state_and_private_replay_together() {
    let (db, owner, round) = report_fixture().await;
    let before = evidence(&db, &owner, &round).await;
    let committed = round.commit(&db).await.unwrap();
    assert!(!committed.replayed);
    assert_eq!(committed.results.len(), 1);
    assert_result(&committed.results[0], false);
    assert_eq!(committed.state, round.state);
    assert_eq!(committed.state_updated_at, round.timestamp);
    let goal = db.conversation_goal(&owner).await.unwrap().unwrap();
    assert_eq!(
        goal.state(),
        &GoalState::Complete(GoalText::parse("verified done").unwrap())
    );
    let history = db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history.last().unwrap().snapshot(), &goal);
    let after = evidence(&db, &owner, &round).await;
    assert_eq!(
        after["messages"].as_array().unwrap().len(),
        before["messages"].as_array().unwrap().len() + 2
    );
    assert_eq!(
        after["state"][0],
        serde_json::to_string(&round.state).unwrap()
    );
    assert_eq!(after["state"][2], round.timestamp.to_rfc3339());
    let ProviderReplayUpdate::Anthropic(expected) = round.replay() else {
        unreachable!()
    };
    assert_eq!(
        db.load_provider_replay_state(&round.assistant.conversation_id)
            .await
            .unwrap()
            .unwrap()
            .response_sets,
        vec![expected]
    );
}

#[tokio::test]
async fn stale_report_commits_paired_refusal_without_goal_or_history_mutation() {
    let (db, owner, round) = report_fixture().await;
    replace_goal(&db, &owner, &round, "stale-replacement").await;
    let before = evidence(&db, &owner, &round).await;
    let committed = round.commit(&db).await.unwrap();
    assert!(!committed.replayed);
    assert_result(&committed.results[0], true);
    let after = evidence(&db, &owner, &round).await;
    for field in ["goal", "history", "facts", "obligations"] {
        assert_eq!(after[field], before[field], "{field}");
    }
    assert_eq!(
        after["messages"].as_array().unwrap().len(),
        before["messages"].as_array().unwrap().len() + 2
    );
    assert_eq!(
        after["state"][0],
        serde_json::to_string(&round.state).unwrap()
    );
    assert!(after["replay"].is_object());
}

#[tokio::test]
async fn retained_success_and_refusal_preserve_advanced_goal_state_and_replay() {
    for refused in [false, true] {
        let (db, owner, round) = report_fixture().await;
        if refused {
            replace_goal(&db, &owner, &round, "refusal-replacement").await;
        }
        let first = round.commit(&db).await.unwrap();
        assert_result(&first.results[0], refused);
        replace_goal(&db, &owner, &round, "latest-goal").await;
        let latest_state = ConvState::LlmRequesting { attempt: 9 };
        let latest_time = round.timestamp + Duration::seconds(2);
        db.update_state_and_replay(
            &round.assistant.conversation_id,
            &latest_state,
            latest_time,
            &ProviderReplayUpdate::Clear,
        )
        .await
        .unwrap();
        let ProviderReplayUpdate::Anthropic(mut latest_set) = round.replay() else {
            unreachable!()
        };
        latest_set.identity.response_id = "latest-response".into();
        latest_set.owner_message_id = "latest-assistant".into();
        db.update_state_and_replay(
            &round.assistant.conversation_id,
            &latest_state,
            latest_time,
            &ProviderReplayUpdate::Anthropic(latest_set),
        )
        .await
        .unwrap();
        let before = evidence(&db, &owner, &round).await;
        let replay = round.commit(&db).await.unwrap();
        assert!(replay.replayed);
        assert_eq!(
            serde_json::to_value(&replay.results).unwrap(),
            serde_json::to_value(&first.results).unwrap()
        );
        assert_eq!(replay.state, latest_state);
        assert_eq!(replay.state_updated_at, latest_time);
        assert_eq!(evidence(&db, &owner, &round).await, before);
    }
}

#[tokio::test]
async fn duplicate_or_missing_round_members_reject_without_any_writes() {
    for duplicate_call in [false, true] {
        let (db, owner, mut round) = report_fixture().await;
        if duplicate_call {
            let MessageContent::Agent(blocks) = &mut round.assistant.content else {
                unreachable!()
            };
            blocks.push(blocks[0].clone());
            round.results.push(round.results[0].clone());
        } else {
            round.results.clear();
        }
        let before = evidence(&db, &owner, &round).await;
        assert!(round.commit(&db).await.is_err());
        assert_eq!(evidence(&db, &owner, &round).await, before);
    }
}

#[tokio::test]
async fn duplicate_results_reject_even_with_matching_cardinality() {
    let (db, owner, mut round) = report_fixture().await;
    let MessageContent::Agent(blocks) = &mut round.assistant.content else {
        unreachable!()
    };
    blocks.push(ContentBlock::ToolUse {
        id: "other-call".into(),
        name: "search".into(),
        input: serde_json::json!({}),
    });
    round.results.push(round.results[0].clone());
    let before = evidence(&db, &owner, &round).await;
    assert!(round.commit(&db).await.is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn changed_retained_assistant_or_report_arguments_reject_atomically() {
    for change_assistant in [false, true] {
        let (db, owner, mut round) = report_fixture().await;
        round.commit(&db).await.unwrap();
        if change_assistant {
            let MessageContent::Agent(blocks) = &mut round.assistant.content else {
                unreachable!()
            };
            blocks.push(ContentBlock::text("changed retained assistant"));
        } else {
            let ToolRoundResult::PendingGoalReport { report, .. } = &round.results[0] else {
                unreachable!()
            };
            let binding = db
                .conversation_goal_execution_binding(round.execution)
                .await
                .unwrap()
                .unwrap();
            round.results[0] = ToolRoundResult::PendingGoalReport {
                report: GoalPendingReport::new(
                    binding.try_into().unwrap(),
                    report.call().clone(),
                    GoalReportOutcome::Complete(GoalText::parse("changed reason").unwrap()),
                ),
                duration_ms: 17,
            };
        }
        let before = evidence(&db, &owner, &round).await;
        assert!(round.commit(&db).await.is_err());
        assert_eq!(evidence(&db, &owner, &round).await, before);
    }
}

#[tokio::test]
async fn partial_retained_round_rejects_without_repair_or_goal_mutation() {
    let (db, owner, mut round) = report_fixture().await;
    let MessageContent::Agent(blocks) = &mut round.assistant.content else {
        unreachable!()
    };
    blocks.push(ContentBlock::ToolUse {
        id: "search-call".into(),
        name: "search".into(),
        input: serde_json::json!({}),
    });
    round
        .results
        .push(ToolResult::success("search-call".into(), "found".into()).into());
    round.commit(&db).await.unwrap();
    sqlx::query("DELETE FROM messages WHERE message_id=?")
        .bind(tool_result_message_id(
            &round.assistant.message_id,
            "search-call",
        ))
        .execute(db.pool())
        .await
        .unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert!(round.commit(&db).await.is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn before_canonical_commit_cut_rolls_back_report_and_all_checkpoint_evidence() {
    let (db, owner, round) = report_fixture().await;
    let before = evidence(&db, &owner, &round).await;
    let error = round
        .cut(&db, CanonicalCommitCut::BeforeCanonicalCommit)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("injected before canonical commit"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
    let retry = round.commit(&db).await.unwrap();
    assert!(!retry.replayed);
    assert_result(&retry.results[0], false);
}

#[tokio::test]
async fn after_canonical_commit_cut_retains_report_and_replays_without_duplicate_facts() {
    let (db, owner, round) = report_fixture().await;
    let error = round
        .cut(&db, CanonicalCommitCut::AfterCanonicalCommit)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("injected after canonical commit"));
    let before = evidence(&db, &owner, &round).await;
    assert_eq!(before["history"].as_array().unwrap().len(), 2);
    assert!(before["replay"].is_object());
    let retry = round.commit(&db).await.unwrap();
    assert!(retry.replayed);
    assert_result(&retry.results[0], false);
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn finalized_report_success_cannot_bypass_the_pending_checkpoint_path() {
    let (db, owner, round) = report_fixture().await;
    let committed = round.commit(&db).await.unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert!(db
        .persist_tool_round(
            &round.assistant.conversation_id,
            &round.assistant,
            &committed.results,
        )
        .await
        .is_err());
    assert!(db
        .persist_tool_round_and_state(
            &round.assistant.conversation_id,
            &round.assistant,
            &committed.results,
            &ConvState::Idle,
            round.timestamp + Duration::seconds(1),
        )
        .await
        .is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn terminal_report_checkpoint_requires_the_admitted_turn() {
    let (db, owner, round) = report_fixture().await;
    let obligation = crate::workflow::DirectTurnTerminalObligationInput {
        turn_id: phoenix_workflow::TurnAuthorityId(
            u64::try_from(round.execution.turn_id() + 1).unwrap(),
        ),
        expected_generation: 0,
        terminal: phoenix_workflow::TurnTerminal::Cancelled,
        projection: crate::workflow::PersistedConversationProjection {
            state: ConvState::Idle,
            state_updated_at: round.timestamp,
        },
        response_message_id: None,
    };
    let before = evidence(&db, &owner, &round).await;
    let error = db
        .persist_goal_tool_round_with_terminal_obligation(
            &round.assistant,
            round.execution,
            &round.results,
            &obligation,
            Some(&round.replay()),
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("goal report terminal turn differs"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn terminal_obligation_conflict_rolls_back_accepted_report_and_replay() {
    let (db, owner, round) = report_fixture().await;
    let generation: i64 =
        sqlx::query_scalar("SELECT generation FROM durable_turns WHERE turn_id=?")
            .bind(round.execution.turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    let obligation = crate::workflow::DirectTurnTerminalObligationInput {
        turn_id: phoenix_workflow::TurnAuthorityId(
            u64::try_from(round.execution.turn_id()).unwrap(),
        ),
        expected_generation: u64::try_from(generation + 1).unwrap(),
        terminal: phoenix_workflow::TurnTerminal::Cancelled,
        projection: crate::workflow::PersistedConversationProjection {
            state: ConvState::Idle,
            state_updated_at: round.timestamp,
        },
        response_message_id: None,
    };
    let before = evidence(&db, &owner, &round).await;
    assert!(db
        .persist_goal_tool_round_with_terminal_obligation(
            &round.assistant,
            round.execution,
            &round.results,
            &obligation,
            Some(&round.replay())
        )
        .await
        .is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
    let retry = round.commit(&db).await.unwrap();
    assert!(!retry.replayed);
    assert_result(&retry.results[0], false);
}
