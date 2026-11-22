use super::{DbError, DbResult, Migration};
use sqlx::{Connection, SqlitePool};

const INSTALLED_PARENT_PREFIX: &str = "WHEN NOT (NEW.authority_kind = 'explicitly_suppressed' OR (NEW.authority_kind = 'attempt_interrupted' AND EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.cleanup_run_ordinal))) AND NOT EXISTS (";
const INSTALLED_CHILD_PREFIX: &str = "WHEN NOT EXISTS (SELECT 1 FROM close_cleanup_failures failure WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id AND (failure.authority_kind = 'explicitly_suppressed' OR (failure.authority_kind = 'attempt_interrupted' AND EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = failure.attempt_id AND plan.run_ordinal = failure.cleanup_run_ordinal)))) AND NOT EXISTS (";

const PARENT_EXEMPTION: &str = "WHEN NOT (NEW.authority_kind = 'expected_resource' AND NEW.cleanup_run_ordinal > 1 AND EXISTS (SELECT 1 FROM close_obligations obligation WHERE obligation.attempt_id = NEW.attempt_id AND obligation.product_conversation_id = NEW.source_product_conversation_id AND obligation.native_managed = 1)) AND ";
const CHILD_EXEMPTION: &str = "WHEN NOT EXISTS (SELECT 1 FROM close_cleanup_failures failure JOIN close_obligations obligation ON obligation.attempt_id = failure.attempt_id AND obligation.product_conversation_id = failure.source_product_conversation_id WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id AND failure.authority_kind = 'expected_resource' AND failure.cleanup_run_ordinal > 1 AND obligation.native_managed = 1) AND ";

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    for (name, installed, exemption) in [
        (
            "close_cleanup_failures_require_initial_authority",
            INSTALLED_PARENT_PREFIX,
            PARENT_EXEMPTION,
        ),
        (
            "close_cleanup_failure_resources_require_authority",
            INSTALLED_CHILD_PREFIX,
            CHILD_EXEMPTION,
        ),
    ] {
        let definition: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
        )
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        if definition.matches(installed).count() != 1 {
            return Err(DbError::Serialization(format!(
                "migration 176 found unexpected {name} definition"
            )));
        }
        let replacement = format!("{exemption}{}", installed.strip_prefix("WHEN ").unwrap());
        let rewritten = definition.replacen(installed, &replacement, 1);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER {name}")))
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(rewritten))
            .execute(&mut *tx)
            .await?;
    }
    sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
        .bind(migration.version)
        .bind(migration.name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
