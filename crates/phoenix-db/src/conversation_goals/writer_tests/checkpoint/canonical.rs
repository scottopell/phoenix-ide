use super::*;

async fn receipt_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM canonical_tool_round_receipts")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn plain_round_cannot_claim_any_canonical_checkpoint() {
    let (db, owner, mut round) = report_fixture().await;
    terminal::ordinary_round(&mut round);
    let finalized = vec![crate::conversation_goals::round::result_message(
        &round.assistant,
        0,
        &ToolResult::success("checkpoint-call".into(), "found".into()),
    )
    .unwrap()];
    db.persist_tool_round(
        &round.assistant.conversation_id,
        &round.assistant,
        &finalized,
    )
    .await
    .unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert!(db
        .persist_tool_round_and_state(
            &round.assistant.conversation_id,
            &round.assistant,
            &finalized,
            &round.state,
            round.timestamp
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("retained round has no canonical checkpoint receipt"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
    assert!(db
        .persist_tool_round_state_and_replay(
            &round.assistant.conversation_id,
            &round.assistant,
            &finalized,
            &round.state,
            round.timestamp,
            &round.replay()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("retained round has no canonical checkpoint receipt"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
    assert!(db
        .persist_goal_tool_round_and_state(&round.checkpoint())
        .await
        .unwrap_err()
        .to_string()
        .contains("retained round has no canonical checkpoint receipt"));
    assert!(round
        .commit(&db)
        .await
        .unwrap_err()
        .to_string()
        .contains("retained round has no canonical checkpoint receipt"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
    assert_eq!(receipt_count(&db).await, 0);
}

#[tokio::test]
async fn report_messages_without_canonical_receipt_cannot_claim_state_or_replay() {
    let (db, owner, round) = report_fixture().await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    crate::conversation_goals::round::write_round_tx(
        &mut tx,
        &round.assistant.conversation_id,
        &round.assistant,
        crate::conversation_goals::round::ToolRoundWrite::Pending {
            execution: round.execution,
            results: &round.results,
        },
        &round.state,
        round.timestamp,
        db.goal_write_policies(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert!(db
        .persist_goal_tool_round_and_state(&round.checkpoint())
        .await
        .unwrap_err()
        .to_string()
        .contains("retained round has no canonical checkpoint receipt"));
    assert!(round
        .commit(&db)
        .await
        .unwrap_err()
        .to_string()
        .contains("retained round has no canonical checkpoint receipt"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
    assert_eq!(receipt_count(&db).await, 0);
}

#[tokio::test]
async fn exact_canonical_retry_preserves_newer_state_and_private_replay() {
    let (db, owner, round) = report_fixture().await;
    let first = round.commit(&db).await.unwrap();
    assert_eq!(receipt_count(&db).await, 1);
    let newer = ConvState::LlmRequesting { attempt: 4 };
    db.update_conversation_state(&round.assistant.conversation_id, &newer)
        .await
        .unwrap();
    db.clear_provider_replay_state(&round.assistant.conversation_id)
        .await
        .unwrap();
    let before = evidence(&db, &owner, &round).await;
    let retry = round.commit(&db).await.unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.state, newer);
    assert_eq!(
        retry.state_updated_at.to_rfc3339(),
        before["state"][2].as_str().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&retry.results).unwrap(),
        serde_json::to_value(&first.results).unwrap()
    );
    assert_eq!(evidence(&db, &owner, &round).await, before);
    assert_eq!(receipt_count(&db).await, 1);
}

#[tokio::test]
async fn retained_canonical_identity_rejects_changed_state_time_and_replay_command() {
    let (db, owner, mut round) = report_fixture().await;
    round.commit(&db).await.unwrap();
    let before = evidence(&db, &owner, &round).await;
    let state = round.state.clone();
    round.state = ConvState::LlmRequesting { attempt: 7 };
    assert!(round
        .commit(&db)
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    round.state = state;
    round.timestamp += Duration::microseconds(1);
    assert!(round
        .commit(&db)
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    round.timestamp -= Duration::microseconds(1);
    assert!(db
        .persist_goal_tool_round_and_state(&round.checkpoint())
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    assert!(db
        .persist_goal_tool_round_state_and_replay(&round.checkpoint(), &ProviderReplayUpdate::Clear)
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    let altered = ProviderReplayUpdate::Anthropic(
        AnthropicResponseSet::with_public_content(
            AnthropicResponseIdentity {
                response_id: "changed-response".into(),
                model: "checkpoint-model".into(),
            },
            match &round.assistant.content {
                MessageContent::Agent(blocks) => blocks.clone(),
                _ => unreachable!(),
            },
            vec![],
        )
        .unwrap()
        .with_owner_message_id(round.assistant.message_id.clone()),
    );
    assert!(db
        .persist_goal_tool_round_state_and_replay(&round.checkpoint(), &altered)
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    assert_eq!(evidence(&db, &owner, &round).await, before);
    assert_eq!(receipt_count(&db).await, 1);
}

#[tokio::test]
async fn no_replay_command_and_explicit_clear_are_distinct_canonical_identities() {
    let (db, owner, round) = report_fixture().await;
    db.persist_goal_tool_round_and_state(&round.checkpoint())
        .await
        .unwrap();
    let before = evidence(&db, &owner, &round).await;
    assert!(db
        .persist_goal_tool_round_state_and_replay(&round.checkpoint(), &ProviderReplayUpdate::Clear)
        .await
        .unwrap_err()
        .to_string()
        .contains("canonical checkpoint command differs"));
    assert!(
        db.persist_goal_tool_round_and_state(&round.checkpoint())
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(evidence(&db, &owner, &round).await, before);
}

#[tokio::test]
async fn canonical_receipt_rolls_back_before_commit_and_survives_after_commit_failure() {
    for (cut, retained) in [
        (CanonicalCommitCut::BeforeCanonicalCommit, false),
        (CanonicalCommitCut::AfterCanonicalCommit, true),
    ] {
        let (db, owner, round) = report_fixture().await;
        let before = evidence(&db, &owner, &round).await;
        assert!(round.cut(&db, cut).await.is_err());
        assert_eq!(receipt_count(&db).await, i64::from(retained));
        if retained {
            let after = evidence(&db, &owner, &round).await;
            assert!(round.commit(&db).await.unwrap().replayed);
            assert_eq!(evidence(&db, &owner, &round).await, after);
        } else {
            assert_eq!(evidence(&db, &owner, &round).await, before);
            assert!(!round.commit(&db).await.unwrap().replayed);
        }
    }
}
