use chrono::Utc;
use phoenix_core::domain::conversation_goal::{
    GoalExecution, GoalReportOutcome, GoalState, GoalVersion, ToolRoundResult,
};
use phoenix_core::domain::db_schema::MessageContent;
use phoenix_core::domain::sm_state::ConvState;

use super::super::t;
use super::{pending_fixture, round_assistant, settle_fixture_turn};
use crate::conversation_goals::round::GoalRoundCheckpoint;

#[tokio::test]
async fn genuine_report_round_commits_paired_messages_and_state_then_replays_original_result() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("finished"))).await;
    let assistant = round_assistant(&db).await;
    sqlx::query("DELETE FROM messages WHERE message_id='report-message'")
        .execute(db.pool())
        .await
        .unwrap();
    let results = [ToolRoundResult::PendingGoalReport {
        report: pending,
        duration_ms: 12,
    }];
    let state = ConvState::LlmRequesting { attempt: 2 };
    let timestamp = Utc::now();
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &state,
        state_updated_at: timestamp,
    };
    let committed = db
        .persist_goal_tool_round_and_state(&checkpoint)
        .await
        .unwrap();
    assert!(!committed.replayed);
    assert_eq!(committed.state, state);
    assert_eq!(committed.state_updated_at, timestamp);
    assert_eq!(db.get_conversation("full-root").await.unwrap().state, state);
    assert_eq!(committed.results.len(), 1);
    let MessageContent::Tool(result) = &committed.results[0].content else {
        panic!("paired result");
    };
    assert!(!result.is_error);
    let retained_output = result.content.clone();
    assert!(matches!(
        db.conversation_goal(&command.owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Complete { .. }
    ));
    settle_fixture_turn(&db).await;
    let later_state = ConvState::Idle;
    db.update_conversation_state("full-root", &later_state)
        .await
        .unwrap();
    let replay = db
        .persist_goal_tool_round_and_state(&checkpoint)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, later_state);
    assert_eq!(
        db.get_conversation("full-root").await.unwrap().state,
        later_state
    );
    let MessageContent::Tool(result) = &replay.results[0].content else {
        panic!("paired replay");
    };
    assert_eq!(result.content, retained_output);
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(db.get_messages("full-root").await.unwrap().len(), 3);
}

#[tokio::test]
async fn selected_state_failure_rolls_back_genuine_assistant_report_and_result_together() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("finished"))).await;
    let assistant = round_assistant(&db).await;
    sqlx::query("DELETE FROM messages WHERE message_id='report-message'")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_round_state BEFORE UPDATE OF state ON conversations WHEN NEW.id='full-root' BEGIN SELECT RAISE(ABORT,'state checkpoint refused'); END")
        .execute(db.pool()).await.unwrap();
    let results = [ToolRoundResult::PendingGoalReport {
        report: pending,
        duration_ms: 1,
    }];
    let state = ConvState::LlmRequesting { attempt: 2 };
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    assert!(db
        .persist_goal_tool_round_and_state(&checkpoint)
        .await
        .is_err());
    assert_eq!(db.get_messages("full-root").await.unwrap().len(), 1);
    assert_eq!(
        db.conversation_goal(&command.owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        db.get_conversation("full-root").await.unwrap().state,
        ConvState::Idle
    ));
}

