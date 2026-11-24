use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

const COMPLETED_INITIAL_INTERRUPTION: &str = r#"
NEW.authority_kind IN ('attempt_interrupted', 'explicitly_suppressed')
AND NEW.reason = 'interrupted' AND NEW.cleanup_run_ordinal = 1
AND NEW.stop_certainty = 'conversation_and_processes_stopped' AND NEW.confirmed_at_us IS NOT NULL
AND NEW.confirmed_at_us <= NEW.occurred_at_us
AND NEW.inspection_generation IS NULL AND NEW.inspection_fingerprint IS NULL
AND NEW.scope IS NULL AND NEW.resource_kind IS NULL
AND NEW.identity_kind IS NULL AND NEW.identity_codec IS NULL AND NEW.identity_value IS NULL
AND EXISTS (
    SELECT 1 FROM native_close_initial_complete_retirement_proofs proof
    JOIN close_obligations obligation ON obligation.attempt_id = proof.attempt_id
    JOIN close_runs run ON run.attempt_id = proof.attempt_id AND run.run_ordinal = proof.run_ordinal
    JOIN product_conversations product ON product.id = proof.product_conversation_id
    WHERE proof.attempt_id = NEW.attempt_id AND proof.run_ordinal = NEW.cleanup_run_ordinal
      AND proof.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.phase IN ('retirement_requested', 'needs_repair')
      AND obligation.inspection_generation = proof.inspection_generation
      AND obligation.inspection_fingerprint = proof.inspection_fingerprint
      AND run.status = 'running' AND run.ended_at_us IS NULL
      AND NEW.confirmed_at_us >= run.created_at_us
      AND product.ordinary_lifecycle = 'open'
      AND NOT EXISTS (SELECT 1 FROM close_runs later
                      WHERE later.attempt_id = proof.attempt_id AND later.run_ordinal > 1)
      AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = proof.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects resource WHERE resource.attempt_id = proof.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success WHERE success.attempt_id = proof.attempt_id)
)
"#;

const COMPLETED_INITIAL_DISPOSITION: &str = r#"
NEW.native_managed = 1 AND NEW.topology_sealed = 1
AND OLD.phase IN ('retirement_requested', 'needs_repair')
AND NEW.close_outcome = 'archived_cleanup_attention'
AND failure.cleanup_run_ordinal = 1
AND failure.authority_kind IN ('attempt_interrupted', 'explicitly_suppressed')
AND failure.reason = 'interrupted' AND failure.stop_certainty = 'conversation_and_processes_stopped'
AND failure.confirmed_at_us IS NOT NULL
AND failure.confirmed_at_us <= failure.occurred_at_us
AND failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
AND failure.scope IS NULL AND failure.resource_kind IS NULL
AND failure.identity_kind IS NULL AND failure.identity_codec IS NULL AND failure.identity_value IS NULL
AND EXISTS (SELECT 1 FROM product_conversations product
            WHERE product.id = NEW.product_conversation_id
              AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'history')
AND NOT EXISTS (SELECT 1 FROM close_attempt_participants participant
                JOIN conversations conversation ON conversation.id = participant.conversation_id
                WHERE participant.attempt_id = NEW.attempt_id AND conversation.archived <> 1)
AND EXISTS (SELECT 1 FROM coordinator_watch_events event
            WHERE event.mandatory_failure_occurrence_id = failure.failure_occurrence_id
              AND event.route_kind = 'mandatory_close_failure')
AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
AND EXISTS (
    SELECT 1 FROM native_close_initial_complete_retirement_proofs proof
    JOIN close_runs run ON run.attempt_id = proof.attempt_id AND run.run_ordinal = proof.run_ordinal
    WHERE proof.attempt_id = NEW.attempt_id AND proof.run_ordinal = failure.cleanup_run_ordinal
      AND proof.product_conversation_id = failure.source_product_conversation_id
      AND proof.product_conversation_id = NEW.product_conversation_id
      AND proof.inspection_generation = NEW.inspection_generation
      AND proof.inspection_fingerprint = NEW.inspection_fingerprint
      AND run.status = 'stopped' AND run.ended_at_us = failure.occurred_at_us
      AND failure.confirmed_at_us >= run.created_at_us
      AND NOT EXISTS (SELECT 1 FROM close_runs later
                      WHERE later.attempt_id = proof.attempt_id AND later.run_ordinal > 1)
)
"#;

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        for (object, marker, replacement) in [
            (
                "close_cleanup_failures_require_initial_authority",
                "\nBEGIN\n",
                format!("\nAND NOT ({COMPLETED_INITIAL_INTERRUPTION})\nBEGIN\n"),
            ),
            (
                "native_close_source_fence_requires_exact_plan",
                "\nBEGIN\n",
                format!("\nAND NOT ({COMPLETED_INITIAL_INTERRUPTION})\nBEGIN\n"),
            ),
            (
                "close_cleanup_failure_completion_requires_disposition",
                "AND NEW.close_outcome = 'close_incomplete'))",
                format!("AND NEW.close_outcome = 'close_incomplete') OR ({COMPLETED_INITIAL_DISPOSITION}))"),
            ),
            (
                "close_cleanup_failure_completion_requires_disposition",
                "failure.authority_kind = 'attempt_interrupted' OR (",
                format!("failure.authority_kind = 'attempt_interrupted' OR ({COMPLETED_INITIAL_DISPOSITION}) OR ("),
            ),
        ] {
            let definition: String = sqlx::query_scalar(
                "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
            )
            .bind(object)
            .fetch_one(&mut *tx)
            .await?;
            if definition.matches(marker).count() != 1 {
                return Err(DbError::Serialization(format!(
                    "migration 183 found unexpected {object} definition"
                )));
            }
            sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE type = 'trigger' AND name = ?1")
                .bind(object)
                .bind(definition.replacen(marker, &replacement, 1))
                .execute(&mut *tx)
                .await?;
        }
        let schema_version: i64 = sqlx::query_scalar("PRAGMA schema_version")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "PRAGMA schema_version = {}",
            schema_version + 1
        )))
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
    .await;
    let disable = guard.disable().await;
    result.and(disable)
}
