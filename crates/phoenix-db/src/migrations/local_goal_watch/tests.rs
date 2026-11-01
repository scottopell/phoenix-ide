use super::super::run_through_for_test;
use crate::Database;
use sqlx::{sqlite::SqlitePoolOptions, Row};

// Schema-only migration coverage. Goal capture/dispatch behavior from the
// source slice is deferred to the Goals feature slice.
#[tokio::test]
async fn migration_137_widens_watch_events_and_preserves_existing_rows() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-137-schema".into());
    db.run_migrations().await.unwrap();
    run_through_for_test(db.pool(), 136).await.unwrap();
    let source = db
        .create_conversation("watch-137", "watch-137", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    let watch: i64 = sqlx::query_scalar(
        "SELECT id FROM coordinator_watches WHERE source_product_conversation_id=?",
    )
    .bind(source.product_conversation_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us) VALUES ('legacy-event',?,'creation','legacy-event',0,?,'completed',(SELECT max(enrolled_at_us)+1 FROM coordinator_watches))")
        .bind(watch)
        .bind(&source.id)
        .execute(db.pool()).await.unwrap();

    assert_eq!(run_through_for_test(db.pool(), 137).await.unwrap(), 1);
    let preserved = sqlx::query("SELECT source_occurrence_kind, terminal_kind FROM coordinator_watch_events WHERE event_id='legacy-event'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        preserved.get::<String, _>("source_occurrence_kind"),
        "creation"
    );
    assert_eq!(preserved.get::<String, _>("terminal_kind"), "completed");
    let schema: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='coordinator_watch_events'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(schema.contains("'goal_change'"));
    assert!(schema.contains("'goal_changed'"));
    assert_eq!(run_through_for_test(db.pool(), 137).await.unwrap(), 0);
}

#[tokio::test]
async fn migration_137_rolls_back_on_invalid_ddl_without_recording_version() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-137-rollback".into());
    db.run_migrations().await.unwrap();
    run_through_for_test(db.pool(), 136).await.unwrap();
    let migration = super::super::MIGRATIONS
        .iter()
        .find(|m| m.version == 137)
        .unwrap();
    let broken = super::super::Migration {
        version: migration.version,
        name: migration.name,
        sql: "INVALID MIGRATION SQL",
    };
    assert!(super::run(db.pool(), &broken).await.is_err());
    let recorded: i64 = sqlx::query_scalar("SELECT count(*) FROM _migrations WHERE version=137")
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
