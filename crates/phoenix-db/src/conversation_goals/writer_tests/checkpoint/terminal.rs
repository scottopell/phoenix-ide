use super::*;
use crate::conversation_goals::round::CommittedGoalTerminalRound;
use crate::workflow::{DirectTurnTerminalObligationInput, PersistedConversationProjection};

pub(super) async fn obligation(
    db: &Database,
    round: &ReportRound,
) -> DirectTurnTerminalObligationInput {
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

pub(super) fn ordinary_round(round: &mut ReportRound) {
    round.assistant.content = MessageContent::Agent(vec![ContentBlock::ToolUse {
        id: "checkpoint-call".into(),
        name: "search".into(),
        input: serde_json::json!({}),
    }]);
    round.results = vec![ordinary_result().into()];
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
    let finalized = vec![finalized_result(&round.assistant, ordinary_result())];
    assert_eq!(
        serde_json::to_value(finalized[0].row()).unwrap(),
        serde_json::to_value(&canonical.results[0]).unwrap()
    );
    let before = evidence(&db, &owner, &round).await;
    let error = db
        .persist_tool_round_with_terminal_obligation(
            &round.assistant.conversation_id,
            &round.assistant,
            &finalized,
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
    let finalized = vec![finalized_result(&round.assistant, ordinary_result())];
    assert_eq!(
        serde_json::to_value(finalized[0].row()).unwrap(),
        serde_json::to_value(&committed.results[0]).unwrap()
    );
    let newer_state = ConvState::LlmRequesting { attempt: 3 };
    db.update_conversation_state(&round.assistant.conversation_id, &newer_state)
        .await
        .unwrap();
    let before = evidence(&db, &owner, &round).await;
    db.persist_tool_round_with_terminal_obligation_input_at_cut(
        &round.assistant.conversation_id,
        &round.assistant,
        ToolRoundWrite::Finalized(&finalized),
        &obligation,
        Some(&round.replay()),
        crate::TerminalEvidenceTransactionCut::None,
    )
    .await
    .unwrap();
    assert_eq!(evidence(&db, &owner, &round).await, before);
    obligation.expected_generation += 1;
    assert!(db
        .persist_tool_round_with_terminal_obligation_input_at_cut(
            &round.assistant.conversation_id,
            &round.assistant,
            ToolRoundWrite::Finalized(&finalized),
            &obligation,
            Some(&round.replay()),
            crate::TerminalEvidenceTransactionCut::None,
        )
        .await
        .is_err());
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn terminal_retries_bind_full_replay_command_without_rewinding_newer_evidence() {
    for ordinary in [false, true] {
        let (db, owner, mut round) = report_fixture().await;
        if ordinary {
            ordinary_round(&mut round);
        }
        let obligation = obligation(&db, &round).await;
        terminal_commit(&db, &round, &obligation).await.unwrap();
        db.update_conversation_state(
            &round.assistant.conversation_id,
            &ConvState::LlmRequesting { attempt: 9 },
        )
        .await
        .unwrap();
        db.clear_provider_replay_state(&round.assistant.conversation_id)
            .await
            .unwrap();
        let before = evidence(&db, &owner, &round).await;
        assert_eq!(before["receipts"].as_array().unwrap().len(), 1);
        assert!(
            terminal_commit(&db, &round, &obligation)
                .await
                .unwrap()
                .replayed
        );
        assert_eq!(evidence(&db, &owner, &round).await, before);
        let ProviderReplayUpdate::Anthropic(mut changed) = round.replay() else {
            unreachable!()
        };
        changed.identity.model = "changed-model".into();
        for replay in [
            Some(ProviderReplayUpdate::Anthropic(changed)),
            None,
            Some(ProviderReplayUpdate::Clear),
        ] {
            let error = db
                .persist_goal_tool_round_with_terminal_obligation(
                    &round.assistant,
                    round.execution,
                    &round.results,
                    &obligation,
                    replay.as_ref(),
                )
                .await
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("canonical checkpoint command differs"));
            assert_eq!(evidence(&db, &owner, &round).await, before);
        }
    }
}

#[tokio::test]
async fn terminal_receipt_and_obligation_share_before_and_after_commit_cuts() {
    use crate::TerminalEvidenceTransactionCut;
    for ordinary in [false, true] {
        for (cut, retained) in [
            (TerminalEvidenceTransactionCut::BeforeCommit, false),
            (TerminalEvidenceTransactionCut::AfterCommit, true),
        ] {
            let (db, owner, mut round) = report_fixture().await;
            if ordinary {
                ordinary_round(&mut round);
            }
            let obligation = obligation(&db, &round).await;
            let before = evidence(&db, &owner, &round).await;
            assert!(db
                .persist_tool_round_with_terminal_obligation_input_at_cut(
                    &round.assistant.conversation_id,
                    &round.assistant,
                    ToolRoundWrite::Pending {
                        execution: round.execution,
                        results: &round.results
                    },
                    &obligation,
                    Some(&round.replay()),
                    cut,
                )
                .await
                .is_err());
            let after = evidence(&db, &owner, &round).await;
            assert_eq!(after["obligations"], i64::from(retained));
            assert_eq!(
                after["receipts"].as_array().unwrap().len(),
                usize::from(retained)
            );
            if retained {
                assert!(
                    terminal_commit(&db, &round, &obligation)
                        .await
                        .unwrap()
                        .replayed
                );
                assert_eq!(evidence(&db, &owner, &round).await, after);
            } else {
                assert_eq!(after, before);
            }
        }
    }
}

#[tokio::test]
async fn terminal_to_canonical_requires_the_identical_projection_and_replay_command() {
    let (db, owner, round) = report_fixture().await;
    let obligation = obligation(&db, &round).await;
    terminal_commit(&db, &round, &obligation).await.unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert!(round
        .commit(&db)
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    let checkpoint = GoalRoundCheckpoint {
        state: &obligation.projection.state,
        state_updated_at: obligation.projection.state_updated_at,
        ..round.checkpoint()
    };
    assert!(
        db.persist_canonical_checkpoint(&checkpoint, Some(&round.replay()))
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(evidence(&db, &owner, &round).await, before);
}
