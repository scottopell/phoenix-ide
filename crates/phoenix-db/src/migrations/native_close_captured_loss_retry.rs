use super::{DbError, DbResult, Migration, WritableSchemaGuard};
use sqlx::{Connection, SqlitePool};

const CAPTURE_HEADER: &str = r"
EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans capture
 WHERE capture.attempt_id = NEW.attempt_id AND capture.run_ordinal = NEW.run_ordinal
   AND NEW.failed_run_ordinal = 1 AND NEW.evidence_kind = 'resource_plan'
   AND NEW.inspection_generation = capture.inspection_generation
   AND NEW.inspection_fingerprint = capture.inspection_fingerprint
   AND NEW.effect_count = (SELECT COUNT(*) FROM native_close_captured_loss_retry_resources resource
       WHERE resource.attempt_id = capture.attempt_id AND resource.run_ordinal = capture.run_ordinal)
   AND NEW.effect_count = (SELECT SUM(1 + (captured_worktree_identity IS NOT NULL))
       FROM close_attempt_scopes WHERE attempt_id = capture.attempt_id)
   AND (SELECT COUNT(*) FROM native_close_captured_loss_retry_inspections inspection
       WHERE inspection.attempt_id = capture.attempt_id AND inspection.run_ordinal = capture.run_ordinal)
       = (SELECT COUNT(*) FROM close_attempt_scopes WHERE attempt_id = capture.attempt_id
           AND captured_worktree_identity IS NOT NULL)
   AND NEW.inspection_generation = (SELECT 'v1' || group_concat(component, '') FROM (
       SELECT length(CAST(scope AS BLOB)) || ':' || scope || length(CAST(generation AS BLOB)) || ':' || generation AS component
       FROM native_close_captured_loss_retry_inspections
       WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal ORDER BY scope))
   AND NEW.inspection_fingerprint = (SELECT 'v1' || group_concat(component, '') FROM (
       SELECT length(CAST(scope AS BLOB)) || ':' || scope || length(CAST(fingerprint AS BLOB)) || ':' || fingerprint AS component
       FROM native_close_captured_loss_retry_inspections
       WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal ORDER BY scope))
   AND EXISTS (SELECT 1 FROM close_runs prior
       JOIN close_obligations obligation ON obligation.attempt_id = prior.attempt_id
       JOIN close_completion_retry_allowances allowance ON allowance.attempt_id = prior.attempt_id
       JOIN native_close_initial_receipts initial ON initial.attempt_id = prior.attempt_id
           AND initial.run_ordinal = prior.run_ordinal AND initial.intent = 'initial'
           AND initial.product_conversation_id = obligation.product_conversation_id
       JOIN close_cleanup_failures failure ON failure.attempt_id = prior.attempt_id
           AND failure.cleanup_run_ordinal = prior.run_ordinal
       WHERE prior.attempt_id = NEW.attempt_id AND prior.run_ordinal = NEW.failed_run_ordinal
           AND prior.status = 'stopped' AND prior.ended_at_us = failure.occurred_at_us
           AND prior.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = NEW.attempt_id)
           AND obligation.native_managed = 1 AND obligation.topology_sealed = 1
           AND obligation.phase = 'completed'
           AND obligation.close_outcome = 'close_incomplete'
           AND obligation.inspection_generation = capture.origin_generation
           AND obligation.inspection_fingerprint = capture.origin_fingerprint
           AND allowance.suppressed = 0
           AND failure.authority_kind = 'captured_scope' AND failure.reason = 'manual_repair_required'
           AND failure.stop_certainty = 'shutdown_uncertain' AND failure.confirmed_at_us IS NULL
           AND capture.observed_at_us > failure.occurred_at_us)
   AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_original_retirement_anchors WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_retirement_inventories WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_expected_retirement_resources WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_dispatches WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_retirement_resources WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_history WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_process_step_successes WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects WHERE attempt_id = NEW.attempt_id)
   AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes WHERE attempt_id = NEW.attempt_id))
";

const CAPTURE_EFFECT: &str = r"
NOT EXISTS (SELECT 1 FROM close_runs WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects
    WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal AND ordinal = NEW.ordinal)
AND EXISTS (SELECT 1 FROM native_close_captured_loss_retry_resources resource
    JOIN native_close_retry_plans plan USING (attempt_id, run_ordinal)
    WHERE resource.attempt_id = NEW.attempt_id AND resource.run_ordinal = NEW.run_ordinal
      AND resource.ordinal = NEW.ordinal AND resource.scope = NEW.scope
      AND resource.resource_kind = NEW.resource_kind AND resource.identity_kind = NEW.identity_kind
      AND resource.identity_codec = NEW.identity_codec AND resource.identity_value = NEW.identity_value
      AND plan.effect_count > NEW.ordinal
      AND plan.inspection_generation = (SELECT inspection_generation
          FROM native_close_captured_loss_retry_plans capture
          WHERE capture.attempt_id = plan.attempt_id AND capture.run_ordinal = plan.run_ordinal)
      AND EXISTS (SELECT 1 FROM native_close_retry_resource_authorities expected
          WHERE expected.attempt_id = plan.attempt_id AND expected.scope = NEW.scope
            AND expected.inspection_generation = plan.inspection_generation
            AND expected.inspection_fingerprint = plan.inspection_fingerprint
            AND expected.resource_kind = NEW.resource_kind AND expected.identity_kind = NEW.identity_kind
            AND expected.identity_codec = NEW.identity_codec AND expected.identity_value = NEW.identity_value))
