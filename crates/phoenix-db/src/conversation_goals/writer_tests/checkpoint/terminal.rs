use super::*;
use crate::workflow::{DirectTurnTerminalObligationInput, PersistedConversationProjection};

async fn obligation(db: &Database, round: &ReportRound) -> DirectTurnTerminalObligationInput {
    let generation: i64 =
        sqlx::query_scalar("SELECT generation FROM durable_turns WHERE turn_id=?")
            .bind(round.execution.turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    DirectTurnTerminalObligationInput {
        turn_id: phoenix_workflow::TurnAuthorityId(
            u64::try_from(round.execution.turn_id()).unwrap(),
        ),
        expected_generation: u64::try_from(generation).unwrap(),
        terminal: phoenix_workflow::TurnTerminal::Completed,
        projection: PersistedConversationProjection {
            state: ConvState::Idle,
            state_updated_at: round.timestamp,
        },
        response_message_id: None,
    }
}

fn ordinary_round(round: &mut ReportRound) {
    round.assistant.content = MessageContent::Agent(vec![ContentBlock::ToolUse {
        id: "checkpoint-call".into(),
        name: "search".into(),
        input: serde_json::json!({}),
    }]);
    round.results = vec![ToolResult::success("checkpoint-call".into(), "found".into()).into()];
}

async fn terminal_commit(
    db: &Database,
    round: &ReportRound,
    obligation: &DirectTurnTerminalObligationInput,
) -> crate::DbResult<CommittedGoalTerminalRound> {
    db.persist_goal_tool_round_with_terminal_obligation(
        &round.assistant,
        round.execution,
        &round.results,
        obligation,
        Some(&round.replay()),
    )
    .await
}

#[tokio::test]
async fn canonical_report_first_terminal_second_rejects_without_an_obligation() {
    let (db, owner, round) = report_fixture().await;
    let obligation = obligation(&db, &round).await;
    round.commit(&db).await.unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert_eq!(before["obligations"], 0);
    let error = terminal_commit(&db, &round, &obligation).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("retained round has no terminal obligation"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn canonical_ordinary_first_terminal_second_rejects_without_an_obligation() {
    let (db, owner, mut round) = report_fixture().await;
    ordinary_round(&mut round);
    let obligation = obligation(&db, &round).await;
    let canonical = round.commit(&db).await.unwrap();
    let before = evidence(&db, &owner, &round).await;
    let error = db
        .persist_tool_round_with_terminal_obligation(
            &round.assistant.conversation_id,
            &round.assistant,
            &canonical.results,
            &obligation,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("retained round has no terminal obligation"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn terminal_report_replay_verifies_identity_without_reapplying_projection() {
    let (db, owner, round) = report_fixture().await;
    let mut obligation = obligation(&db, &round).await;
    assert!(
        !terminal_commit(&db, &round, &obligation)
            .await
            .unwrap()
            .replayed
    );
    let before = evidence(&db, &owner, &round).await;
    assert_eq!(before["obligations"], 1);
    let replay = terminal_commit(&db, &round, &obligation).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(evidence(&db, &owner, &round).await, before);
    obligation.terminal = phoenix_workflow::TurnTerminal::Cancelled;
    assert!(terminal_commit(&db, &round, &obligation).await.is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn ordinary_terminal_replay_verifies_identity_and_preserves_newer_state() {
    let (db, owner, mut round) = report_fixture().await;
    ordinary_round(&mut round);
    let mut obligation = obligation(&db, &round).await;
    let committed = terminal_commit(&db, &round, &obligation).await.unwrap();
    assert!(!committed.replayed);
    let newer_state = ConvState::LlmRequesting { attempt: 3 };
    db.update_conversation_state(&round.assistant.conversation_id, &newer_state)
        .await
        .unwrap();
    let before = evidence(&db, &owner, &round).await;
    db.persist_tool_round_with_terminal_obligation(
        &round.assistant.conversation_id,
        &round.assistant,
        &committed.results,
        &obligation,
    )
    .await
    .unwrap();
    assert_eq!(evidence(&db, &owner, &round).await, before);
    obligation.expected_generation += 1;
    assert!(db
        .persist_tool_round_with_terminal_obligation(
            &round.assistant.conversation_id,
            &round.assistant,
            &committed.results,
            &obligation,
        )
        .await
        .is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
}
