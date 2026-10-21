use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

fn replace_once(schema: &str, old: &str, new: &str) -> DbResult<String> {
    if schema.matches(old).count() != 1 {
        return Err(DbError::Serialization(format!(
            "migration 136 expected exactly one schema clause: {old}"
        )));
    }
    Ok(schema.replacen(old, new, 1))
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        guard.connection().clear_cached_statements().await?;
        let mut tx = guard.connection().begin().await?;
        let schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='durable_turns'",
        )
        .fetch_one(&mut *tx)
        .await?;
        let schema = replace_once(
            &schema,
            "prepared_payload BLOB NOT NULL",
            "prepared_payload BLOB",
        )?;
        let schema = replace_once(
            &schema,
            "origin_kind TEXT NOT NULL DEFAULT 'unknown_historical'",
            "origin_kind TEXT DEFAULT 'unknown_historical'",
        )?;
        let changed = sqlx::query(
            "UPDATE sqlite_schema SET sql=? WHERE type='table' AND name='durable_turns'",
        )
        .bind(schema)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(DbError::Serialization(
                "migration 136 lost durable_turns table".into(),
            ));
        }
        let version: i64 = sqlx::query_scalar("PRAGMA schema_version")
            .fetch_one(&mut *tx)
            .await?;
        let version = version.checked_add(1).ok_or_else(|| {
            DbError::Serialization("migration 136 schema version overflow".into())
        })?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA schema_version = {version}"
        )))
        .execute(&mut *tx)
        .await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        for name in [
            "conversation_goal_binding_admission",
            "conversation_goal_control_binding_admission",
        ] {
            let schema: String =
                sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?")
                    .bind(name)
                    .fetch_one(&mut *tx)
                    .await?;
            let schema = replace_once(
                &schema,
                "t.canonical_message_id IS NULL",
                "t.materialized = 0",
            )?;
            let changed =
                sqlx::query("UPDATE sqlite_schema SET sql=? WHERE type='trigger' AND name=?")
                    .bind(schema)
                    .bind(name)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected();
            if changed != 1 {
                return Err(DbError::Serialization(
                    "migration 136 lost binding admission trigger".into(),
                ));
            }
        }
        let version: i64 = sqlx::query_scalar("PRAGMA schema_version")
            .fetch_one(&mut *tx)
            .await?;
        let version = version.checked_add(1).ok_or_else(|| {
            DbError::Serialization("migration 136 schema version overflow".into())
        })?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA schema_version = {version}"
        )))
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO _migrations (version,name) VALUES (?,?)")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        guard.connection().clear_cached_statements().await?;
        Ok(())
    }
    .await;
    let clear = guard.connection().clear_cached_statements().await;
    let restore = guard.disable().await;
    result?;
    clear?;
    restore
}

#[cfg(test)]
mod tests;