";

const ADMISSION: &str = include_str!("native_close_explicit_retry_admission.sql");
const FAILURE: &str = include_str!("native_close_retry_failure.sql");
const SOURCE_FENCE: &str = include_str!("native_initial_source_fence.sql");

fn unexpected(name: &str) -> DbError {
    DbError::Serialization(format!(
        "migration 188 found unexpected {name} source shape"
    ))
}

fn trigger_source<'a>(source: &'a str, name: &str) -> DbResult<&'a str> {
    let start = format!("CREATE TRIGGER {name}\n");
    let (prefix, tail) = source.split_once(&start).ok_or_else(|| unexpected(name))?;
    let end = tail.find("\nEND;").ok_or_else(|| unexpected(name))?;
    let at = prefix.len();
    source
        .get(at..at + start.len() + end + "\nEND".len())
        .ok_or_else(|| unexpected(name))
}

fn between<'a>(source: &'a str, name: &str, start: &str, end: &str) -> DbResult<&'a str> {
    let (_, tail) = source.split_once(start).ok_or_else(|| unexpected(name))?;
    let (part, _) = tail.split_once(end).ok_or_else(|| unexpected(name))?;
    Ok(part)
}

fn replace_exact(definition: &str, name: &str, old: &str, new: &str) -> DbResult<String> {
    if old.is_empty() || definition.matches(old).count() != 1 {
        return Err(unexpected(name));
    }
    Ok(definition.replacen(old, new, 1))
}

// ONLY call this on a complete, asserted native SELECT, never on a combined
// trigger. The old SELECT is retained byte-for-byte as the first UNION arm.
fn capture_alternative(query: &str, name: &str, resources: bool) -> DbResult<String> {
    let anchor =
        "JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = plan.attempt_id";
    let mut capture = replace_exact(query, name, anchor,
        "JOIN native_close_retry_snapshot_authorities anchor ON anchor.attempt_id = plan.attempt_id AND anchor.run_ordinal = plan.run_ordinal")?;
    if resources {
        capture = replace_exact(
            &capture,
            name,
            "close_expected_retirement_resources expected",
            "native_close_retry_resource_authorities expected",
        )?;
        capture = replace_exact(
            &capture,
            name,
            "close_retirement_inventories inventory",
            "native_close_retry_scope_authorities inventory",
        )?;
    }
    capture.push_str("\n      AND EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans capture_origin\n          WHERE capture_origin.attempt_id = plan.attempt_id AND capture_origin.run_ordinal = plan.run_ordinal)");
    Ok(format!("{query}\nUNION ALL\n{capture}"))
}

async fn definition(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, name: &str) -> DbResult<String> {
    sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
        .bind(name)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| unexpected(name))
}

