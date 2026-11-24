use super::{DbError, DbResult, Migration};
use sqlx::{Connection, SqlitePool};

const LEGACY_RETRY: &str = "obligation.phase = 'completed'";
const NON_NATIVE_RETRY: &str = "obligation.native_managed = 0 AND obligation.phase = 'completed'";

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    let definition: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type = 'trigger'
         AND name = 'close_retirement_resource_dispatches_require_authority'",
    )
    .fetch_one(&mut *tx)
    .await?;
    if definition.matches(LEGACY_RETRY).count() != 1
        || definition.matches("\nBEGIN").count() != 1
        || !definition.contains("WHEN NOT EXISTS (")
    {
        return Err(DbError::Serialization(
            "migration 180 found unexpected retirement dispatch authority trigger".to_owned(),
        ));
    }
    let rewritten = definition
        .replacen(LEGACY_RETRY, NON_NATIVE_RETRY, 1)
        .replacen(
            "\nBEGIN",
            &format!("\nAND NOT EXISTS (\n{}\n)\nBEGIN", migration.sql),
            1,
        );
    sqlx::raw_sql("DROP TRIGGER close_retirement_resource_dispatches_require_authority")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(rewritten))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
        .bind(migration.version)
        .bind(migration.name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
