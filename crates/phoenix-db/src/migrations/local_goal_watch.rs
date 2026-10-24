use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        let mut schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='coordinator_watch_events'",
        ).fetch_one(&mut *tx).await?;
        for (old, new) in [
            ("'question_request', 'task_approval_wait')", "'question_request', 'task_approval_wait', 'goal_change')"),
            ("'cancelled', 'awaiting_user_response', 'awaiting_task_approval')", "'cancelled', 'awaiting_user_response', 'awaiting_task_approval', 'goal_changed') AND ((source_occurrence_kind = 'goal_change') = (terminal_kind = 'goal_changed'))"),
        ] {
            if schema.matches(old).count() != 1 {
                return Err(DbError::Serialization("migration 135 found unexpected watch CHECK".into()));
            }
            schema = schema.replacen(old, new, 1);
        }
        sqlx::query("UPDATE sqlite_schema SET sql=? WHERE type='table' AND name='coordinator_watch_events'")
            .bind(schema).execute(&mut *tx).await?;
        let version: i64 = sqlx::query_scalar("PRAGMA schema_version").fetch_one(&mut *tx).await?;
        sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA schema_version = {}", version + 1)))
            .execute(&mut *tx).await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        let violations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&mut *tx).await?;
        if violations != 0 {
            return Err(DbError::Serialization("migration 135 found foreign key violations".into()));
        }
        let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&mut *tx).await?;
        if integrity.as_slice() != ["ok"] {
            return Err(DbError::Serialization("migration 135 found integrity violations".into()));
        }
        sqlx::query("INSERT INTO _migrations (version,name) VALUES (?,?)")
            .bind(migration.version).bind(migration.name).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    let restore = guard.disable().await;
    result?;
    restore
}

#[cfg(test)]
mod tests;