#[tokio::test]
async fn committed_domain_refusal_replays_as_error_after_conditions_change() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("finished"))).await;
    let assistant = round_assistant(&db).await;
    sqlx::query("DELETE FROM messages WHERE message_id='report-message'")
        .execute(db.pool())
        .await
        .unwrap();
    settle_fixture_turn(&db).await;
    let results = [ToolRoundResult::PendingGoalReport {
        report: pending,
        duration_ms: 3,
    }];
    let state = ConvState::Idle;
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    let committed = db
        .persist_goal_tool_round_and_state(&checkpoint)
        .await
        .unwrap();
    let MessageContent::Tool(original) = &committed.results[0].content else {
        panic!("error result");
    };
    assert!(original.is_error);
    sqlx::query("UPDATE durable_turns SET terminal_kind=NULL,owns_conversation=1 WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    let replay = db
        .persist_goal_tool_round_and_state(&checkpoint)
        .await
        .unwrap();
    let MessageContent::Tool(retained) = &replay.results[0].content else {
        panic!("retained error");
    };
    assert_eq!(retained, original);
    assert_eq!(
        db.conversation_goal(&command.owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
}

#[tokio::test]
async fn finalized_checkpoint_owners_cannot_commit_precomputed_report_success() {
    use phoenix_core::domain::db_schema::ToolResult;
    let (db, command, _) = pending_fixture(GoalReportOutcome::Complete(t("finished"))).await;
    let assistant = round_assistant(&db).await;
    sqlx::query("DELETE FROM messages WHERE message_id='report-message'")
        .execute(db.pool())
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    let result = crate::conversation_goals::round::result_message(
        &assistant,
        0,
        &ToolResult::success("report-call".into(), "not committed".into()),
    )
    .unwrap();
    let error = crate::conversation_goals::round::persist_finalized_round_tx(
        &mut tx,
        &assistant,
        std::slice::from_ref(&result),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("requires pending round finalization"));
    tx.rollback().await.unwrap();
    assert_eq!(db.get_messages("full-root").await.unwrap().len(), 1);
    assert_eq!(
        db.conversation_goal(&command.owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
}

#[tokio::test]
async fn finalized_owners_reject_cross_assistant_goal_results_before_rows_or_state_are_written() {
    use phoenix_core::domain::db_schema::ToolResult;
    use phoenix_core::domain::llm_types::ContentBlock;
    use phoenix_core::domain::provider_replay::ProviderReplayUpdate;
    for call_id in ["unrelated-call", "report-call"] {
        let (db, command, _) = pending_fixture(GoalReportOutcome::Complete(t("finished"))).await;
        let source = round_assistant(&db).await;
        let result = crate::conversation_goals::round::result_message(
            &source,
            0,
            &ToolResult::success("report-call".into(), "uncommitted goal success".into()),
        )
        .unwrap();
        let mut unrelated = source.clone();
        unrelated.message_id = "unrelated-assistant".into();
        unrelated.sequence_id = 4;
        unrelated.content = MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call_id.into(),
            name: "bash".into(),
            input: serde_json::json!({"command":"pwd"}),
        }]);
        let results = [result];
        let state = ConvState::LlmRequesting { attempt: 2 };
        assert!(db
            .persist_tool_round("full-root", &unrelated, &results)
            .await
            .is_err());
        assert!(db
            .persist_tool_round_and_state("full-root", &unrelated, &results, &state, Utc::now(),)
            .await
            .is_err());
        assert!(db
            .persist_tool_round_state_and_replay(
                "full-root",
                &unrelated,
                &results,
                &state,
                Utc::now(),
                &ProviderReplayUpdate::Clear,
            )
            .await
            .is_err());
        let messages = db.get_messages("full-root").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert!(!messages
            .iter()
            .any(|message| message.message_id == "unrelated-assistant"));
        assert!(matches!(
            db.get_conversation("full-root").await.unwrap().state,
            ConvState::Idle
        ));
        assert_eq!(
            db.conversation_goal(&command.owner)
                .await
                .unwrap()
                .unwrap()
                .version
                .get(),
            1
        );
        assert_eq!(
            db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn finalized_owner_rejects_missing_extra_duplicate_and_cross_conversation_pairing() {
    use phoenix_core::domain::db_schema::{MessageType, ToolResult};
    for invalid in [
        "missing",
        "extra",
        "duplicate_call",
        "cross_conversation",
        "wrong_type",
    ] {
        let (db, _, _) = pending_fixture(GoalReportOutcome::Complete(t("finished"))).await;
        let mut assistant = round_assistant(&db).await;
        let result = crate::conversation_goals::round::result_message(
            &assistant,
            0,
            &ToolResult::error("report-call".into(), "unknown tool".into()),
        )
        .unwrap();
        let mut results = vec![result];
        match invalid {
            "missing" => results.clear(),
            "extra" => results.push(results[0].clone()),
            "duplicate_call" => {
                let MessageContent::Agent(blocks) = &mut assistant.content else {
                    panic!("assistant");
                };
                blocks.push(blocks[0].clone());
                results.push(results[0].clone());
            }
            "cross_conversation" => results[0].conversation_id = "different-transcript".into(),
            "wrong_type" => results[0].message_type = MessageType::Agent,
            _ => unreachable!(),
        }
        assert!(
            db.persist_tool_round_and_state(
                "full-root",
                &assistant,
                &results,
                &ConvState::LlmRequesting { attempt: 2 },
                Utc::now(),
            )
            .await
            .is_err(),
            "{invalid}"
        );
        assert_eq!(
            db.get_messages("full-root").await.unwrap().len(),
            2,
            "{invalid}"
        );
        assert!(
            matches!(
                db.get_conversation("full-root").await.unwrap().state,
                ConvState::Idle
            ),
            "{invalid}"
        );
    }
}
