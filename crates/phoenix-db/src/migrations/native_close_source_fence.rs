use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        for (object, old, new, count) in [
            (
                "close_cleanup_failures",
                "'observed_process_resource', 'attempt_interrupted')",
                "'observed_process_resource', 'attempt_interrupted', 'explicitly_suppressed')",
                1,
            ),
            (
                "close_cleanup_failures",
                "authority_kind = 'attempt_interrupted'",
                "authority_kind IN ('attempt_interrupted', 'explicitly_suppressed')",
                2,
            ),
            (
                "close_cleanup_failures",
                "authority_kind <> 'attempt_interrupted'",
                "authority_kind NOT IN ('attempt_interrupted', 'explicitly_suppressed')",
                1,
            ),
            (
                "close_cleanup_failures_require_initial_authority",
                "WHEN NOT EXISTS (",
                "WHEN NOT (NEW.authority_kind = 'explicitly_suppressed' OR (NEW.authority_kind = 'attempt_interrupted' AND EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.cleanup_run_ordinal))) AND NOT EXISTS (",
                1,
            ),
            (
                "close_cleanup_failure_resources_require_authority",
                "WHEN NOT EXISTS (",
                "WHEN NOT EXISTS (SELECT 1 FROM close_cleanup_failures failure WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id AND (failure.authority_kind = 'explicitly_suppressed' OR (failure.authority_kind = 'attempt_interrupted' AND EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = failure.attempt_id AND plan.run_ordinal = failure.cleanup_run_ordinal)))) AND NOT EXISTS (",
                1,
            ),
        ] {
            let schema: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name = ?1")
                .bind(object)
                .fetch_one(&mut *tx)
                .await?;
            if schema.matches(old).count() != count {
                return Err(DbError::Serialization(format!(
                    "migration 179 found unexpected {object} definition"
                )));
            }
            sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE name = ?1")
                .bind(object)
                .bind(schema.replace(old, new))
                .execute(&mut *tx)
                .await?;
        }
        let version: i64 = sqlx::query_scalar("PRAGMA schema_version")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA schema_version = {}", version + 1)))
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
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
