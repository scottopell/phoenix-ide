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
    sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('before-138',?1,1,'user',?2,'2026-01-01T00:00:00Z','user_api')")
        .bind(&conversation.id).bind(serde_json::to_string(&MessageContent::user("before")).unwrap())
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,0,1,1,0,'Active','direct_turn.snapshot',1,X'0001FF',10,20)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,canonical_message_id) VALUES (1,1,?,'schema-key','schema-fingerprint',X'FF000A','Runtime',0,1,'user_api',?)")
        .bind(&conversation.id)
        .bind("before-138")
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
    assert_eq!(run_through_for_test(db.pool(), 140).await.unwrap(), 0);
    assert_eq!(MIGRATIONS.last().unwrap().version, 142);
    assert_eq!(run_through_for_test(db.pool(), 141).await.unwrap(), 1);
    let receipt_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='canonical_tool_round_receipts'",
    ).fetch_one(db.pool()).await.unwrap();
    assert_eq!(receipt_table, 1);
    assert_eq!(run_through_for_test(db.pool(), 141).await.unwrap(), 0);
    assert_eq!(run_through_for_test(db.pool(), 142).await.unwrap(), 1);
    assert_eq!(run_through_for_test(db.pool(), 142).await.unwrap(), 0);
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
}

#[tokio::test]
async fn migration_141_requires_nonnegative_integer_microsecond_receipts() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-141-clock".into());
    db.run_migrations().await.unwrap();
    run_through_for_test(db.pool(), 140).await.unwrap();
    let conversation = db
        .create_conversation("schema-141", "schema-141", "/tmp", true, None, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('clock-assistant',?1,1,'agent',?2,'2026-01-01T00:00:00Z','system_generated')")
        .bind(&conversation.id).bind(serde_json::to_string(&MessageContent::Agent(vec![])).unwrap())
        .execute(db.pool()).await.unwrap();
    assert_eq!(run_through_for_test(db.pool(), 141).await.unwrap(), 1);
    assert_eq!(run_through_for_test(db.pool(), 141).await.unwrap(), 0);
    let clock_type: String = sqlx::query_scalar(
        "SELECT type FROM pragma_table_info('canonical_tool_round_receipts')
         WHERE name='selected_state_updated_at_us'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(clock_type, "INTEGER");
    for (value, accepted) in [
        ("0", true),
        ("1700000000123456", true),
        ("9223372036854775807", true),
        ("NULL", false),
        ("-1", false),
        ("0.5", false),
        ("'not-an-integer'", false),
        ("X'00'", false),
        ("9223372036854775808", false),
        ("1e30", false),
    ] {
        let mut insert = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "INSERT INTO canonical_tool_round_receipts
             (conversation_id,assistant_message_id,selected_checkpoint_sha256,
              selected_state_updated_at_us,replay_kind)
             VALUES (",
        );
        insert
            .push_bind(&conversation.id)
            .push(",")
            .push_bind("clock-assistant")
            .push(",zeroblob(32),")
            .push(value)
            .push(",'none')");
        let outcome = insert.build().execute(db.pool()).await;
        assert_eq!(outcome.is_ok(), accepted, "{value}: {outcome:?}");
        if accepted {
            let stored: (i64, String) = sqlx::query_as(
                "SELECT selected_state_updated_at_us,typeof(selected_state_updated_at_us)
                 FROM canonical_tool_round_receipts",
            )
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(stored, (value.parse::<i64>().unwrap(), "integer".into()));
        }
        sqlx::query("DELETE FROM canonical_tool_round_receipts")
            .execute(db.pool())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn migration_141_failure_does_not_advance_ledger() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-141-failure".into());
    db.run_migrations().await.unwrap();
    run_through_for_test(db.pool(), 140).await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE sentinel (value INTEGER);
         CREATE TRIGGER canonical_tool_round_receipt_source BEFORE INSERT ON sentinel
         BEGIN SELECT 1; END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(run_through_for_test(db.pool(), 141).await.is_err());
    let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migrations")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(version, 140);
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
         AND name='canonical_tool_round_receipts'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(tables, 0);
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
