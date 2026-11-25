use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

const AUTO_ADMISSION: &str = r"
EXISTS (
    SELECT 1 FROM native_close_completion_retry_admissions admission
    JOIN native_close_completion_retry_initial_failures origin
      ON origin.attempt_id = admission.attempt_id
     AND origin.origin_request_id = admission.origin_request_id
     AND origin.origin_request_fingerprint = admission.origin_request_fingerprint
     AND origin.product_conversation_id = admission.product_conversation_id
    JOIN close_completion_retry_allowances budget ON budget.attempt_id = admission.attempt_id
    JOIN close_obligations obligation ON obligation.attempt_id = admission.attempt_id
    WHERE admission.attempt_id = NEW.attempt_id AND admission.successor_run_ordinal = NEW.run_ordinal
      AND NEW.run_ordinal = 2 AND NEW.status = 'running'
      AND NEW.retry_evidence_kind = 'verified_completion' AND NEW.retry_requested_by = 'global'
      AND NEW.created_at_us = admission.admitted_at_us
      AND NEW.retry_observed_at_us = admission.admitted_at_us
      AND admission.admitted_at_us > origin.occurred_at_us
      AND budget.consumed = 1 AND budget.suppressed = 0
      AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NOT EXISTS (SELECT 1 FROM native_close_completion_retry_source_revocations revoked
                      WHERE revoked.attempt_id = admission.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = admission.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = admission.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success WHERE success.attempt_id = admission.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_runs later
                      WHERE later.attempt_id = admission.attempt_id AND later.run_ordinal > 1)
)
";

const AUTO_FAILURE: &str = r"
NEW.authority_kind IN ('attempt_interrupted', 'explicitly_suppressed')
AND NEW.reason = 'interrupted' AND NEW.cleanup_run_ordinal = 2
AND NEW.failure_occurrence_id = 'close-failure:2:' || NEW.attempt_id
AND NEW.inspection_generation IS NULL AND NEW.inspection_fingerprint IS NULL
AND NEW.scope IS NULL AND NEW.resource_kind IS NULL
AND NEW.identity_kind IS NULL AND NEW.identity_codec IS NULL AND NEW.identity_value IS NULL
AND EXISTS (
    SELECT 1 FROM native_close_completion_retry_successors successor
    JOIN close_obligations obligation ON obligation.attempt_id = successor.attempt_id
    WHERE successor.attempt_id = NEW.attempt_id AND successor.successor_run_ordinal = NEW.cleanup_run_ordinal
      AND successor.product_conversation_id = NEW.source_product_conversation_id
      AND successor.successor_status = 'running'
      AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NEW.occurred_at_us >= successor.created_at_us
      AND ((NEW.stop_certainty = 'shutdown_uncertain' AND NEW.confirmed_at_us IS NULL)
        OR (NEW.stop_certainty = 'conversation_and_processes_stopped'
          AND NEW.confirmed_at_us >= successor.created_at_us AND NEW.confirmed_at_us <= NEW.occurred_at_us))
)
";

