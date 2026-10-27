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
    let finalized = vec![finalized_result(&round.assistant, ordinary_result())];
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
                MessageContent::User(_)
                | MessageContent::Tool(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_) => unreachable!(),
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

async fn seed_anthropic(db: &Database, round: &ReportRound, set: AnthropicResponseSet) {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    crate::provider_replay::apply_replay_tx(
        &mut tx,
        &round.assistant.conversation_id,
        &ProviderReplayUpdate::Anthropic(set),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn new_canonical_round_rejects_rewritten_anthropic_response_identity_atomically() {
    for field in ["model", "owner", "private", "public"] {
        let (db, owner, round) = report_fixture().await;
        let ProviderReplayUpdate::Anthropic(mut retained) = round.replay() else {
            unreachable!()
        };
        match field {
            "model" => retained.identity.model = "older-model".into(),
            "owner" => retained.owner_message_id = "older-assistant".into(),
            "private" => retained.private_blocks.clear(),
            "public" => {
                retained.public_content = vec![ContentBlock::Text {
                    text: "older public text".into(),
                }];
            }
            _ => unreachable!(),
        }
        seed_anthropic(&db, &round, retained).await;
        let before = evidence(&db, &owner, &round).await;
        let error = round.commit(&db).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Anthropic response identity differs"),
            "{field}: {error}"
        );
        assert_eq!(evidence(&db, &owner, &round).await, before, "{field}");
        assert_eq!(receipt_count(&db).await, 0);
    }
}

#[tokio::test]
async fn new_canonical_round_accepts_exact_anthropic_identity_without_rewinding_active_head() {
    let (db, owner, round) = report_fixture().await;
    let ProviderReplayUpdate::Anthropic(original) = round.replay() else {
        unreachable!()
    };
    seed_anthropic(&db, &round, original.clone()).await;
    let mut newer = original;
    newer.identity.response_id = "newer-response".into();
    newer.identity.model = "newer-model".into();
    newer.owner_message_id = "newer-assistant".into();
    seed_anthropic(&db, &round, newer).await;
    let before = evidence(&db, &owner, &round).await;
    assert!(!round.commit(&db).await.unwrap().replayed);
    let after = evidence(&db, &owner, &round).await;
    assert_eq!(after["replay_head"], before["replay_head"]);
    assert_eq!(after["replay"], before["replay"]);
    assert_eq!(receipt_count(&db).await, 1);
    assert_eq!(
        after["history"].as_array().unwrap().len(),
        before["history"].as_array().unwrap().len() + 1
    );
}

#[tokio::test]
async fn new_canonical_round_appends_a_new_anthropic_response_identity() {
    let (db, owner, round) = report_fixture().await;
    let ProviderReplayUpdate::Anthropic(mut earlier) = round.replay() else {
        unreachable!()
    };
    earlier.identity.response_id = "earlier-response".into();
    earlier.owner_message_id = "earlier-assistant".into();
    seed_anthropic(&db, &round, earlier.clone()).await;
    assert!(!round.commit(&db).await.unwrap().replayed);
    let replay = db
        .load_provider_replay_state(&round.assistant.conversation_id)
        .await
        .unwrap()
        .unwrap();
    let ProviderReplayUpdate::Anthropic(current) = round.replay() else {
        unreachable!()
    };
    assert_eq!(replay.response_sets, vec![earlier, current.clone()]);
    let after = evidence(&db, &owner, &round).await;
    assert_eq!(after["replay_head"][0], current.identity.model);
    assert_eq!(after["replay_head"][1], current.identity.response_id);
    assert_eq!(receipt_count(&db).await, 1);
}
