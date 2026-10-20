use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        for table in ["messages", "steering_messages", "durable_turns"] {
            let sql: String = sqlx::query_scalar(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            )
            .bind(table)
            .fetch_one(&mut *tx)
            .await?;
            let old = "REFERENCES coordinator_watch_events(event_id)";
            if sql.matches(old).count() != 1 {
                return Err(DbError::Serialization(format!(
                    "migration 131 unexpected {table} subscription foreign key"
                )));
            }
            sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE type = 'table' AND name = ?1")
                .bind(table)
                .bind(sql.replacen(old, "REFERENCES subscription_delivery_identities(id)", 1))
                .execute(&mut *tx)
                .await?;
        }
        let version: i64 = sqlx::query_scalar("PRAGMA schema_version")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA schema_version = {}",
            version + 1
        )))
        .execute(&mut *tx)
        .await?;
        let violations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&mut *tx)
            .await?;
        if violations != 0 {
            return Err(DbError::Serialization(
                "migration 131 foreign key violation".into(),
            ));
        }
        sqlx::query("INSERT INTO _migrations(version,name) VALUES (?1,?2)")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    let restore = guard.disable().await;
    result?;
    restore
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrations::run_before_goal_storage_for_test, Database};

    async fn history(pool: &SqlitePool) -> Vec<Vec<String>> {
        let mut snapshot = Vec::new();
        for table in [
            "messages",
            "steering_messages",
            "durable_turns",
            "workflows",
            "coordinator_watch_events",
        ] {
            let columns: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
            )))
            .fetch_all(pool)
            .await
            .unwrap();
            let encoded = columns
                .into_iter()
                .map(|column| format!("typeof(\"{column}\") || ':' || hex(\"{column}\")"))
                .collect::<Vec<_>>()
                .join(" || '|' || ");
            snapshot.push(
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT {encoded} FROM {table} ORDER BY 1"
                )))
                .fetch_all(pool)
                .await
                .unwrap(),
            );
        }
        snapshot
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn populated_v130_local_subscription_rows_survive_131_with_fresh_schema_parity() {
        type Schema = (String, String, String, Option<String>);

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let db = Database::from_pool_for_tests(pool, String::new());
        db.run_migrations().await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS _migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO _migrations(version,name) VALUES (131,'fixture_skip_131')")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            run_before_goal_storage_for_test(db.pool()).await.unwrap() as usize,
            super::super::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 130)
                .count()
        );
        let source = db
            .create_conversation("historical-source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let target = db
            .create_conversation(
                "historical-coordinator",
                "coordinator",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET kind = 'coordinator', ordinary_lifecycle = NULL WHERE id = ?1").bind(target.product_conversation_id.as_str()).execute(db.pool()).await.unwrap();
        sqlx::query(
            "INSERT INTO coordinator_roots(product_conversation_id,name) VALUES (?1,'historical')",
        )
        .bind(target.product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        for index in 1..=3 {
            sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us) SELECT ?1,id,'creation',?1,1,?2,'completed',enrolled_at_us+1 FROM coordinator_watches WHERE source_product_conversation_id = ?3")
                .bind(format!("historical-event-{index}")).bind(&source.id).bind(source.product_conversation_id.as_str()).execute(db.pool()).await.unwrap();
        }
        let events: Vec<String> =
            sqlx::query_scalar("SELECT event_id FROM coordinator_watch_events ORDER BY event_id")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(events.len(), 3);
        sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,created_at,origin_kind,origin_subscription_event_id) VALUES ('historical-message',?1,1,'user','{ \"text\": \"preserve exact bytes\" }','2026-01-01T00:00:00Z','subscription_event',?2)")
            .bind(&target.id).bind(&events[0]).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO steering_messages(message_id,conversation_id,ordinal,text,origin_kind,origin_subscription_event_id) VALUES ('historical-steering',?1,0,'preserve steering','subscription_event',?2)")
            .bind(&target.id).bind(&events[1]).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,0,1,1,7,'Active','direct_turn.snapshot',1,X'0001FF',10,20)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,origin_subscription_event_id) VALUES (1,1,?1,'historical-key','historical-fingerprint',X'FF000A','Steering',7,0,'subscription_event',?2)")
            .bind(&target.id).bind(&events[2]).execute(db.pool()).await.unwrap();
        let before = history(db.pool()).await;
        assert!(before.iter().all(|rows| !rows.is_empty()));
        sqlx::query("DELETE FROM _migrations WHERE version = 131")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            run_before_goal_storage_for_test(db.pool()).await.unwrap(),
            1
        );
        assert_eq!(history(db.pool()).await, before);
        let fresh_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let fresh = Database::from_pool_for_tests(fresh_pool, String::new());
        fresh.run_migrations().await.unwrap();
        assert_eq!(
            run_before_goal_storage_for_test(fresh.pool())
                .await
                .unwrap() as usize,
            super::super::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 131)
                .count()
        );
        let schema = "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name";
        assert_eq!(
            sqlx::query_as::<_, Schema>(schema)
                .fetch_all(db.pool())
                .await
                .unwrap(),
            sqlx::query_as::<_, Schema>(schema)
                .fetch_all(fresh.pool())
                .await
                .unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM subscription_delivery_identities WHERE local_event_id IS NOT NULL"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            3
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            run_before_goal_storage_for_test(db.pool()).await.unwrap(),
            0
        );
    }
}