const AUTO_DISPOSITION: &str = r"
OLD.phase = 'completed' AND NEW.phase = 'completed'
AND OLD.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
AND NEW.close_outcome = 'archived_cleanup_attention'
AND NEW.attempt_id IS OLD.attempt_id AND NEW.product_conversation_id IS OLD.product_conversation_id
AND NEW.created_at IS OLD.created_at AND NEW.completed_at IS OLD.completed_at
AND NEW.inspection_generation IS OLD.inspection_generation
AND NEW.inspection_fingerprint IS OLD.inspection_fingerprint
AND EXISTS (
    SELECT 1 FROM native_close_completion_retry_successors successor
    JOIN close_cleanup_failures failure ON failure.attempt_id = successor.attempt_id
     AND failure.cleanup_run_ordinal = successor.successor_run_ordinal
    JOIN coordinator_watch_events event ON event.event_id = failure.failure_occurrence_id
    WHERE successor.attempt_id = NEW.attempt_id AND successor.product_conversation_id = NEW.product_conversation_id
      AND successor.successor_status = 'stopped' AND successor.ended_at_us = failure.occurred_at_us
      AND successor.inspection_generation = NEW.inspection_generation
      AND successor.inspection_fingerprint = NEW.inspection_fingerprint
      AND failure.stop_certainty = 'conversation_and_processes_stopped'
      AND failure.confirmed_at_us IS NOT NULL
      AND failure.confirmed_at_us >= successor.created_at_us
      AND failure.confirmed_at_us <= failure.occurred_at_us
      AND failure.failure_occurrence_id = 'close-failure:2:' || NEW.attempt_id
      AND failure.source_product_conversation_id = NEW.product_conversation_id
      AND failure.authority_kind IN ('attempt_interrupted', 'explicitly_suppressed')
      AND failure.reason = 'interrupted'
      AND failure.scope IS NULL AND failure.resource_kind IS NULL
      AND failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
      AND failure.identity_kind IS NULL AND failure.identity_codec IS NULL AND failure.identity_value IS NULL
      AND event.mandatory_failure_occurrence_id = failure.failure_occurrence_id
      AND event.route_kind = 'mandatory_close_failure'
      AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                      WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
      AND EXISTS (SELECT 1 FROM messages message JOIN close_attempt_members member
                  ON member.conversation_id = message.conversation_id
                  WHERE member.attempt_id = NEW.attempt_id AND member.member_role IN ('latest', 'root_latest')
                    AND message.message_id = 'close-run-outcome:2:' || NEW.attempt_id AND message.message_type = 'system')
)
AND EXISTS (SELECT 1 FROM product_conversations product WHERE product.id = NEW.product_conversation_id
            AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'history')
AND NOT EXISTS (SELECT 1 FROM close_attempt_participants participant JOIN conversations conversation
                ON conversation.id = participant.conversation_id
                WHERE participant.attempt_id = NEW.attempt_id AND conversation.archived <> 1)
";

const AUTO_SUCCESS: &str = r"
OLD.phase = 'completed' AND NEW.phase = 'completed'
AND OLD.close_outcome IN ('close_incomplete', 'archived_cleanup_attention') AND NEW.close_outcome = 'archived'
AND NEW.attempt_id IS OLD.attempt_id AND NEW.product_conversation_id IS OLD.product_conversation_id
AND NEW.created_at IS OLD.created_at AND NEW.completed_at IS OLD.completed_at
AND NEW.inspection_generation IS OLD.inspection_generation
AND NEW.inspection_fingerprint IS OLD.inspection_fingerprint
AND EXISTS (SELECT 1 FROM native_close_completion_retry_finalizable successor
            WHERE successor.attempt_id = NEW.attempt_id AND successor.product_conversation_id = NEW.product_conversation_id)
AND EXISTS (SELECT 1 FROM messages message JOIN close_attempt_members member
            ON member.conversation_id = message.conversation_id
            WHERE member.attempt_id = NEW.attempt_id AND member.member_role IN ('latest', 'root_latest')
              AND message.message_id = 'close-run-outcome:2:' || NEW.attempt_id AND message.message_type = 'system')
AND EXISTS (SELECT 1 FROM product_conversations product WHERE product.id = NEW.product_conversation_id
            AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'history')
AND NOT EXISTS (SELECT 1 FROM close_attempt_participants participant JOIN conversations conversation
                ON conversation.id = participant.conversation_id
                WHERE participant.attempt_id = NEW.attempt_id AND conversation.archived <> 1)
";

fn unexpected(object: &str) -> DbError {
    DbError::Serialization(format!(
        "migration 185 found unexpected {object} definition"
    ))
}

