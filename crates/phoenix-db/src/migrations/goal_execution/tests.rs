mod start_acceptance;
mod start_guards;
mod start_replay;

use super::*;
use crate::Database;
use sqlx::{sqlite::SqlitePoolOptions, Row};

async fn fixture() -> (Database, String) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-136-memory".into());
    db.run_migrations().await.unwrap();
    let index = super::super::MIGRATIONS
        .iter()
        .position(|migration| migration.version == 136)
        .unwrap();
    super::super::run_migration_list(db.pool(), &super::super::MIGRATIONS[..index])
        .await
        .unwrap();
    let conversation = db
        .create_conversation("before136", "before136", "/tmp", true, None, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,0,1,1,0,'Active','direct_turn.snapshot',1,X'0001FF',10,20)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind) VALUES (1,1,?,'message-before','exact-fingerprint',X'FF000A','Runtime',0,1,'user_api')")
        .bind(&conversation.id).execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TABLE migration136_retained_child (turn_id INTEGER PRIMARY KEY REFERENCES durable_turns(turn_id) ON DELETE CASCADE)").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO migration136_retained_child VALUES (1)")
        .execute(db.pool())
        .await
        .unwrap();
    (db, conversation.id)
}

fn migration() -> &'static Migration {
    super::super::MIGRATIONS
        .iter()
        .find(|migration| migration.version == 136)
        .unwrap()
}

async fn rootpages(db: &Database) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT name,rootpage FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(db.pool()).await.unwrap()
}

async fn migrate(db: &Database) -> DbResult<()> {
    super::run(db.pool(), migration()).await
}

#[tokio::test]
async fn migration136_preserves_parent_child_pages_prepared_bytes_and_acceptance_indexes() {
    let (db, _) = fixture().await;
    let before = rootpages(&db).await;
    let mut connection = db.pool().acquire().await.unwrap();
    sqlx::query("SELECT prepared_payload FROM durable_turns WHERE turn_id=1")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    migrate(&db).await.unwrap();
    let after = rootpages(&db).await;
    for page in before {
        assert!(
            after.contains(&page),
            "retained table changed rootpage: {page:?}"
        );
    }
    let row = sqlx::query("SELECT prepared_payload,prepared_fingerprint,input_kind,materialized,goal_start_id,origin_kind FROM durable_turns WHERE turn_id=1")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(row.get::<Vec<u8>, _>("prepared_payload"), vec![255, 0, 10]);
    assert_eq!(
        row.get::<String, _>("prepared_fingerprint"),
        "exact-fingerprint"
    );
    assert_eq!(row.get::<String, _>("input_kind"), "message");
    assert_eq!(row.get::<i64, _>("materialized"), 0);
    assert_eq!(row.get::<Option<i64>, _>("goal_start_id"), None);
    assert_eq!(row.get::<String, _>("origin_kind"), "user_api");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM migration136_retained_child")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    for name in [
        "durable_turns_one_live_owner",
        "durable_turns_discoverable_nonterminal",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name=?"
            )
            .bind(name)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }
    for name in [
        "conversation_goal_binding_admission",
        "conversation_goal_control_binding_admission",
    ] {
        let schema: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?")
                .bind(name)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(schema.contains("t.materialized = 0"));
        assert!(!schema.contains("t.canonical_message_id IS NULL"));
    }
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn migration136_failure_rolls_back_table_widening_new_rows_and_ledger() {
    let (db, _) = fixture().await;
    let schema: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE name='durable_turns' AND type='table'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let before = rootpages(&db).await;
    let failed = Migration { version: 136, name: "injected-failure", sql: "CREATE TABLE migration136_rollback_marker (id INTEGER); SELECT * FROM migration136_missing_table;" };
    assert!(super::run(db.pool(), &failed).await.is_err());
    let after: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE name='durable_turns' AND type='table'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(schema, after);
    assert_eq!(before, rootpages(&db).await);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _migrations WHERE version=136")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert!(
        sqlx::query("UPDATE durable_turns SET prepared_payload=NULL WHERE turn_id=1")
            .execute(db.pool())
            .await
            .is_err()
    );
    migrate(&db).await.unwrap();
}

#[tokio::test]
async fn migration136_materialization_fact_backfills_only_actual_messages_and_refuses_wrong_kind() {
    let (db, conversation) = fixture().await;
    sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('before-message',?,1,'user','{\"text\":\"exact\"}','2026-01-01T00:00:00Z','user_api')")
        .bind(&conversation).execute(db.pool()).await.unwrap();
    sqlx::query("UPDATE durable_turns SET canonical_message_id='before-message' WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    migrate(&db).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT materialized FROM durable_turns WHERE turn_id=1")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    for edit in [
        "UPDATE durable_turns SET materialized=0 WHERE turn_id=1",
        "UPDATE durable_turns SET canonical_message_id=NULL WHERE turn_id=1",
        "UPDATE durable_turns SET input_kind='goal_start',origin_kind=NULL,prepared_payload=NULL WHERE turn_id=1",
        "UPDATE durable_turns SET origin_kind=NULL WHERE turn_id=1",
        "UPDATE durable_turns SET prepared_payload=NULL WHERE turn_id=1",
    ] { assert!(sqlx::query(edit).execute(db.pool()).await.is_err(), "{edit}"); }
}

