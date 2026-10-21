use super::*;

async fn receipt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner: &str,
    transcript: &str,
    workflow: i64,
    turn: i64,
) {
    sqlx::query("INSERT INTO conversation_goal_starts(start_id,owner_id,request_id,goal_id,observed_version,requester_kind,owner_principal_id,historical_workflow_id,historical_turn_id,historical_transcript_id,live_turn_id) VALUES (1,?,'33333333-3333-4333-8333-333333333333','migration-start-goal',1,'owner','11111111-1111-4111-8111-111111111111',?,?,?,?)")
        .bind(owner).bind(workflow).bind(turn).bind(transcript).bind(turn)
        .execute(&mut **tx).await.unwrap();
}

async fn root_owner(
    db: &Database,
    transcript: &str,
) -> phoenix_core::domain::product_conversation::ProductConversationId {
    db.get_conversation(transcript)
        .await
        .unwrap()
        .product_conversation_id
}

#[tokio::test]
async fn migration136_start_input_rejects_other_roots_goal_even_with_matching_receipt_transcript() {
    let (db, conversation) = fixture().await;
    let owner = root_owner(&db, &conversation).await;
    seed_goal(&db, owner.clone(), &conversation).await;
    let other = db
        .create_conversation("other-root", "other-root", "/tmp", true, None, None)
        .await
        .unwrap();
    migrate(&db).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    receipt_tx(&mut tx, owner.as_str(), &other.id, 2, 2).await;
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) SELECT 2,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at FROM workflows WHERE workflow_id=1").execute(&mut *tx).await.unwrap();
    assert!(sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,input_kind,goal_start_id) VALUES (2,2,?,'forged-start','fingerprint',NULL,'Runtime',0,1,NULL,'goal_start',1)")
        .bind(&other.id).execute(&mut *tx).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_goal_starts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn migration136_receipt_commit_requires_exact_reciprocal_start_not_an_existing_message() {
    let (db, conversation) = fixture().await;
    let owner = root_owner(&db, &conversation).await;
    seed_goal(&db, owner.clone(), &conversation).await;
    let other = db
        .create_conversation("other-root", "other-root", "/tmp", true, None, None)
        .await
        .unwrap();
    migrate(&db).await.unwrap();
    for (workflow, transcript) in [
        (1, conversation.as_str()),
        (99, conversation.as_str()),
        (1, other.id.as_str()),
    ] {
        let mut connection = db.pool().acquire().await.unwrap();
        sqlx::query("BEGIN")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_goal_starts(start_id,owner_id,request_id,goal_id,observed_version,requester_kind,owner_principal_id,historical_workflow_id,historical_turn_id,historical_transcript_id,live_turn_id) VALUES (1,?,'33333333-3333-4333-8333-333333333333','migration-start-goal',1,'owner','11111111-1111-4111-8111-111111111111',?,1,?,1)")
            .bind(owner.as_str()).bind(workflow).bind(transcript).execute(&mut *connection).await.unwrap();
        assert!(sqlx::query("COMMIT")
            .execute(&mut *connection)
            .await
            .is_err());
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_starts")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn migration136_detaching_an_unaccepted_phantom_receipt_cannot_evade_commit_proof() {
    let (db, conversation) = fixture().await;
    let owner = root_owner(&db, &conversation).await;
    seed_goal(&db, owner.clone(), &conversation).await;
    migrate(&db).await.unwrap();
    let mut connection = db.pool().acquire().await.unwrap();
    sqlx::query("BEGIN")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO conversation_goal_starts(start_id,owner_id,request_id,goal_id,observed_version,requester_kind,owner_principal_id,historical_workflow_id,historical_turn_id,historical_transcript_id,live_turn_id) VALUES (1,?,'33333333-3333-4333-8333-333333333333','migration-start-goal',1,'owner','11111111-1111-4111-8111-111111111111',99,99,?,99)")
        .bind(owner.as_str()).bind(&conversation).execute(&mut *connection).await.unwrap();
    sqlx::query("UPDATE conversation_goal_starts SET live_turn_id=NULL WHERE start_id=1")
        .execute(&mut *connection)
        .await
        .unwrap();
    assert!(
        sqlx::query("INSERT INTO conversation_goal_start_canonical_inputs VALUES (1)")
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .is_err());
    sqlx::query("ROLLBACK")
        .execute(&mut *connection)
        .await
        .unwrap();
}

#[tokio::test]
async fn migration136_both_start_materialization_states_freeze_turn_workflow_and_transcript() {
    let (db, conversation, _) = start_fixture().await;
    let other = db
        .create_conversation("other-root", "other-root", "/tmp", true, None, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) SELECT 3,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at FROM workflows WHERE workflow_id=2").execute(db.pool()).await.unwrap();
    for materialized in [0_i64, 1] {
        sqlx::query("UPDATE durable_turns SET materialized=? WHERE turn_id=2")
            .bind(materialized)
            .execute(db.pool())
            .await
            .unwrap();
        for edit in [
            "UPDATE durable_turns SET turn_id=99 WHERE turn_id=2",
            "UPDATE durable_turns SET workflow_id=3 WHERE turn_id=2",
        ] {
            assert!(
                sqlx::query(edit).execute(db.pool()).await.is_err(),
                "{edit}"
            );
        }
        assert!(
            sqlx::query("UPDATE durable_turns SET conversation_id=? WHERE turn_id=2")
                .bind(&other.id)
                .execute(db.pool())
                .await
                .is_err()
        );
        let row: (i64, String) =
            sqlx::query_as("SELECT workflow_id,conversation_id FROM durable_turns WHERE turn_id=2")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(row, (2, conversation.clone()));
    }
    sqlx::query("DELETE FROM durable_turns WHERE turn_id=2")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT live_turn_id FROM conversation_goal_starts WHERE start_id=1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_goal_start_canonical_inputs"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert!(
        sqlx::query("DELETE FROM conversation_goal_start_canonical_inputs")
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
}