fn append_exact_exception(
    definition: &str,
    object: &str,
    abort: &str,
    exception: &str,
) -> DbResult<String> {
    // SQLite stores CREATE TRIGGER without its final statement terminator.
    let definition = definition.strip_suffix(';').unwrap_or(definition);
    let suffix = format!("\nBEGIN\n    SELECT RAISE(ABORT, '{abort}');\nEND");
    let prefix = definition
        .strip_suffix(&suffix)
        .ok_or_else(|| unexpected(object))?;
    if !prefix.starts_with(&format!("CREATE TRIGGER {object}\n"))
        || prefix.matches("\nWHEN ").count() != 1
        || prefix.contains("\nBEGIN\n")
    {
        return Err(unexpected(object));
    }
    Ok(format!("{prefix}\nAND NOT ({exception}){suffix}"))
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        let object = "close_runs_require_next_ordinal";
        let definition: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
            .bind(object).fetch_one(&mut *tx).await?;
        let marker = "    AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)\n    AND NOT EXISTS (";
        if definition.matches(marker).count() != 1 {
            return Err(unexpected(object));
        }
        let replacement = definition.replacen(marker, &format!(
            "    AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)\n    AND NOT ({AUTO_ADMISSION})\n    AND NOT EXISTS ("
        ), 1);
        sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE type = 'trigger' AND name = ?1")
            .bind(object).bind(replacement).execute(&mut *tx).await?;
        for (object, abort, exception) in [
            (
                "close_cleanup_failures_require_initial_authority",
                "cleanup failure requires exact active expected-resource, captured-scope, or observed-process authority",
                AUTO_FAILURE.to_owned(),
            ),
            (
                "native_close_source_fence_requires_exact_plan",
                "source fence requires an exact native retry or effect-free initial receipt",
                AUTO_FAILURE.to_owned(),
            ),
            (
                "close_obligations_completed_outcome_is_immutable",
                "completed Close outcome is immutable without exact successful retry",
                format!("({AUTO_SUCCESS}) OR ({AUTO_DISPOSITION})"),
            ),
            (
                "close_cleanup_failure_completion_requires_disposition",
                "cleanup failure completion requires exact terminal disposition",
                AUTO_DISPOSITION.to_owned(),
            ),
        ] {
            let definition: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
                .bind(object).fetch_one(&mut *tx).await?;
            let replacement = append_exact_exception(&definition, object, abort, &exception)?;
            sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE type = 'trigger' AND name = ?1")
                .bind(object).bind(replacement).execute(&mut *tx).await?;
        }
        // These guards apply even where a historical generic exception would otherwise match.
        for (name, event, when, error) in [
            ("native_close_completion_retry_runs_require_binding", "BEFORE INSERT ON close_runs",
             format!("EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id) AND NOT ({AUTO_ADMISSION})"),
             "automatic completion run requires its exact unsuppressed admission"),
            ("native_close_completion_retry_failure_requires_binding", "BEFORE INSERT ON close_cleanup_failures",
             format!("NEW.cleanup_run_ordinal = 2 AND EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id) AND NOT ({AUTO_FAILURE})"),
             "automatic completion failure requires its exact zero-effect successor"),
            ("close_obligations_require_archived_run_proof", "BEFORE UPDATE OF phase, close_outcome ON close_obligations",
             format!("NEW.phase = 'completed' AND NEW.close_outcome = 'archived' AND EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id) AND NOT ({AUTO_SUCCESS})"),
             "automatic completion archival requires exact unsuppressed running proof"),
            ("native_close_completion_retry_failure_requires_disposition", "BEFORE UPDATE OF phase, close_outcome ON close_obligations",
             format!("NEW.phase = 'completed' AND NEW.close_outcome IN ('close_incomplete', 'archived_cleanup_attention') AND EXISTS (SELECT 1 FROM close_cleanup_failures failure JOIN native_close_completion_retry_admissions admission ON admission.attempt_id = failure.attempt_id AND admission.successor_run_ordinal = failure.cleanup_run_ordinal WHERE failure.attempt_id = NEW.attempt_id) AND NOT ({AUTO_DISPOSITION})"),
             "automatic completion failure requires exact stopped disposition and mandatory outbox"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE TRIGGER {name}\n{event}\nWHEN {when}\nBEGIN\n    SELECT RAISE(ABORT, '{error}');\nEND;"
            ))).execute(&mut *tx).await?;
        }
        let schema_version: i64 = sqlx::query_scalar("PRAGMA schema_version").fetch_one(&mut *tx).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("PRAGMA schema_version = {}", schema_version + 1)))
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
            .bind(migration.version).bind(migration.name).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    let disable = guard.disable().await;
    result.and(disable)
}
