use super::super::{run_through_for_test, MIGRATIONS};
use crate::{schema::MessageContent, Database};
use sqlx::{sqlite::SqlitePoolOptions, Row};

// Schema-only coverage for migration 138. Goal execution and start acceptance tests
// from the source slice remain deferred to the Goals feature slice.
#[tokio::test]
async fn migration_138_backfills_materialized_without_breaking_ordinary_message_writes() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-138-schema".into());
    db.run_migrations().await.unwrap();
    run_through_for_test(db.pool(), 137).await.unwrap();
    let conversation = db
        .create_conversation("schema-138", "schema-138", "/tmp", true, None, None)
        .await
        .unwrap();
    let message = db
        .add_message(
            "before-138",
            &conversation.id,
            &MessageContent::user("before"),
            None,
            None,
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,0,1,1,0,'Active','direct_turn.snapshot',1,X'0001FF',10,20)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,canonical_message_id) VALUES (1,1,?,'schema-key','schema-fingerprint',X'FF000A','Runtime',0,1,'user_api',?)")
        .bind(&conversation.id)
        .bind(&message.message_id)
        .execute(db.pool()).await.unwrap();

    assert_eq!(run_through_for_test(db.pool(), 138).await.unwrap(), 1);
    let row =
        sqlx::query("SELECT materialized, prepared_payload FROM durable_turns WHERE turn_id=1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(row.get::<i64, _>("materialized"), 1);
    assert_eq!(row.get::<Vec<u8>, _>("prepared_payload"), vec![255, 0, 10]);
    assert!(
        sqlx::query("UPDATE durable_turns SET materialized=0 WHERE turn_id=1")
            .execute(db.pool())
            .await
            .is_err()
    );

    run_through_for_test(db.pool(), 140).await.unwrap();
    let written = db
        .add_message(
            "after-138",
            &conversation.id,
            &MessageContent::user("after"),
            None,
            None,
        )
        .await
        .unwrap();
    let decoded = db.get_message_by_id(&written.message_id).await.unwrap();
    assert_eq!(decoded.message_id, written.message_id);
    assert_eq!(decoded.content, written.content);
    assert_eq!(run_through_for_test(db.pool(), 140).await.unwrap(), 0);
    assert_eq!(MIGRATIONS.last().unwrap().version, 140);
}

#[tokio::test]
async fn migration_138_rolls_back_schema_edit_without_ledger_write() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-138-rollback".into());
    db.run_migrations().await.unwrap();
    run_through_for_test(db.pool(), 137).await.unwrap();
    let migration = MIGRATIONS.iter().find(|m| m.version == 138).unwrap();
    let broken = super::super::Migration {
        version: migration.version,
        name: migration.name,
        sql: "INVALID MIGRATION SQL",
    };
    assert!(super::run(db.pool(), &broken).await.is_err());
    let column: i64 = sqlx::query_scalar(
        "SELECT \"notnull\" FROM pragma_table_info('durable_turns') WHERE name='prepared_payload'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(column, 1);
    let recorded: i64 = sqlx::query_scalar("SELECT count(*) FROM _migrations WHERE version=138")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(recorded, 0);
    let writable: i64 = sqlx::query_scalar("PRAGMA writable_schema")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(writable, 0);
}
