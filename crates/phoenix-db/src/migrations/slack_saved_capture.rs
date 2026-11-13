use super::{ForeignKeysDisabledConnection, Migration};
use crate::{DbError, DbResult};
use sqlx::{Connection, SqlitePool};

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = ForeignKeysDisabledConnection {
        connection: pool.acquire().await?,
        armed: true,
    };
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *guard.connection)
        .await?;
    let result = async {
        let mut tx = guard.connection.begin_with("BEGIN IMMEDIATE").await?;
        let result: DbResult<()> = async {
            sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
            let invalid = sqlx::query("SELECT 1 FROM pragma_foreign_key_check WHERE \"table\" IN ('messages','steering_messages','durable_turns','product_creation_jobs','slack_gateway_requests','slack_gateway_return_coverage','slack_gateway_native_outcomes','slack_gateway_saved_inboxes') LIMIT 1")
                .fetch_optional(&mut *tx).await?;
            if invalid.is_some() {
                return Err(DbError::Serialization("Saved capture migration left invalid source references".into()));
            }
            sqlx::query("INSERT INTO _migrations(version,name) VALUES (?1,?2)")
                .bind(migration.version).bind(migration.name).execute(&mut *tx).await?;
            Ok(())
        }.await;
        match result {
            Ok(()) => tx.commit().await.map_err(DbError::from),
            Err(error) => { tx.rollback().await?; Err(error) }
        }
    }.await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *guard.connection)
        .await?;
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *guard.connection)
        .await?;
    if enabled != 1 {
        return Err(DbError::Serialization(
            "Saved capture migration could not restore foreign keys".into(),
        ));
    }
    guard.armed = false;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{run_pending_migrations, run_through_for_test};

    async fn seed(pool: &SqlitePool) {
        sqlx::raw_sql("INSERT INTO slack_gateway_accounts(workspace,owner,baseline_complete,status,retry_at_us) VALUES ('example.slack.com','UOWNER',1,'healthy',123);
        INSERT INTO slack_gateway_bindings(binding_id,workspace,owner,channel,thread_ts,kind,creation_id) VALUES ('binding','example.slack.com','UOWNER','C123','100.000001','explicit','creation');
        INSERT INTO slack_gateway_requests(request_id,workspace,owner,channel,message_ts,thread_ts,kind,instruction,context,source_url,binding_id,accepted_at_unix_micros,input_id,observed_at_unix_micros,trigger_kind,eyes_failure) VALUES ('request','example.slack.com','UOWNER','C123','100.000001','100.000001','explicit','investigate','quoted','https://example.slack.com/archives/C123/p100000001','binding',1,'input',1,'prefix','unconfirmed');
        INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,created_at_unix_micros) VALUES ('return','binding','done','thread','reply',1);
        INSERT INTO slack_gateway_return_coverage(return_id,request_id) VALUES ('return','request');
        INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id) VALUES ('request','done','Completed','transcript');
        INSERT INTO slack_gateway_observations(workspace,owner,channel,message_ts,first_observed_at_unix_micros) VALUES ('example.slack.com','UOWNER','C123','101.000001',456);
        INSERT INTO slack_gateway_discovery(workspace,owner,kind,after_date,cursor) VALUES ('example.slack.com','UOWNER','reaction','2000-01-01','retained');").execute(pool).await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Preserve source identity, native provenance and recovery records through one upgrade.
    async fn migration_preserves_work_references_and_reopens_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("upgrade.db");
        let db = crate::Database::open(path.to_str().unwrap()).await.unwrap();
        run_through_for_test(db.pool(), 161).await.unwrap();
        seed(db.pool()).await;
        let conversation = db
            .create_conversation(
                "migration-source",
                "migration-source",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        db.add_message_with_seq_and_origin(
            "input",
            &conversation.id,
            1,
            &phoenix_core::domain::db_schema::MessageContent::user("investigate"),
            None,
            None,
            &phoenix_core::domain::db_schema::InputOrigin::SlackGateway {
                request_id: "request".into(),
            },
        )
        .await
        .unwrap();
        let before = db
            .slack_requests("example.slack.com", "UOWNER")
            .await
            .unwrap()
            .remove(0);
        assert_eq!(run_pending_migrations(db.pool()).await.unwrap(), 1);
        let after = db
            .slack_requests("example.slack.com", "UOWNER")
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        assert_eq!(
            db.slack_returns("example.slack.com", "UOWNER")
                .await
                .unwrap()[0]
                .request_ids,
            vec!["request"]
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT request_id FROM slack_gateway_native_outcomes")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "request"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT origin_slack_request_id FROM messages WHERE message_id='input'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            "request"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT cursor FROM slack_gateway_discovery")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "retained"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT capture_scope FROM slack_gateway_observations")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "emoji"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value FROM app_settings WHERE key='slack_gateway_saved_enabled'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            "false"
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        db.pool().close().await;
        let reopened = crate::Database::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(run_pending_migrations(reopened.pool()).await.unwrap(), 0);
        assert_eq!(
            reopened
                .slack_requests("example.slack.com", "UOWNER")
                .await
                .unwrap()[0]
                .input_id,
            "input"
        );
    }

    #[tokio::test]
    async fn ledger_failure_rolls_back_rebuild_and_restores_foreign_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollback.db");
        let old = crate::Database::open(path.to_str().unwrap()).await.unwrap();
        run_through_for_test(old.pool(), 161).await.unwrap();
        seed(old.pool()).await;
        sqlx::query("CREATE TRIGGER refuse_saved_ledger BEFORE INSERT ON _migrations WHEN NEW.version=162 BEGIN SELECT RAISE(ABORT,'fixture refusal'); END").execute(old.pool()).await.unwrap();
        assert!(run_pending_migrations(old.pool()).await.is_err());
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM pragma_table_xinfo('slack_gateway_requests') WHERE name='capture_scope'").fetch_one(old.pool()).await.unwrap(),0);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT request_id FROM slack_gateway_requests")
                .fetch_one(old.pool())
                .await
                .unwrap(),
            "request"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(old.pool())
                .await
                .unwrap(),
            1
        );
        sqlx::query("DROP TRIGGER refuse_saved_ledger")
            .execute(old.pool())
            .await
            .unwrap();
        assert_eq!(run_pending_migrations(old.pool()).await.unwrap(), 1);
    }
}