async fn install(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    sql: String,
) -> DbResult<()> {
    sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE type = 'trigger' AND name = ?1")
        .bind(name)
        .bind(sql)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

// Audited list: dispatch (181), both metadata guards (182), source-fence parent
// (183 + 185) and children (179), failure parent/child (180), header/effect/run
// (178 + run's independent 185 exception), success (119), receipt binding (178).
// No discovery by LIKE: a missing or changed native branch aborts the transaction.
async fn rewrite_native_guards(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> DbResult<()> {
    for (name, query) in [
        (
            "close_retirement_resource_dispatches_require_authority",
            include_str!("native_close_retry_dispatch.sql"),
        ),
        (
            "close_worktree_cleanup_plan_require_retry_authority",
            include_str!("native_close_retry_worktree_metadata.sql"),
        ),
        (
            "close_worktree_tombstone_require_retry_authority",
            include_str!("native_close_retry_worktree_metadata.sql"),
        ),
    ] {
        let old = definition(tx, name).await?;
        let query = query.trim_end();
        let rewritten = replace_exact(&old, name, query, &capture_alternative(query, name, true)?)?;
        install(tx, name, rewritten).await?;
    }
    for name in [
        "native_close_retry_failure_parent_requires_exact_plan",
        "native_close_retry_failure_child_requires_exact_pending_tail",
    ] {
        let source = trigger_source(FAILURE, name)?;
        let old = definition(tx, name).await?;
        if old.trim_end_matches(';') != source {
            return Err(unexpected(name));
        }
        let query = between(source, name, " AND NOT EXISTS (\n", "\n)\nBEGIN")?;
        let rewritten = replace_exact(&old, name, query, &capture_alternative(query, name, true)?)?;
        install(tx, name, rewritten).await?;
    }
    let name = "native_close_source_fence_requires_exact_plan";
    let source = trigger_source(SOURCE_FENCE, name)?;
    let query = between(source, name, "    EXISTS (\n", "\n    )\n    OR EXISTS (")?;
    let old = definition(tx, name).await?;
    let rewritten = replace_exact(&old, name, query, &capture_alternative(query, name, false)?)?;
    install(tx, name, rewritten).await?;

    let name = "close_run_retry_effects_require_remaining_authority";
    let source = trigger_source(ADMISSION, name)?;
    let old = definition(tx, name).await?;
    if old.trim_end_matches(';') != source {
        return Err(unexpected(name));
    }
    let query = between(source, name, "    AND NOT EXISTS (\n", "\n    ))\nBEGIN")?;
    let rewritten = replace_exact(
        &old,
        name,
        query,
        &format!("{query}\nUNION ALL\nSELECT 1 WHERE {CAPTURE_EFFECT}"),
    )?;
    install(tx, name, rewritten).await?;

    let name = "native_close_retry_plans_require_original";
    let source = trigger_source(ADMISSION, name)?;
    let old = definition(tx, name).await?;
    if old.trim_end_matches(';') != source {
        return Err(unexpected(name));
    }
    let query = between(
        source,
        name,
        "  OR NOT EXISTS (",
        ")\n  OR EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions",
    )?;
    // The duplicate/request/latest-run/exclusion gates outside this SELECT are
    // deliberately NOT exempted. Only the original-anchor requirement gains an arm.
    let rewritten = replace_exact(
        &old,
        name,
        query,
        &format!("{query}\nUNION ALL\nSELECT 1 WHERE {CAPTURE_HEADER}"),
    )?;
    install(tx, name, rewritten).await?;

    verify_unchanged_native_guards(tx).await
}

async fn verify_unchanged_native_guards(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> DbResult<()> {
    // These guards need no new authority: they bind already admitted plans and
    // effects, not anchors/inventories. Assert their actual installed sources.
    for (source, name) in [
        (
            super::MIGRATION_119,
            "close_run_retry_successes_require_running_plan",
        ),
        (
            include_str!("native_close_source_fence.sql"),
            "native_close_source_fence_children_require_pending_plan",
        ),
        (ADMISSION, "native_close_retry_receipts_require_plan"),
    ] {
        if definition(tx, name).await?.trim_end_matches(';') != trigger_source(source, name)? {
            return Err(unexpected(name));
        }
    }
    let name = "close_runs_require_next_ordinal";
    let source = trigger_source(ADMISSION, name)?;
    let query = between(
        source,
        name,
        "    AND NOT EXISTS (\n      SELECT 1 FROM native_close_retry_plans plan\n",
        "\n    ))\nBEGIN",
    )?;
    let installed = definition(tx, name).await?;
    if installed.matches(query).count() != 1
        || !installed.contains("OR NEW.run_ordinal <> COALESCE((SELECT MAX(run_ordinal) + 1")
    {
        return Err(unexpected(name));
    }
    Ok(())
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result = async {
        let mut tx = guard.connection().begin().await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        for table in ["native_close_captured_loss_retry_plans",
            "native_close_captured_loss_retry_inspections", "native_close_captured_loss_retry_resources"] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE TRIGGER {table}_immutable BEFORE UPDATE ON {table}
                 BEGIN SELECT RAISE(ABORT, 'captured-loss retry authority is immutable'); END;"
            ))).execute(&mut *tx).await?;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE TRIGGER {table}_retained BEFORE DELETE ON {table}
                 WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
                 BEGIN SELECT RAISE(ABORT, 'captured-loss retry authority must be retained'); END;"
            ))).execute(&mut *tx).await?;
        }
        // Dispatch still requires the existing exact trigger. Its former expected-
        // inventory FK cannot represent the distinct capture-origin authority.
        // Retain the common captured scope FK and all dispatch identity constraints;
        // do not synthesize an expected-resource row or an original anchor.
        let object = "close_retirement_resource_dispatches";
        let definition: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1")
            .bind(object).fetch_one(&mut *tx).await?;
        let old = "FOREIGN KEY (\n        attempt_id, scope, inspection_generation, inspection_fingerprint,\n        resource_kind, identity_kind, identity_value\n    ) REFERENCES close_expected_retirement_resources (\n        attempt_id, scope, inspection_generation, inspection_fingerprint,\n        resource_kind, identity_kind, identity_value\n    ) ON DELETE RESTRICT";
        if definition.matches(old).count() != 1 {
            return Err(DbError::Serialization("migration 188 unexpected dispatch FK".to_owned()));
        }
        let rewritten = definition.replace(old,
            "FOREIGN KEY (attempt_id, scope) REFERENCES close_attempt_scopes(attempt_id, scope) ON DELETE RESTRICT");
        sqlx::query("UPDATE sqlite_schema SET sql = ?2 WHERE type = 'table' AND name = ?1")
            .bind(object).bind(rewritten).execute(&mut *tx).await?;

        rewrite_native_guards(&mut tx).await?;
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