async fn seed_goal(
    db: &Database,
    owner: phoenix_core::domain::product_conversation::ProductConversationId,
    transcript: &str,
) {
    use crate::conversation_goals::user_commands::{
        GoalUserChange, GoalUserCommand, GoalUserResult,
    };
    use phoenix_core::domain::conversation_goal::{GoalText, GoalTimestampUs, GoalVersion};
    sqlx::query("UPDATE durable_turns SET terminal_kind='Cancelled',generation=generation+1,owns_conversation=0 WHERE conversation_id=? AND terminal_kind IS NULL")
        .bind(transcript).execute(db.pool()).await.unwrap();
    let receipt = db
        .apply_user_goal_command(
            &GoalUserCommand {
                owner,
                principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
                request_id: "11111111-1111-4111-8111-111111111111".parse().unwrap(),
                expected_version: GoalVersion::parse(0).unwrap(),
                change: GoalUserChange::Create {
                    objective: GoalText::parse("perform the goal").unwrap(),
                },
            },
            transcript,
            GoalTimestampUs::parse(1).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
}

async fn start_fixture() -> (Database, String, phoenix_workflow::PreparedTurn) {
    let (db, conversation) = fixture().await;
    let owner = db
        .get_conversation(&conversation)
        .await
        .unwrap()
        .product_conversation_id;
    seed_goal(&db, owner.clone(), &conversation).await;
    migrate(&db).await.unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) SELECT 2,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at FROM workflows WHERE workflow_id=1").execute(db.pool()).await.unwrap();
    sqlx::query("DELETE FROM durable_turns WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    let authority = phoenix_workflow::ConversationAuthority(conversation.clone());
    let prepared = phoenix_workflow::PreparedTurn::from_goal_start(
        &authority,
        phoenix_workflow::GoalStartReceiptId::new(1).unwrap(),
    );
    let principal_id = db.instance_id().await.unwrap().to_string();
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_starts(start_id,owner_id,request_id,goal_id,observed_version,requester_kind,owner_principal_id,historical_workflow_id,historical_turn_id,historical_transcript_id,live_turn_id) VALUES (1,?,'22222222-2222-4222-8222-222222222222',(SELECT goal_id FROM conversation_goal_history WHERE version=1),1,'owner',?,2,2,?,2)")
        .bind(owner.as_str()).bind(principal_id).bind(&conversation).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,input_kind,goal_start_id) VALUES (2,2,?,'start-key',?,NULL,'Runtime',0,1,NULL,'goal_start',1)")
        .bind(&conversation).bind(prepared.fingerprint()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    (db, conversation, prepared)
}

#[tokio::test]
async fn migration136_message_free_start_roundtrips_all_materialization_facts_and_detaches_receipt()
{
    let (db, conversation, prepared) = start_fixture().await;
    let repo = db.workflow_repository();
    let turn = repo
        .load_authoritative_turn(phoenix_workflow::TurnAuthorityId(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.prepared, prepared);
    assert!(turn.prepared.message_payload().is_err());
    assert_eq!(
        turn.materialization,
        phoenix_workflow::Materialization::UnmaterializedGoalStart {
            receipt: phoenix_workflow::GoalStartReceiptId::new(1).unwrap()
        }
    );
    sqlx::query("UPDATE durable_turns SET materialized=1 WHERE turn_id=2")
        .execute(db.pool())
        .await
        .unwrap();
    let turn = repo
        .load_authoritative_turn(phoenix_workflow::TurnAuthorityId(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        turn.materialization,
        phoenix_workflow::Materialization::MaterializedGoalStart {
            receipt: phoenix_workflow::GoalStartReceiptId::new(1).unwrap()
        }
    );
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM durable_turns WHERE disposition='Runtime' AND terminal_kind IS NULL AND materialized=0").fetch_one(db.pool()).await.unwrap(), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE conversation_id=?")
            .bind(&conversation)
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    for edit in [
        "UPDATE durable_turns SET canonical_message_id='fake' WHERE turn_id=2",
        "UPDATE durable_turns SET origin_kind='user_api' WHERE turn_id=2",
        "UPDATE durable_turns SET prepared_payload=X'00' WHERE turn_id=2",
        "UPDATE durable_turns SET materialized=0 WHERE turn_id=2",
    ] {
        assert!(
            sqlx::query(edit).execute(db.pool()).await.is_err(),
            "{edit}"
        );
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
    assert!(
        sqlx::query("UPDATE conversation_goal_starts SET live_turn_id=2 WHERE start_id=1")
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM conversation_goal_starts WHERE start_id=1")
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
