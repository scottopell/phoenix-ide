use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

const INITIAL_RECEIPT_EXCEPTION: &str = "NOT EXISTS (\n    SELECT 1 FROM native_close_initial_receipts receipt\n    JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = receipt.run_ordinal\n    JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id\n    JOIN product_conversations product ON product.id = obligation.product_conversation_id\n    WHERE receipt.attempt_id = NEW.attempt_id AND receipt.run_ordinal = NEW.cleanup_run_ordinal\n      AND receipt.intent = 'initial' AND receipt.request_fingerprint <> ''\n      AND receipt.product_conversation_id = NEW.source_product_conversation_id\n      AND obligation.product_conversation_id = NEW.source_product_conversation_id\n      AND obligation.native_managed = 1\n      AND obligation.phase IN ('awaiting_blocker_resolution', 'awaiting_stop_work_confirmation', 'settling_active_work', 'awaiting_retirement_inspection')\n      AND obligation.inspection_generation IS NULL AND obligation.inspection_fingerprint IS NULL\n      AND obligation.topology_sealed = 1\n      AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open'\n      AND run.run_ordinal = 1 AND run.status = 'running'\n      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)\n      AND NEW.authority_kind IN ('explicitly_suppressed', 'attempt_interrupted')\n      AND NEW.reason = 'interrupted' AND NEW.stop_certainty = 'shutdown_uncertain'\n      AND NEW.confirmed_at_us IS NULL AND NEW.occurred_at_us >= run.created_at_us\n      AND NEW.scope IS NULL\n      AND NEW.inspection_generation IS NULL AND NEW.inspection_fingerprint IS NULL\n      AND NEW.resource_kind IS NULL AND NEW.identity_kind IS NULL\n      AND NEW.identity_codec IS NULL AND NEW.identity_value IS NULL\n      AND NOT EXISTS (SELECT 1 FROM close_retirement_inspections WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_retirement_inventories WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_retirement_losses WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_expected_retirement_resources WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_retirement_resources WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_dispatches WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes WHERE attempt_id = receipt.attempt_id)\n      AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = receipt.attempt_id)\n)";

// Preserve the completion guard's lifecycle, participant, and message checks;
// extend only its zero-resource disposition alternative. The failure INSERT's
// stop-run trigger has already ended the exact run before completion is written.
const INITIAL_SUPPRESSED_DISPOSITION: &str = r"((failure.authority_kind = 'attempt_interrupted' OR (
            failure.authority_kind = 'explicitly_suppressed'
            AND failure.scope IS NULL AND failure.resource_kind IS NULL
            AND failure.identity_kind IS NULL AND failure.identity_codec IS NULL AND failure.identity_value IS NULL
            AND failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
            AND failure.reason = 'interrupted' AND failure.stop_certainty = 'shutdown_uncertain'
            AND failure.confirmed_at_us IS NULL AND NEW.close_outcome = 'close_incomplete'
            AND NEW.native_managed = 1 AND NEW.topology_sealed = 1
            AND OLD.phase IN ('awaiting_blocker_resolution', 'awaiting_stop_work_confirmation', 'settling_active_work', 'awaiting_retirement_inspection')
            AND NEW.inspection_generation IS NULL AND NEW.inspection_fingerprint IS NULL
            AND EXISTS (SELECT 1 FROM native_close_initial_receipts receipt
                JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = receipt.run_ordinal
                WHERE receipt.attempt_id = NEW.attempt_id AND receipt.product_conversation_id = NEW.product_conversation_id
                  AND receipt.intent = 'initial' AND receipt.run_ordinal = 1 AND run.status = 'stopped'
                  AND run.ended_at_us = failure.occurred_at_us
                  AND receipt.run_ordinal = failure.cleanup_run_ordinal
                  AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = NEW.attempt_id))
            AND NOT EXISTS (SELECT 1 FROM close_retirement_inspections WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_retirement_inventories WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_retirement_losses WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_expected_retirement_resources WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_retirement_resources WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_dispatches WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes WHERE attempt_id = NEW.attempt_id)
            AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id)
        ))
            AND NOT EXISTS";

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        let schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE name = 'close_cleanup_failures_require_initial_authority'",
        )
        .fetch_one(&mut *tx)
        .await?;
        let marker = ")\nBEGIN\n    SELECT RAISE(ABORT, 'cleanup failure requires exact active expected-resource, captured-scope, or observed-process authority');";
        if schema.matches(marker).count() != 1 {
            return Err(DbError::Serialization(
                "migration 179 found unexpected initial-authority trigger definition".into(),
            ));
        }
        let replacement = format!(") AND {INITIAL_RECEIPT_EXCEPTION}\nBEGIN\n    SELECT RAISE(ABORT, 'cleanup failure requires exact active expected-resource, captured-scope, or observed-process authority');");
        sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE name = ?1")
            .bind("close_cleanup_failures_require_initial_authority")
            .bind(schema.replace(marker, &replacement))
            .execute(&mut *tx)
            .await?;
        let completion: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name = 'close_cleanup_failure_completion_requires_disposition'")
            .fetch_one(&mut *tx).await?;
        let disposition_marker = "(failure.authority_kind = 'attempt_interrupted'\n            AND NOT EXISTS";
        if completion.matches(disposition_marker).count() != 1 {
            return Err(DbError::Serialization("migration 179 found unexpected completion-disposition trigger definition".into()));
        }
        sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE name = ?1")
            .bind("close_cleanup_failure_completion_requires_disposition")
            .bind(completion.replacen(disposition_marker, INITIAL_SUPPRESSED_DISPOSITION, 1))
            .execute(&mut *tx).await?;
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
